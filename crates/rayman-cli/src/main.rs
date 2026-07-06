use std::fs;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::{Command as ProcessCommand, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use clap::{Parser, ValueEnum};
use rayman_core::agent_skill::AgentSkillInstallManager;
use rayman_core::assets::{
    AssetCleanupRequest, AssetExemptRequest, AssetRetireRequest, AssetRetirementManager,
};
use rayman_core::audit;
use rayman_core::backup::BackupManager;
use rayman_core::compile::{AutoCompileResult, auto_compile_generated, compile_result_summary};
use rayman_core::config::{ConfigManager, ModelRef, parse_scalar};
use rayman_core::context::ContextKernel;
use rayman_core::control::ControlPlaneManager;
use rayman_core::customer_deploy::{CredentialRef, CustomerDeployManager, CustomerDeployUpdate};
use rayman_core::docs;
use rayman_core::evidence::{EvidenceCheckOptions, check_workspace_evidence};
use rayman_core::feature_coverage::{self, render_feature_coverage_markdown};
use rayman_core::goal::{
    GoalManager, GoalRunOptions, GoalRunUntil, build_goal_clarification,
    render_goal_clarification_text,
};
use rayman_core::instruction;
use rayman_core::integrations::IntegrationManager;
use rayman_core::model_catalog::ModelCatalogManager;
use rayman_core::models::AgentManager;
use rayman_core::project::{ProjectAnalyzer, run_benchmark_smoke};
use rayman_core::quality::{QualityIncidentDraft, QualityManager};
use rayman_core::regression_history::RegressionHistoryManager;
use rayman_core::research::ResearchManager;
use rayman_core::risk::{RiskFixOptions, RiskManager, RiskScanOptions};
use rayman_core::selfcheck::SelfManager;
use rayman_core::semantic::SemanticContextManager;
use rayman_core::session::SessionManager;
use rayman_core::skills;
use rayman_core::stats::{AuxiliaryContributionStore, AuxiliaryUsageStore};
use rayman_core::subagent::{
    SubagentDispatchRequest, SubagentLedgerManager, SubagentPlanRequest, SubagentRecordRequest,
    SubagentResultRequest, SubagentReviewRequest,
};
use rayman_core::temp::{TempCleanupOptions, TempManager};
use rayman_core::tools;
use rayman_core::trace::TraceManager;
use rayman_core::workflow;
use rayman_core::workspace::WorkspaceActivationManager;
use rayman_core::{display_path, ensure_within, yaml};
use serde_json::Value as JsonValue;

mod cli;
use cli::*;
mod regression;
use regression::run_regression_profile;
mod governance;
use governance::{cmd_eval, cmd_gate, cmd_release, cmd_security};
mod output;
use output::{
    AUXILIARY_CONTRIBUTION_LABEL, AUXILIARY_USAGE_VALUE_LABEL, format_contribution_scope,
    format_usage_scope, print_auxiliary_status, print_auxiliary_value,
    print_project_contribution_footer, usage_detail_lines,
};
#[cfg(test)]
use output::{auxiliary_detail_lines, auxiliary_status_line};
mod reminder;
mod runtime;
use runtime::{load_dotenv, root, write_or_print};

#[tokio::main]
async fn main() {
    let console_encoding = ConsoleEncodingGuard::activate();
    let mut exit_code = 0;
    if let Err(error) = run().await {
        eprintln!("错误: {error:#}");
        exit_code = 1;
    }
    drop(console_encoding);
    if exit_code != 0 {
        std::process::exit(exit_code);
    }
}

struct ConsoleEncodingGuard {
    #[cfg(windows)]
    input_code_page: Option<u32>,
    #[cfg(windows)]
    output_code_page: Option<u32>,
}

impl ConsoleEncodingGuard {
    fn activate() -> Self {
        #[cfg(windows)]
        {
            use windows_sys::Win32::System::Console::{
                GetConsoleCP, GetConsoleOutputCP, SetConsoleCP, SetConsoleOutputCP,
            };

            const CP_UTF8: u32 = 65001;

            let input_code_page = nonzero_code_page(unsafe { GetConsoleCP() });
            let output_code_page = nonzero_code_page(unsafe { GetConsoleOutputCP() });
            if input_code_page.is_some_and(|code_page| code_page != CP_UTF8) {
                let _ = unsafe { SetConsoleCP(CP_UTF8) };
            }
            if output_code_page.is_some_and(|code_page| code_page != CP_UTF8) {
                let _ = unsafe { SetConsoleOutputCP(CP_UTF8) };
            }
            Self {
                input_code_page,
                output_code_page,
            }
        }
        #[cfg(not(windows))]
        {
            Self {}
        }
    }
}

#[cfg(windows)]
impl Drop for ConsoleEncodingGuard {
    fn drop(&mut self) {
        use windows_sys::Win32::System::Console::{SetConsoleCP, SetConsoleOutputCP};

        if let Some(code_page) = self.input_code_page {
            let _ = unsafe { SetConsoleCP(code_page) };
        }
        if let Some(code_page) = self.output_code_page {
            let _ = unsafe { SetConsoleOutputCP(code_page) };
        }
    }
}

#[cfg(windows)]
fn nonzero_code_page(code_page: u32) -> Option<u32> {
    (code_page != 0).then_some(code_page)
}

async fn run() -> Result<()> {
    load_dotenv();
    let cli = Cli::parse();
    let print_contribution_footer = cli.show_stats && !command_prints_auxiliary_stats(&cli.command);
    let _reminder_guard =
        reminder_trigger_for_command(&cli.command).map(reminder::ReminderGuard::arm);
    match cli.command {
        Command::Generate(args) => cmd_generate(args)?,
        Command::Review(args) => cmd_review(args)?,
        Command::Test(args) => cmd_test(args)?,
        Command::Refactor(args) => cmd_refactor(args)?,
        Command::Explain(args) => cmd_explain(args)?,
        Command::UpdateModels(args) => cmd_update_models(args)?,
        Command::CheckModels => cmd_check_models()?,
        Command::InstallTools(args) => cmd_install_tools(args)?,
        Command::ListModels => cmd_list_models()?,
        Command::RouteModels(args) => cmd_route_models(args)?,
        Command::Config(args) => cmd_config(args)?,
        Command::Backup(command) => cmd_backup(command)?,
        Command::AgentSkill(command) => cmd_agent_skill(command)?,
        Command::WorkspaceSkill(command) => cmd_workspace_skill(command)?,
        Command::Session(command) => cmd_session(command)?,
        Command::Context(command) => cmd_context(command)?,
        Command::Project(command) => cmd_project(command)?,
        Command::Assets(command) => cmd_assets(command)?,
        Command::Impact(args) => cmd_impact(args)?,
        Command::Regression(command) => cmd_regression(command)?,
        Command::Trace(command) => cmd_trace(command)?,
        Command::Eval(command) => cmd_eval(command)?,
        Command::Security(command) => cmd_security(command)?,
        Command::Release(command) => cmd_release(command)?,
        Command::Gate(command) => cmd_gate(command)?,
        Command::Evidence(command) => cmd_evidence(command)?,
        Command::SelfCommand(command) => cmd_self(command)?,
        Command::Benchmark(command) => cmd_benchmark(command)?,
        Command::Temp(command) => cmd_temp(command)?,
        Command::Mcp(command) => cmd_mcp(command).await?,
        Command::Plugin(command) => cmd_plugin(command)?,
        Command::Api(ApiCommand::Serve {
            host,
            port,
            allow_remote,
        }) => {
            if !allow_remote && !is_loopback_host(&host) {
                bail!(
                    "api serve 默认只允许绑定 loopback 主机（明文 HTTP、无 TLS）。\
                     如确需对外暴露，请前置带安全边界的反向代理并显式加 --allow-remote。"
                );
            }
            if allow_remote && !is_loopback_host(&host) {
                eprintln!(
                    "警告: api serve 正在绑定非 loopback 主机 {host} 且无 TLS；请确保前置代理提供加密与鉴权边界。"
                );
            }
            rayman_api::serve(root()?, &host, port).await?;
        }
        Command::Docs(command) => cmd_docs(command)?,
        Command::Instruction(command) => cmd_instruction(command)?,
        Command::Auxiliary(command) => cmd_auxiliary(command)?,
        Command::Quality(command) => cmd_quality(command)?,
        Command::Risk(command) => cmd_risk(command)?,
        Command::Goal(command) => cmd_goal(command)?,
        Command::Research(command) => cmd_research(command)?,
        Command::Subagent(command) => cmd_subagent(command)?,
        Command::Models(command) => cmd_models(command)?,
        Command::Control(command) => cmd_control(command)?,
        Command::Workflow(command) => cmd_workflow(command)?,
        Command::CustomerDeploy(command) => cmd_customer_deploy(command)?,
        Command::Coverage(command) => cmd_coverage(command)?,
        Command::Doctor(command) => cmd_doctor(command)?,
        Command::Stats => cmd_stats()?,
        Command::Audit => cmd_audit()?,
    }
    if print_contribution_footer && let Err(error) = print_project_contribution_footer() {
        eprintln!("辅助AI统计页脚不可用: {error:#}");
    }
    Ok(())
}

fn reminder_trigger_for_command(command: &Command) -> Option<reminder::ReminderTrigger> {
    match command {
        Command::Goal(GoalCommand::Run { .. }) | Command::Goal(GoalCommand::Resume { .. }) => {
            Some(reminder::ReminderTrigger::GoalStopped)
        }
        Command::Goal(GoalCommand::Close { .. }) => Some(reminder::ReminderTrigger::GoalClosed),
        Command::Session(SessionCommand::Close { .. }) => {
            Some(reminder::ReminderTrigger::SessionClosed)
        }
        _ => None,
    }
}

fn command_prints_auxiliary_stats(command: &Command) -> bool {
    matches!(
        command,
        Command::Generate(_)
            | Command::Review(_)
            | Command::Test(_)
            | Command::Refactor(_)
            | Command::Explain(_)
            | Command::Auxiliary(_)
            | Command::Evidence(_)
            | Command::Goal(_)
            | Command::Research(_)
            | Command::Stats
    )
}

fn cmd_evidence(command: EvidenceCommand) -> Result<()> {
    match command {
        EvidenceCommand::Check(args) => {
            let report = check_workspace_evidence(
                root()?,
                EvidenceCheckOptions {
                    scope: args.scope.as_str().into(),
                    goal_id: args.goal_id,
                    include_advisory: args.include_advisory,
                },
            )?;
            match args.format {
                EvidenceOutputFormat::Json => {
                    println!("{}", serde_json::to_string_pretty(&report)?);
                }
                EvidenceOutputFormat::Text => {
                    println!("Evidence scope: {}", report.scope);
                    println!("Status: {}", report.status.as_str());
                    println!(
                        "Claims: {} verified={}",
                        report.claim_count, report.verified_count
                    );
                    if !report.unknowns.is_empty() {
                        println!("Unknowns:");
                        for item in &report.unknowns {
                            println!("- {item}");
                        }
                    }
                    if !report.blockers.is_empty() {
                        println!("Blockers:");
                        for item in &report.blockers {
                            println!("- {item}");
                        }
                    }
                    if !report.required_actions.is_empty() {
                        println!("Required actions:");
                        for item in &report.required_actions {
                            println!("- {item}");
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

fn cmd_generate(args: GenerateArgs) -> Result<()> {
    let mut manager = manager(&args.model)?;
    if let Some(workflow_name) = &args.workflow.workflow {
        let report = workflow::run_workflow(
            &mut manager,
            workflow_name,
            &args.prompt,
            &args.language,
            None,
            &args.workflow.requirement,
            &args.workflow.acceptance,
        )?;
        emit_workflow_report(&report, args.workflow.goal_report.as_ref())?;
        if let Some(code) = report.artifacts.get("code").and_then(JsonValue::as_str) {
            write_or_print(args.output.as_ref(), "代码已保存到", code)?;
            maybe_auto_compile(&args.language, args.output.as_deref(), args.no_auto_compile)?;
        }
        return Ok(());
    }
    let generated = skills::generate_code(&mut manager, &args.prompt, &args.language)?;
    let validation =
        skills::validate_and_fix(&mut manager, &generated, &args.prompt, &args.language)?;
    write_or_print(args.output.as_ref(), "代码已保存到", &validation.final_code)?;
    println!("\n实现后自检:");
    println!("{}", validation.validation_summary);
    maybe_auto_compile(&args.language, args.output.as_deref(), args.no_auto_compile)?;
    print_auxiliary_status(&manager);
    Ok(())
}

fn cmd_review(args: ReviewArgs) -> Result<()> {
    let source_path = args
        .file
        .canonicalize()
        .with_context(|| format!("无法读取: {}", args.file.display()))?;
    let workspace_display = display_path(&root()?);
    let source_display = display_path(&source_path);
    let code = fs::read_to_string(&source_path)?;
    enforce_review_gate(Some(&code), Some(&source_path))?;
    if args.apply_prune && args.workflow.workflow.is_some() {
        bail!("review --apply-prune 不能与 --workflow 同时使用");
    }
    let mut manager = manager(&args.model)?;
    if let Some(workflow_name) = &args.workflow.workflow {
        let report = workflow::run_workflow(
            &mut manager,
            workflow_name,
            &format!("审查代码文件 {source_display}"),
            &args.language,
            Some(&code),
            &args.workflow.requirement,
            &args.workflow.acceptance,
        )?;
        emit_workflow_report(&report, args.workflow.goal_report.as_ref())?;
        return Ok(());
    }
    let result = skills::review_code(
        &mut manager,
        &code,
        &args.language,
        Some(&workspace_display),
        Some(&source_display),
    )?;
    println!("审查结果:\n{}", result.review);
    if args.apply_prune {
        let comment = args
            .backup_comment
            .as_deref()
            .context("review --apply-prune 需要 --backup-comment")?;
        let pruned =
            skills::prune_obsolete_code(&mut manager, &code, &args.language, &result.review)?;
        if pruned.trim().is_empty() {
            bail!("review --apply-prune 返回空内容，已拒绝写回");
        }
        if pruned == code {
            println!("过时代码瘦身未产生文件变更");
        } else {
            let backup = BackupManager::new(root()?)?.create_backup(
                std::slice::from_ref(&source_display),
                comment,
                "review_apply_prune",
            )?;
            fs::write(&source_path, pruned)
                .with_context(|| format!("无法写回瘦身结果: {source_display}"))?;
            println!("过时代码瘦身已写回: {source_display}");
            println!("写回前备份: {}", backup["id"]);
        }
    }
    print_auxiliary_status(&manager);
    Ok(())
}

fn cmd_test(args: TestArgs) -> Result<()> {
    let code = fs::read_to_string(&args.file)?;
    let mut manager = manager(&args.model)?;
    if let Some(workflow_name) = &args.workflow.workflow {
        let report = workflow::run_workflow(
            &mut manager,
            workflow_name,
            &format!("为代码文件 {} 生成测试", args.file.display()),
            &args.language,
            Some(&code),
            &args.workflow.requirement,
            &args.workflow.acceptance,
        )?;
        emit_workflow_report(&report, args.workflow.goal_report.as_ref())?;
        if let Some(test_code) = report
            .artifacts
            .get("test_code")
            .and_then(JsonValue::as_str)
        {
            write_or_print(args.output.as_ref(), "测试已保存到", test_code)?;
        }
        return Ok(());
    }
    let result = skills::generate_tests(
        &mut manager,
        &code,
        &args.language,
        &[
            "positive".into(),
            "negative".into(),
            "boundary".into(),
            "cross".into(),
        ],
    )?;
    write_or_print(args.output.as_ref(), "测试已保存到", &result.test_code)?;
    print_auxiliary_status(&manager);
    Ok(())
}

fn cmd_refactor(args: RefactorArgs) -> Result<()> {
    let code = fs::read_to_string(&args.file)?;
    let mut manager = manager(&args.model)?;
    let result = skills::refactor_code(&mut manager, &code, &args.language, &args.goals)?;
    write_or_print(args.output.as_ref(), "重构结果已保存到", &result)?;
    print_auxiliary_status(&manager);
    Ok(())
}

fn cmd_explain(args: ExplainArgs) -> Result<()> {
    let code = fs::read_to_string(&args.file)?;
    let mut manager = manager(&args.model)?;
    let result = skills::explain_code(&mut manager, &code, &args.language, &args.detail_level)?;
    println!("{result}");
    print_auxiliary_status(&manager);
    Ok(())
}

fn cmd_update_models(args: UpdateModelsArgs) -> Result<()> {
    let config = ConfigManager::new(root()?)?;
    let enabled = model_updates_enabled(&config);
    let checked = args.force || enabled;
    let auxiliary_update = if checked {
        match config.refresh_auxiliary_ai_from_settings(args.force) {
            Ok(value) => value,
            Err(error) => {
                println!("  auxiliary_ai_update_error={error:#}");
                None
            }
        }
    } else {
        None
    };
    println!("模型更新执行:");
    println!("  checked={checked}");
    println!("  updated=false");
    println!("  force={}", args.force);
    if let Some(auxiliary_update) = auxiliary_update {
        println!("  auxiliary_ai_updated=true");
        println!(
            "  auxiliary_ai_base_url={}",
            auxiliary_update["preferred_base_url"]
                .as_str()
                .unwrap_or("<unknown>")
        );
    } else {
        println!("  auxiliary_ai_updated=false");
    }
    if !checked {
        println!("  reason=model auto-update disabled; use --force to check metadata");
    }
    Ok(())
}

fn cmd_check_models() -> Result<()> {
    let config = ConfigManager::new(root()?)?;
    let routing_enabled = rayman_core::config::mapping_get(&config.config, "model_routing")
        .and_then(|v| rayman_core::config::mapping_get(v, "enabled"))
        .and_then(yaml::Value::as_bool)
        .unwrap_or(true);
    println!("模型更新状态:");
    println!(
        "  自动更新: {}",
        if model_updates_enabled(&config) {
            "启用"
        } else {
            "禁用"
        }
    );
    println!("  更新间隔: {} 天", model_updates_interval_days(&config));
    println!(
        "  模型路由: {}",
        if routing_enabled { "启用" } else { "停用" }
    );
    println!(
        "  需要更新: {}",
        if model_updates_enabled(&config) && model_updates_last_update_missing(&config) {
            "是"
        } else {
            "否"
        }
    );
    let route_findings = config.model_route_catalog_findings();
    if route_findings.is_empty() {
        println!("  路由引用: 全部已在模型目录定义");
    } else {
        println!("  路由引用: {} 个未定义", route_findings.len());
        for finding in &route_findings {
            println!(
                "    - {} -> {} ({})",
                finding.source, finding.model, finding.reason
            );
        }
        bail!(
            "模型路由引用未在 config/models.yaml 定义: {}",
            route_findings.len()
        );
    }
    Ok(())
}

fn cmd_install_tools(args: InstallToolsArgs) -> Result<()> {
    let tools: Vec<String> = args
        .tools
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_string)
        .collect();
    for (tool, ok) in tools::install_required_tools(&tools)? {
        println!("{} {}", if ok { "✓" } else { "✗" }, tool);
    }
    Ok(())
}

fn cmd_list_models() -> Result<()> {
    let config = ConfigManager::new(root()?)?;
    let catalog = config
        .model_catalog()
        .context("未找到模型目录 config/models.yaml")?;
    println!("{}", yaml::to_string(catalog)?);
    Ok(())
}

fn cmd_route_models(args: RouteModelsArgs) -> Result<()> {
    let config = ConfigManager::new(root()?)?;
    let explicit = explicit_model_override(&args.model.model_type, &args.model.model_name)?;
    let routes = config.route_candidates(
        args.model.task.as_deref(),
        args.model.route_mode.as_deref(),
        explicit,
        args.model.no_fallback,
    );
    println!("模型路由:");
    for route in routes {
        println!("- {}", route.as_string());
    }
    Ok(())
}

fn cmd_config(args: ConfigArgs) -> Result<()> {
    let mut config = ConfigManager::from_path(args.config_path)?;
    match args.action {
        ConfigAction::Show => println!("{}", yaml::to_string(&config.config)?),
        ConfigAction::Get { key } => {
            let value = config
                .get(&key)
                .with_context(|| format!("配置不存在: {key}"))?;
            println!("{}", yaml::to_string(value)?.trim());
        }
        ConfigAction::Set { key, value } => {
            let parsed = parse_scalar(&value);
            config.set(&key, parsed)?;
            config.save()?;
            println!("已设置配置: {key}");
        }
    }
    Ok(())
}

fn cmd_backup(command: BackupCommand) -> Result<()> {
    let manager = BackupManager::new(root()?)?;
    match command {
        BackupCommand::Create { paths, message } => {
            let backup = manager.create_backup(&paths, &message, "manual")?;
            println!("备份已创建: {}", backup["id"]);
            if let Some(hint) = manager.stale_hint()? {
                println!("{hint}");
            }
        }
        BackupCommand::List => {
            let backups = manager.list_backups()?;
            if backups.is_empty() {
                println!("暂无备份");
            }
            for backup in backups {
                println!(
                    "- {} [{}] {} bytes | {}",
                    backup["id"].as_str().unwrap_or("unknown"),
                    if backup["stale"].as_bool().unwrap_or(false) {
                        "过时"
                    } else {
                        "保留"
                    },
                    backup["size"].as_u64().unwrap_or(0),
                    backup["comment"].as_str().unwrap_or("")
                );
            }
        }
        BackupCommand::Restore { backup_id } => {
            let result = manager.restore_backup(&backup_id)?;
            println!("备份已还原: {backup_id}");
            println!(
                "还原文件数: {}",
                result["restored_files"]
                    .as_array()
                    .map(Vec::len)
                    .unwrap_or(0)
            );
        }
        BackupCommand::Cleanup { stale } => {
            if !stale {
                bail!("cleanup 目前只支持 --stale");
            }
            let result = manager.cleanup_stale()?;
            println!("已清理过时备份: {} 份", result["removed_count"]);
        }
    }
    Ok(())
}

fn cmd_agent_skill(command: AgentSkillCommand) -> Result<()> {
    match command {
        AgentSkillCommand::Sync {
            target,
            canonical_root,
        } => {
            let manager = AgentSkillInstallManager::new(canonical_root.unwrap_or(root()?))?;
            let results = manager.sync(&target)?;
            print_agent_results("Agent skill 入口已同步:", &results);
            if results.iter().any(|result| !result.ok()) {
                bail!("存在未同步成功的 agent skill 入口");
            }
        }
        AgentSkillCommand::Status {
            target,
            canonical_root,
        } => {
            let manager = AgentSkillInstallManager::new(canonical_root.unwrap_or(root()?))?;
            let results = manager.status(&target)?;
            print_agent_results("Agent skill 入口状态:", &results);
        }
    }
    Ok(())
}

fn cmd_workspace_skill(command: WorkspaceSkillCommand) -> Result<()> {
    let manager = WorkspaceActivationManager::new(root()?)?;
    let status = match command {
        WorkspaceSkillCommand::Status => manager.status()?,
        WorkspaceSkillCommand::Enable { message } => manager.enable(
            message
                .as_deref()
                .unwrap_or("enable raymancodingskill for this workspace"),
            "manual",
        )?,
        WorkspaceSkillCommand::Disable { message } | WorkspaceSkillCommand::Stop { message } => {
            manager.disable(
                message
                    .as_deref()
                    .unwrap_or("disable raymancodingskill for this workspace"),
                "manual",
            )?
        }
        WorkspaceSkillCommand::MarkUsed { message } => manager.record_use(
            message
                .as_deref()
                .unwrap_or("workspace used raymancodingskill"),
            "auto",
        )?,
    };
    println!("{}", yaml::to_string(&status)?);
    Ok(())
}

fn cmd_session(command: SessionCommand) -> Result<()> {
    let manager = SessionManager::new(root()?)?;
    match command {
        SessionCommand::Status => {
            let pending = manager.list_pending()?;
            println!("RaymanCodingSkill 待完成状态:");
            println!("  工作区: {}", display_path(&manager.workspace));
            println!("  状态文件: {}", display_path(&manager.state_path));
            if pending.is_empty() {
                println!("  暂无待完成项");
            } else {
                println!("  待完成数量: {}", pending.len());
                for item in pending {
                    println!(
                        "- {} [{}/{}] {}",
                        item["id"].as_str().unwrap_or("unknown"),
                        item["priority"].as_str().unwrap_or("must"),
                        item["kind"].as_str().unwrap_or("task"),
                        item["title"].as_str().unwrap_or("")
                    );
                }
            }
            let goal_manager = GoalManager::new(root()?)?;
            if let Some(goal) = goal_manager.next_active_goal()? {
                println!("  下一目标: {} [{}]", goal.id, goal.status);
                println!("  阶段: {}", goal.current_stage);
                println!("  下一步: {}", goal.next_action);
            }
        }
        SessionCommand::Recover => {
            let report = manager.recovery_report()?;
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
        SessionCommand::AddPending {
            title,
            message,
            kind,
            priority,
            source,
        } => {
            let item = manager.add_pending(
                &title,
                message.as_deref().unwrap_or(""),
                &value_name(kind),
                &source,
                &value_name(priority),
                JsonValue::Object(Default::default()),
            )?;
            println!("已登记待完成项: {}", item["id"]);
        }
        SessionCommand::Complete { item_id, message } => {
            let item = manager.complete(&item_id, message.as_deref().unwrap_or(""))?;
            println!("已完成待完成项: {}", item["id"]);
        }
        SessionCommand::Close {
            status,
            message,
            next_step,
        } => {
            let status = value_name(status);
            if status.as_str() != "success" && message.as_deref().unwrap_or("").is_empty() {
                bail!("session close 非 success 状态需要 -m/--message 说明未完成内容");
            }
            let result = manager.close_session(
                &status,
                message.as_deref().unwrap_or("session completed"),
                &next_step,
            )?;
            if result["blocked"].as_bool().unwrap_or(false) {
                println!("会话存在未完成项，不能视为完全完成");
            } else {
                println!("会话已完成，暂无待完成项");
            }
        }
    }
    Ok(())
}

fn cmd_context(command: ContextCommand) -> Result<()> {
    let kernel = ContextKernel::new(root()?)?;
    match command {
        ContextCommand::Status { check } => {
            let status = kernel.status()?;
            println!("RaymanCodingSkill Context Kernel:");
            println!(
                "  工作区: {}",
                status["workspace_path"].as_str().unwrap_or("")
            );
            println!("  状态: {}", status["status"].as_str().unwrap_or("unknown"));
            println!(
                "  记录数: {}",
                status["counts"]["records"].as_u64().unwrap_or(0)
            );
            println!(
                "  待完成: {}",
                status["counts"]["pending_work"].as_u64().unwrap_or(0)
            );
            println!(
                "  审查阻断: {}",
                status["counts"]["review_blockers"].as_u64().unwrap_or(0)
            );
            println!(
                "  审计发现: {}",
                status["counts"]["audit_findings"].as_u64().unwrap_or(0)
            );
            println!(
                "  上下文索引待刷新: {}",
                status["counts"]["context_index_stale"]
                    .as_u64()
                    .unwrap_or(0)
            );
            println!(
                "  文件索引: {}",
                status["counts"]["file_inventory"].as_u64().unwrap_or(0)
            );
            println!(
                "  符号索引: {}",
                status["counts"]["symbol_index"].as_u64().unwrap_or(0)
            );
            if !status["next_record"].is_null() {
                println!(
                    "  下一项: [{}] {}",
                    status["next_record"]["kind"].as_str().unwrap_or("record"),
                    status["next_record"]["title"].as_str().unwrap_or("")
                );
            }
            if let Some(policy) = status["source_policy"].as_str() {
                println!("  来源策略: {policy}");
            }
            if let Some(actions) = status["required_actions"].as_array()
                && !actions.is_empty()
            {
                println!("  下一步动作:");
                for action in actions {
                    if let Some(action) = action.as_str() {
                        println!("    - {action}");
                    }
                }
            }
            if check
                && status["counts"]["context_index_stale"]
                    .as_u64()
                    .unwrap_or(1)
                    > 0
            {
                bail!("context index is missing or stale; run rayman context refresh");
            }
            if check && status["counts"]["context_os_stale"].as_u64().unwrap_or(1) > 0 {
                bail!("context os state is missing or stale; run rayman context os --write");
            }
        }
        ContextCommand::Refresh => {
            let index = kernel.refresh_index()?;
            println!("RaymanCodingSkill Context Index refreshed:");
            println!("  工作区: {}", index.workspace_path);
            println!("  索引文件: .RaymanCodingSkill/context/index.json");
            println!("  Context OS: .RaymanCodingSkill/context/state.json");
            println!("  项目输入: {}", index.project_inputs.len());
            println!("  文件: {}", index.file_inventory.files.len());
            println!("  符号: {}", index.symbol_index.len());
            println!("  入口点: {}", index.dependency_map.entry_points.len());
        }
        ContextCommand::Os { write, check } => {
            let report = if write {
                let state = kernel.refresh_context_os("manual_context_os_write")?;
                serde_json::json!({
                    "written": true,
                    "status": "ready",
                    "state_path": ".RaymanCodingSkill/context/state.json",
                    "event_log_path": ".RaymanCodingSkill/context/events.jsonl",
                    "state": state,
                })
            } else {
                kernel.context_os_status()?
            };
            if check && report["status"].as_str().unwrap_or("missing") != "ready" {
                bail!("context os state is missing or stale; run rayman context os --write");
            }
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
        ContextCommand::Task { query } => {
            println!(
                "{}",
                serde_json::to_string_pretty(&kernel.task_context(&query)?)?
            );
        }
        ContextCommand::Semantic(command) => {
            let semantic = SemanticContextManager::new(root()?)?;
            match command {
                SemanticCommand::Build => {
                    println!("{}", serde_json::to_string_pretty(&semantic.build()?)?);
                }
                SemanticCommand::Query { query } => {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&semantic.query(&query)?)?
                    );
                }
                SemanticCommand::Status { check } => {
                    let status = semantic.status();
                    if check && status.status != "passed" {
                        bail!(
                            "semantic context status blocked: {}",
                            status.blockers.join("; ")
                        );
                    }
                    println!("{}", serde_json::to_string_pretty(&status)?);
                }
            }
        }
        ContextCommand::List => {
            let summary = kernel.collect()?;
            for record in summary.records {
                println!(
                    "- {} [{}:{}] {}",
                    record.id, record.kind, record.status, record.title
                );
            }
        }
        ContextCommand::Explain => {
            println!("{}", serde_json::to_string_pretty(&kernel.explain()?)?);
        }
    }
    Ok(())
}

fn cmd_project(command: ProjectCommand) -> Result<()> {
    let analyzer = ProjectAnalyzer::new(root()?)?;
    match command {
        ProjectCommand::Detect => {
            println!("{}", serde_json::to_string_pretty(&analyzer.detect()?)?);
        }
        ProjectCommand::Index => {
            let project = analyzer.write_index()?;
            ContextKernel::new(root()?)?.refresh_index()?;
            println!("{}", serde_json::to_string_pretty(&project)?);
        }
    }
    Ok(())
}

fn cmd_assets(command: AssetsCommand) -> Result<()> {
    let manager = AssetRetirementManager::new(root()?)?;
    let report = match command {
        AssetsCommand::Status => manager.status()?,
        AssetsCommand::Scan => manager.scan()?,
        AssetsCommand::Cleanup(args) => {
            manager.cleanup(AssetCleanupRequest { apply: args.apply })?
        }
        AssetsCommand::Retire(args) => manager.retire(AssetRetireRequest {
            path: args.path,
            replacement_behavior: args.replacement,
            deletion_reason: args.reason,
            validation_command: args.validation_command,
            apply_delete: args.apply_delete,
        })?,
        AssetsCommand::Exempt(args) => manager.exempt(AssetExemptRequest {
            path: args.path,
            retention_reason: args.reason,
            expires_at: args.expires_at,
        })?,
    };
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

fn cmd_impact(args: ImpactArgs) -> Result<()> {
    let analyzer = ProjectAnalyzer::new(root()?)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&analyzer.impact(&args.path)?)?
    );
    Ok(())
}

fn cmd_regression(command: RegressionCommand) -> Result<()> {
    match command {
        RegressionCommand::Plan { path } => {
            let analyzer = ProjectAnalyzer::new(root()?)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&analyzer.regression_plan(&path)?)?
            );
        }
        RegressionCommand::Run { profile } => {
            run_regression_profile(root()?, profile)?;
        }
        RegressionCommand::History { limit } => {
            let manager = RegressionHistoryManager::new(root()?)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&manager.history(limit)?)?
            );
        }
    }
    Ok(())
}

fn cmd_trace(command: TraceCommand) -> Result<()> {
    let manager = TraceManager::new(root()?)?;
    match command {
        TraceCommand::Record {
            kind,
            message,
            evidence,
        } => {
            let event = manager.record(&kind, &message, &evidence)?;
            println!("{}", serde_json::to_string_pretty(&event)?);
        }
        TraceCommand::Status => {
            println!("{}", serde_json::to_string_pretty(&manager.status())?);
        }
        TraceCommand::Replay => {
            println!("{}", serde_json::to_string_pretty(&manager.replay()?)?);
        }
    }
    Ok(())
}

fn cmd_self(command: SelfCommand) -> Result<()> {
    let manager = SelfManager::new(root()?)?;
    match command {
        SelfCommand::Status => {
            println!("{}", serde_json::to_string_pretty(&manager.status()?)?);
        }
        SelfCommand::Install => {
            println!("{}", serde_json::to_string_pretty(&manager.install()?)?);
        }
    }
    Ok(())
}

fn cmd_benchmark(command: BenchmarkCommand) -> Result<()> {
    match command {
        BenchmarkCommand::Run { smoke } => {
            if !smoke {
                bail!("benchmark run currently requires --smoke");
            }
            let report = run_benchmark_smoke()?;
            println!("{}", serde_json::to_string_pretty(&report)?);
            if report.status != "passed" {
                bail!("benchmark smoke failed");
            }
        }
    }
    Ok(())
}

fn cmd_temp(command: TempCommand) -> Result<()> {
    let manager = TempManager::new(root()?)?;
    match command {
        TempCommand::Status => {
            println!("{}", serde_json::to_string_pretty(&manager.status()?)?);
        }
        TempCommand::Cleanup {
            completed,
            stale,
            all_failed,
            cargo_targets,
        } => {
            if !completed && !stale && !all_failed && !cargo_targets {
                bail!(
                    "temp cleanup requires --completed, --stale, --all-failed, or --cargo-targets"
                );
            }
            let report = manager.cleanup(&TempCleanupOptions {
                completed,
                stale,
                all_failed,
                cargo_targets,
            })?;
            println!("{}", serde_json::to_string_pretty(&report)?);
            if !report.failed.is_empty() {
                bail!("temp cleanup failed for one or more managed temp entries");
            }
        }
        TempCommand::Doctor => {
            let report = manager.doctor();
            println!("{}", serde_json::to_string_pretty(&report)?);
            if report.status == "failed" {
                bail!("temp doctor failed");
            }
        }
    }
    Ok(())
}

async fn cmd_mcp(command: McpCommand) -> Result<()> {
    let manager = IntegrationManager::new(root()?)?;
    match command {
        McpCommand::Schema => {
            println!("{}", serde_json::to_string_pretty(&manager.schema())?);
        }
        McpCommand::Serve(args) => {
            if args.stdio == args.http {
                bail!("choose exactly one MCP transport: --stdio or --http");
            }
            if args.stdio {
                serve_mcp_stdio(manager)?;
            } else {
                if !is_loopback_host(&args.host) {
                    bail!(
                        "MCP HTTP transport must bind to loopback host unless an external proxy adds its own security boundary"
                    );
                }
                eprintln!(
                    "rayman MCP HTTP listening on http://{}:{}/mcp",
                    args.host, args.port
                );
                rayman_api::serve_mcp(root()?, &args.host, args.port).await?;
            }
        }
    }
    Ok(())
}

fn is_loopback_host(host: &str) -> bool {
    matches!(host, "127.0.0.1" | "localhost" | "::1")
}

fn serve_mcp_stdio(manager: IntegrationManager) -> Result<()> {
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let payload: JsonValue = serde_json::from_str(&line)?;
        if let Some(response) = manager.mcp_rpc_response(payload) {
            writeln!(stdout, "{}", serde_json::to_string(&response)?)?;
            stdout.flush()?;
        }
    }
    Ok(())
}

fn cmd_plugin(command: PluginCommand) -> Result<()> {
    let manager = IntegrationManager::new(root()?)?;
    match command {
        PluginCommand::Export => {
            println!(
                "{}",
                serde_json::to_string_pretty(&manager.export_plugin()?)?
            );
        }
    }
    Ok(())
}

fn cmd_models(command: ModelsCommand) -> Result<()> {
    let manager = ModelCatalogManager::new(root()?)?;
    match command {
        ModelsCommand::Refresh { dry_run, apply } => {
            let apply = apply && !dry_run;
            println!(
                "{}",
                serde_json::to_string_pretty(&manager.refresh(apply)?)?
            );
        }
        ModelsCommand::Status { check } => {
            let status = if check {
                manager.assert_passed()?
            } else {
                manager.status()?
            };
            println!("{}", serde_json::to_string_pretty(&status)?);
        }
    }
    Ok(())
}

fn cmd_control(command: ControlCommand) -> Result<()> {
    match command {
        ControlCommand::Status { format } => {
            let snapshot = ControlPlaneManager::new(root()?)?.snapshot()?;
            match format {
                ControlOutputFormat::Json => {
                    println!("{}", serde_json::to_string_pretty(&snapshot)?);
                }
                ControlOutputFormat::Text => {
                    println!("RaymanCodingSkill control status: {}", snapshot.status);
                    println!("  工作区: {}", snapshot.workspace_path);
                    println!("  required_actions={}", snapshot.required_actions.len());
                    for action in snapshot.required_actions.iter().take(20) {
                        println!("    - {action}");
                    }
                }
            }
        }
    }
    Ok(())
}

fn cmd_workflow(command: WorkflowCommand) -> Result<()> {
    let manager = workflow::WorkflowLearningManager::new(root()?)?;
    match command {
        WorkflowCommand::Learn { name, evidence } => {
            let candidate = manager.learn(&name, &evidence)?;
            println!("{}", serde_json::to_string_pretty(&candidate)?);
        }
        WorkflowCommand::Promote { id } => {
            let candidate = manager.promote(&id)?;
            println!("{}", serde_json::to_string_pretty(&candidate)?);
        }
        WorkflowCommand::Status => {
            println!("{}", serde_json::to_string_pretty(&manager.status()?)?);
        }
    }
    Ok(())
}

fn cmd_docs(command: DocsCommand) -> Result<()> {
    match command {
        DocsCommand::Maintain(args) => {
            let report = docs::maintain_html_docs(docs::DocsMaintainOptions {
                root: args.root.unwrap_or(root()?),
                output: args.output,
                prompt: args.prompt,
                prompt_file: args.prompt_file,
                model_output: args.model_output,
                dry_run: args.dry_run,
                check: args.check,
                apply_prune: args.apply_prune,
            })?;
            println!("{}", serde_json::to_string_pretty(&report)?);
            if report.status == "blocked" || (report.check && report.status != "current") {
                bail!("docs maintain status={}", report.status);
            }
        }
        DocsCommand::Compress {
            file,
            budget_chars,
            output,
        } => {
            bail!(
                "docs compress 已停用，因为单文件压缩会丢失信息；请使用 rayman docs compact-skill-rules --root {} --dry-run 检查无损拆分。原请求: file={}, budget_chars={}, output={}",
                display_path(&root()?),
                file.display(),
                budget_chars,
                output
                    .as_ref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| "<stdout>".into())
            );
        }
        DocsCommand::CompactSkillRules {
            dry_run,
            root: compact_root,
        } => {
            let compact_root = compact_root.unwrap_or(root()?);
            let summary = docs::compact_skill_rules(&compact_root, dry_run)?;
            println!("skill rule lossless splitting:");
            println!("  root: {}", display_path(&summary.root));
            println!("  dry_run: {}", summary.dry_run);
            println!("  scanned: {}", summary.scanned_files);
            println!("  split: {}", summary.split_files);
            println!("  skipped: {}", summary.skipped_files);
            for report in summary.reports {
                println!(
                    "- {} {} original={} final={} refs={}",
                    report.action,
                    display_path(&report.path),
                    report.original_chars,
                    report.final_chars,
                    report.references.len()
                );
                for reference in report.references {
                    println!("  ref: {}", display_path(&reference));
                }
            }
        }
    }
    Ok(())
}

fn cmd_instruction(command: InstructionCommand) -> Result<()> {
    match command {
        InstructionCommand::Audit => {
            instruction::assert_stale_instructions_released(&root()?)?;
            println!("指令生命周期审计通过");
        }
    }
    Ok(())
}

fn cmd_auxiliary(command: AuxiliaryCommand) -> Result<()> {
    match command {
        AuxiliaryCommand::Advise(args) => {
            let mut manager = AgentManager::new(root()?, None, None, None, false)?;
            let advice = manager.auxiliary_advice(&args.message, Some(&args.task))?;
            match advice {
                Some(advice) => println!("辅助AI建议:\n{advice}"),
                None => println!("辅助AI建议: <none>"),
            }
            print_auxiliary_status(&manager);
        }
        AuxiliaryCommand::Target => {
            let manager = AgentManager::new(root()?, None, None, None, false)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&manager.auxiliary_target_report())?
            );
        }
        AuxiliaryCommand::Status => {
            let manager = AgentManager::new(root()?, None, None, None, false)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&manager.auxiliary_task_status_json()?)?
            );
        }
        AuxiliaryCommand::Reconcile => {
            let manager = AgentManager::new(root()?, None, None, None, false)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&manager.reconcile_auxiliary_tasks()?)?
            );
        }
        AuxiliaryCommand::Worker(args) => {
            let mut manager = AgentManager::new(root()?, None, None, None, false)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&manager.run_auxiliary_worker(&args.task_id)?)?
            );
        }
    }
    Ok(())
}

fn cmd_quality(command: QualityCommand) -> Result<()> {
    let workspace_root = root()?;
    let manager = QualityManager::new(&workspace_root)?;
    match command {
        QualityCommand::Incident(QualityIncidentCommand::Add(args)) => {
            let incident = manager.add_incident(QualityIncidentDraft {
                source: args.source,
                symptom: args.symptom,
                root_cause: args.root_cause,
                fix: args.fix,
                generalized_behavior: args.generalized_behavior,
                pattern_id: args.pattern,
                tags: args.tag,
            })?;
            println!("{}", serde_json::to_string_pretty(&incident)?);
        }
        QualityCommand::Patterns => {
            println!(
                "{}",
                serde_json::to_string_pretty(&manager.patterns_json()?)?
            );
        }
        QualityCommand::Gate { goal_id } => {
            let goal = GoalManager::new(&workspace_root)?.get_goal(goal_id.as_deref())?;
            let report = manager.gate_goal(&goal, None)?;
            println!("{}", serde_json::to_string_pretty(&report)?);
            if report.status != "passed" {
                bail!(
                    "质量模式硬门禁未通过: {}",
                    report.missing_evidence.join("; ")
                );
            }
        }
    }
    Ok(())
}

fn cmd_risk(command: RiskCommand) -> Result<()> {
    let manager = RiskManager::new(root()?)?;
    match command {
        RiskCommand::Scan { no_write } => {
            let report = manager.scan(RiskScanOptions {
                write_ledger: !no_write,
                include_expensive: true,
            })?;
            println!("{}", serde_json::to_string_pretty(&report)?);
            if report.unresolved_high_critical_count > 0 {
                bail!(
                    "风险扫描存在未解决 high/critical 风险: {}",
                    report.unresolved_high_critical_count
                );
            }
        }
        RiskCommand::Plan => {
            println!("{}", serde_json::to_string_pretty(&manager.plan()?)?);
        }
        RiskCommand::Fix(args) => {
            let safe_only = !args.guarded;
            let report = manager.fix(RiskFixOptions {
                safe_only: args.safe_only || safe_only,
                guarded: args.guarded,
            })?;
            println!("{}", serde_json::to_string_pretty(&report)?);
            if report.post_scan.unresolved_high_critical_count > 0 {
                bail!(
                    "风险自动修复后仍阻塞: {}",
                    report.required_actions.join("; ")
                );
            }
        }
        RiskCommand::Verify => {
            let report = manager.verify()?;
            println!("{}", serde_json::to_string_pretty(&report)?);
            if report.status != "passed" {
                bail!("风险验证未通过: {}", report.required_actions.join("; "));
            }
        }
        RiskCommand::Learn => {
            println!("{}", serde_json::to_string_pretty(&manager.learn()?)?);
        }
    }
    Ok(())
}

fn cmd_goal(command: GoalCommand) -> Result<()> {
    let manager = GoalManager::new(root()?)?;
    match command {
        GoalCommand::Clarify {
            goal,
            requirement,
            acceptance,
            verify,
            assumption,
            format,
        } => {
            let clarification =
                build_goal_clarification(&goal, &requirement, &acceptance, &verify, &assumption);
            match format {
                ClarificationOutputFormat::Text => {
                    println!("{}", render_goal_clarification_text(&clarification));
                }
                ClarificationOutputFormat::Json => {
                    println!("{}", serde_json::to_string_pretty(&clarification)?);
                }
            }
        }
        GoalCommand::Start {
            goal,
            workflow,
            requirement,
            acceptance,
            verify,
            assumption,
        } => {
            let record = manager.start(
                &goal,
                &workflow,
                &requirement,
                &acceptance,
                &verify,
                &assumption,
            )?;
            println!("目标已创建: {}", record.id);
            print_goal_record(&record);
        }
        GoalCommand::Run {
            id,
            validation,
            message,
            until,
            checkpoint_interval,
            max_repair_attempts,
        } => {
            let record = if let Some(validation) = validation {
                manager.record_validation_result(
                    id.as_deref(),
                    matches!(validation, ValidationStatus::Passed),
                    message.as_deref().unwrap_or("validation result recorded"),
                )?
            } else {
                let root = root()?;
                let mut agent = AgentManager::new(root, None, None, None, false).ok();
                let report = manager.run_layered(
                    id.as_deref(),
                    agent.as_mut(),
                    goal_run_options(until, checkpoint_interval, max_repair_attempts),
                )?;
                println!(
                    "长程运行: status={} iterations={} stopped_reason={} resume={}",
                    report.status, report.iterations, report.stopped_reason, report.resume_command
                );
                print_host_subagent_dispatch_request(&report)?;
                report.goal
            };
            println!("目标已推进: {}", record.id);
            print_goal_record(&record);
        }
        GoalCommand::Resume {
            id,
            until,
            checkpoint_interval,
            max_repair_attempts,
        } => {
            let root = root()?;
            let mut agent = AgentManager::new(root, None, None, None, false).ok();
            let report = manager.resume(
                Some(&id),
                agent.as_mut(),
                goal_run_options(until, checkpoint_interval, max_repair_attempts),
            )?;
            println!(
                "目标已恢复: {} status={} iterations={} stopped_reason={} resume={}",
                report.goal.id,
                report.status,
                report.iterations,
                report.stopped_reason,
                report.resume_command
            );
            print_host_subagent_dispatch_request(&report)?;
            print_goal_record(&report.goal);
        }
        GoalCommand::Status { id } => {
            let status = manager.status(id.as_deref())?;
            println!("{}", serde_json::to_string_pretty(&status)?);
        }
        GoalCommand::Close {
            id,
            status,
            message,
            next_step,
        } => {
            let status = value_name(status);
            if status != "success" && message.as_deref().unwrap_or("").is_empty() {
                bail!("goal close 非 success 状态需要 -m/--message 说明未完成或阻塞原因");
            }
            let record = manager.close_goal(
                id.as_deref(),
                &status,
                message.as_deref().unwrap_or("goal completed"),
                &next_step,
            )?;
            println!("目标已关闭: {}", record.id);
            print_goal_record(&record);
        }
    }
    Ok(())
}

fn cmd_research(command: ResearchCommand) -> Result<()> {
    let workspace_root = root()?;
    let manager = ResearchManager::new(&workspace_root)?;
    match command {
        ResearchCommand::Start { question, goal_id } => {
            let session = manager.start(&question, goal_id)?;
            println!("{}", serde_json::to_string_pretty(&session)?);
        }
        ResearchCommand::Run(args) => {
            let session = thread::spawn(move || {
                let manager = ResearchManager::new(&workspace_root)?;
                let mut agent = research_run_agent(workspace_root, &args)?;
                manager.run_once(args.id.as_deref(), agent.as_mut())
            })
            .join()
            .map_err(|_| anyhow::anyhow!("research run worker panicked"))??;
            println!("{}", serde_json::to_string_pretty(&session)?);
        }
        ResearchCommand::Status { id } => {
            println!(
                "{}",
                serde_json::to_string_pretty(&manager.status(id.as_deref())?)?
            );
        }
        ResearchCommand::Reconcile { id } => {
            let session = manager.reconcile(id.as_deref())?;
            println!("{}", serde_json::to_string_pretty(&session)?);
        }
        ResearchCommand::Report { id } => {
            println!(
                "{}",
                serde_json::to_string_pretty(&manager.report(id.as_deref())?)?
            );
        }
    }
    Ok(())
}

fn cmd_subagent(command: SubagentCommand) -> Result<()> {
    let manager = SubagentLedgerManager::new(root()?)?;
    match command {
        SubagentCommand::Plan(args) | SubagentCommand::AutoStart(args) => {
            let plan = manager.plan(SubagentPlanRequest {
                task: args.task,
                paths: args.path,
                read_only: args.read_only,
                max_lanes: args.max_lanes,
            })?;
            println!("{}", serde_json::to_string_pretty(&plan)?);
        }
        SubagentCommand::Dispatch(args) => {
            let record = manager.dispatch(SubagentDispatchRequest {
                task: args.task,
                paths: args.path,
                read_only: args.read_only,
                max_lanes: args.max_lanes,
                create_worktree: args.create_worktree,
            })?;
            println!("{}", serde_json::to_string_pretty(&record)?);
        }
        SubagentCommand::Reconcile => {
            let report = manager.reconcile()?;
            println!("{}", serde_json::to_string_pretty(&report)?);
            if report["status"].as_str().unwrap_or("blocked") != "passed" {
                bail!("subagent reconcile blocked");
            }
        }
        SubagentCommand::Record(args) => {
            let record = manager.record(SubagentRecordRequest {
                host_agent_id: args.agent_id,
                goal_id: args.goal_id,
                dispatch_request_id: args.dispatch_request_id,
                nickname: args.nickname,
                task: args.task,
                boundary: args.boundary,
                read_only: args.read_only,
                write_paths: args.write_path,
            })?;
            println!("{}", serde_json::to_string_pretty(&record)?);
        }
        SubagentCommand::Result(args) => {
            let record = manager.record_result(
                &args.id,
                SubagentResultRequest {
                    status: value_name(args.status),
                    summary: args.message,
                    evidence_refs: args.evidence,
                    changed_paths: args.changed_path,
                },
            )?;
            println!("{}", serde_json::to_string_pretty(&record)?);
        }
        SubagentCommand::Review(args) => {
            let record = manager.record_review(
                &args.id,
                SubagentReviewRequest {
                    verdict: value_name(args.verdict),
                    summary: args.message,
                    overlap_resolution: args.overlap_resolution,
                },
            )?;
            println!("{}", serde_json::to_string_pretty(&record)?);
        }
        SubagentCommand::Status => {
            println!("{}", serde_json::to_string_pretty(&manager.status()?)?);
        }
    }
    Ok(())
}

fn print_host_subagent_dispatch_request(report: &rayman_core::goal::GoalRunReport) -> Result<()> {
    if let Some(dispatch) = &report.subagent_dispatch {
        println!(
            "HOST_SUBAGENT_DISPATCH_REQUEST {}",
            serde_json::to_string(dispatch)?
        );
    }
    Ok(())
}

fn print_goal_record(record: &rayman_core::goal::GoalRecord) {
    println!("  状态: {}", record.status);
    println!("  当前阶段: {}", record.current_stage);
    println!("  下一步: {}", record.next_action);
    if let Some(reason) = &record.blocked_reason {
        println!("  阻塞原因: {reason}");
    }
    let must_total = record
        .contract
        .requirements
        .iter()
        .filter(|requirement| requirement.priority == "must")
        .count();
    let must_done = record
        .contract
        .requirements
        .iter()
        .filter(|requirement| requirement.priority == "must" && requirement.status == "satisfied")
        .count();
    println!("  must 完成: {must_done}/{must_total}");
    if !record.contract.clarification.default_choices.is_empty() {
        println!("  默认选项:");
        for choice in &record.contract.clarification.default_choices {
            println!("    - {}: {}", choice.title, choice.default_option);
        }
    }
    if !record.contract.verification.is_empty() {
        println!("  验证命令:");
        for command in &record.contract.verification {
            println!("    - {command}");
        }
    }
}

fn goal_run_options(
    until: GoalRunUntilArg,
    checkpoint_interval_minutes: u64,
    max_repair_attempts: u32,
) -> GoalRunOptions {
    GoalRunOptions {
        until: match until {
            GoalRunUntilArg::NextStep => GoalRunUntil::NextStep,
            GoalRunUntilArg::Blocked => GoalRunUntil::Blocked,
            GoalRunUntilArg::Summary => GoalRunUntil::Summary,
            GoalRunUntilArg::Complete => GoalRunUntil::Complete,
        },
        checkpoint_interval_minutes,
        max_repair_attempts,
    }
}

fn cmd_customer_deploy(command: CustomerDeployCommand) -> Result<()> {
    let manager = CustomerDeployManager::new(root()?)?;
    match command {
        CustomerDeployCommand::Status => {
            println!("{}", serde_json::to_string_pretty(&manager.status()?)?);
        }
        CustomerDeployCommand::Validate => {
            let report = manager.validate()?;
            println!("{}", serde_json::to_string_pretty(&report)?);
            if report.status != "ready" {
                bail!("客户发布配置未就绪: {}", report.missing_required.join(", "));
            }
        }
        CustomerDeployCommand::Unset { key } => {
            manager.unset(&key)?;
            println!("{}", serde_json::to_string_pretty(&manager.status()?)?);
        }
        CustomerDeployCommand::Set(args) => {
            manager.set(CustomerDeployUpdate {
                environment: args.environment,
                build_command: args.build_command,
                test_commands: args.test_command,
                deploy_command: args.deploy_command,
                artifact_paths: args.artifact_path,
                target_alias: args.target_alias,
                rollback_command: args.rollback_command,
                credential_refs: credential_refs(args.credential_env, args.credential_ref),
                notes: args.notes,
            })?;
            println!("客户发布配置已保存");
            println!("{}", serde_json::to_string_pretty(&manager.status()?)?);
        }
    }
    Ok(())
}

fn credential_refs(envs: Vec<String>, refs: Vec<String>) -> Vec<CredentialRef> {
    let mut credentials = Vec::new();
    credentials.extend(envs.into_iter().map(|env| CredentialRef {
        env: Some(env),
        credential_ref: None,
    }));
    credentials.extend(refs.into_iter().map(|credential_ref| CredentialRef {
        env: None,
        credential_ref: Some(credential_ref),
    }));
    credentials
}

fn cmd_coverage(command: CoverageCommand) -> Result<()> {
    match command {
        CoverageCommand::Status(args) => {
            let workspace_root = root()?;
            let output = args
                .output
                .map(|output| {
                    ensure_within(
                        &output,
                        &workspace_root,
                        "coverage output escaped workspace",
                    )
                })
                .transpose()?;
            let canonical_markdown_output = match (&args.format, output.as_deref()) {
                (CoverageOutputFormat::Markdown, Some(output)) => {
                    is_canonical_feature_coverage_output(&workspace_root, output)?
                }
                _ => false,
            };
            let report = feature_coverage::check_feature_coverage_with_options(
                &workspace_root,
                feature_coverage::FeatureCoverageOptions {
                    strict: args.strict || args.check || canonical_markdown_output,
                },
            )?;
            let text = match args.format {
                CoverageOutputFormat::Text => render_feature_coverage_text_summary(&report),
                CoverageOutputFormat::Json => serde_json::to_string_pretty(&report)?,
                CoverageOutputFormat::Markdown => render_feature_coverage_markdown(&report),
            };
            if let Some(output) = output {
                if args.check {
                    let current = output
                        .exists()
                        .then(|| fs::read_to_string(&output))
                        .transpose()?;
                    if current.as_deref() != Some(text.as_str()) {
                        bail!(
                            "feature coverage markdown is stale: {}",
                            display_path(&output)
                        );
                    }
                } else {
                    if let Some(parent) = output.parent() {
                        fs::create_dir_all(parent)?;
                    }
                    fs::write(&output, text)?;
                    println!("feature coverage written: {}", display_path(&output));
                }
            } else {
                println!("{text}");
            }
            if args.check && report.status != "passed" {
                bail!(
                    "feature coverage status={}: {}",
                    report.status,
                    feature_coverage::format_feature_coverage_findings(&report)
                );
            }
        }
    }
    Ok(())
}

fn render_feature_coverage_text_summary(
    report: &feature_coverage::FeatureCoverageReport,
) -> String {
    let mut lines = vec![
        format!("Feature coverage: {}", report.status),
        format!(
            "  features={} findings={} strict={}",
            report.feature_count, report.finding_count, report.strict
        ),
        format!(
            "  public_commands documented={} implemented={} registered={}",
            report.documented_public_commands.len(),
            report.implemented_public_commands.len(),
            report.registered_public_commands.len()
        ),
        format!(
            "  api_endpoints documented={} implemented={} registered={}",
            report.documented_api_endpoints.len(),
            report.implemented_api_endpoints.len(),
            report.registered_api_endpoints.len()
        ),
    ];
    if report.findings.is_empty() {
        lines.push("  findings: none".into());
    } else {
        lines.push("  findings:".into());
        for finding in report.findings.iter().take(20) {
            let path = finding
                .path
                .as_ref()
                .map(|path| display_path(path))
                .unwrap_or_else(|| "<manifest>".into());
            lines.push(format!(
                "    - {}:{} {}: {}",
                path, finding.line, finding.kind, finding.message
            ));
        }
        if report.findings.len() > 20 {
            lines.push(format!(
                "    - ... {} more findings omitted; rerun with --format json for full detail",
                report.findings.len() - 20
            ));
        }
    }
    for action in &report.required_actions {
        lines.push(format!("  action: {action}"));
    }
    lines.join("\n")
}

fn cmd_doctor(command: DoctorCommand) -> Result<()> {
    match command {
        DoctorCommand::Shell => cmd_doctor_shell(),
    }
}

fn cmd_doctor_shell() -> Result<()> {
    println!("Rayman shell doctor:");
    println!("  workspace: {}", display_path(&root()?));
    println!(
        "  rayman_exe: {}",
        std::env::current_exe()
            .map(|path| display_path(&path))
            .unwrap_or_else(|_| "<unknown>".into())
    );
    println!(
        "  recommendation: prefer target\\release\\rayman.exe or a NoProfile shell when PowerShell profile output pollutes CLI results"
    );
    for candidate in ["pwsh", "powershell"] {
        let report = probe_powershell(candidate);
        println!(
            "  {}: status={} profile_load={} no_profile={} detail={}",
            candidate, report.status, report.profile_load, report.no_profile, report.detail
        );
    }
    Ok(())
}

struct ShellProbeReport {
    status: String,
    profile_load: String,
    no_profile: String,
    detail: String,
}

fn probe_powershell(exe: &str) -> ShellProbeReport {
    const PROFILE_MARKER: &str = "RAYMAN_PROFILE_PROBE";
    const NO_PROFILE_MARKER: &str = "RAYMAN_NO_PROFILE_PROBE";
    let profile = run_shell_probe(
        exe,
        &[
            "-NoLogo",
            "-NonInteractive",
            "-Command",
            "Write-Output RAYMAN_PROFILE_PROBE",
        ],
    );
    let no_profile = run_shell_probe(
        exe,
        &[
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "Write-Output RAYMAN_NO_PROFILE_PROBE",
        ],
    );
    let profile_noise = probe_noise_detail(&profile, PROFILE_MARKER);
    let no_profile_noise = probe_noise_detail(&no_profile, NO_PROFILE_MARKER);
    let profile_clean = profile.ok && profile_noise.is_none();
    let no_profile_clean = no_profile.ok && no_profile_noise.is_none();
    let status = if profile_clean && no_profile_clean {
        "clean"
    } else if no_profile_clean {
        "profile_noise"
    } else {
        "unavailable"
    }
    .to_string();
    let detail = if let Some(noise) = profile_noise {
        noise
    } else if !profile.error.is_empty() {
        profile.error.clone()
    } else if let Some(noise) = no_profile_noise {
        noise
    } else if !no_profile.error.is_empty() {
        no_profile.error.clone()
    } else {
        "probe completed".into()
    };
    ShellProbeReport {
        status,
        profile_load: probe_state(&profile, PROFILE_MARKER),
        no_profile: probe_state(&no_profile, NO_PROFILE_MARKER),
        detail,
    }
}

struct ShellProbe {
    ok: bool,
    timed_out: bool,
    stdout: String,
    stderr: String,
    error: String,
}

fn run_shell_probe(exe: &str, args: &[&str]) -> ShellProbe {
    let mut child = match ProcessCommand::new(exe)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            return ShellProbe {
                ok: false,
                timed_out: false,
                stdout: String::new(),
                stderr: String::new(),
                error: error.to_string(),
            };
        }
    };
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() < Duration::from_secs(3) => {
                thread::sleep(Duration::from_millis(50));
            }
            Ok(None) => {
                let _ = child.kill();
                let output = child.wait_with_output().ok();
                return ShellProbe {
                    ok: false,
                    timed_out: true,
                    stdout: output
                        .as_ref()
                        .map(|output| String::from_utf8_lossy(&output.stdout).to_string())
                        .unwrap_or_default(),
                    stderr: output
                        .as_ref()
                        .map(|output| String::from_utf8_lossy(&output.stderr).to_string())
                        .unwrap_or_default(),
                    error: "probe timed out after 3 seconds".into(),
                };
            }
            Err(error) => {
                return ShellProbe {
                    ok: false,
                    timed_out: false,
                    stdout: String::new(),
                    stderr: String::new(),
                    error: error.to_string(),
                };
            }
        }
    }
    match child.wait_with_output() {
        Ok(output) => ShellProbe {
            ok: output.status.success(),
            timed_out: false,
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            error: if output.status.success() {
                String::new()
            } else {
                format!("exit_code={:?}", output.status.code())
            },
        },
        Err(error) => ShellProbe {
            ok: false,
            timed_out: false,
            stdout: String::new(),
            stderr: String::new(),
            error: error.to_string(),
        },
    }
}

fn probe_state(probe: &ShellProbe, expected_stdout_marker: &str) -> String {
    if probe.timed_out {
        "timeout".into()
    } else if !probe.ok {
        "failed".into()
    } else if !probe.stderr.trim().is_empty() {
        "stderr".into()
    } else if stdout_noise_line(&probe.stdout, expected_stdout_marker).is_some() {
        "stdout".into()
    } else {
        "clean".into()
    }
}

fn probe_noise_detail(probe: &ShellProbe, expected_stdout_marker: &str) -> Option<String> {
    if !probe.stderr.trim().is_empty() {
        Some(first_line(&probe.stderr))
    } else {
        stdout_noise_line(&probe.stdout, expected_stdout_marker)
    }
}

fn stdout_noise_line(stdout: &str, expected_marker: &str) -> Option<String> {
    let mut saw_marker = false;
    for line in stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        if line == expected_marker {
            saw_marker = true;
        } else {
            return Some(line.to_string());
        }
    }
    if saw_marker {
        None
    } else {
        Some(format!("missing expected stdout marker {expected_marker}"))
    }
}

fn first_line(text: &str) -> String {
    text.lines()
        .find(|line| !line.trim().is_empty())
        .map(|line| line.trim().to_string())
        .unwrap_or_else(|| "<empty stderr>".into())
}

fn is_canonical_feature_coverage_output(workspace_root: &Path, output: &Path) -> Result<bool> {
    let canonical = ensure_within(
        Path::new(feature_coverage::FEATURE_COVERAGE_MARKDOWN),
        workspace_root,
        "canonical feature coverage path escaped workspace",
    )?;
    Ok(output == canonical)
}

fn cmd_stats() -> Result<()> {
    let workspace_root = root()?;
    let usage = AuxiliaryUsageStore::new(&workspace_root)?.report_without_round()?;
    let report = AuxiliaryContributionStore::new(&workspace_root)?.report_without_round()?;
    let goal_stats = GoalManager::new(&workspace_root)?.stats()?;
    let research_stats = ResearchManager::new(&workspace_root)?.stats()?;
    let quality_stats = QualityManager::new(&workspace_root)?.stats()?;
    println!("目标自治统计:");
    println!(
        "  active={} completed={} blocked={} failed={} partial={} total={}",
        goal_stats["active"].as_u64().unwrap_or(0),
        goal_stats["completed"].as_u64().unwrap_or(0),
        goal_stats["blocked"].as_u64().unwrap_or(0),
        goal_stats["failed"].as_u64().unwrap_or(0),
        goal_stats["partial"].as_u64().unwrap_or(0),
        goal_stats["total"].as_u64().unwrap_or(0)
    );
    println!("{AUXILIARY_USAGE_VALUE_LABEL}:");
    println!(
        "  说明: planning/workflow_summary/research 等建议或分析类调用计入使用价值；不计入实现纠错贡献。"
    );
    println!(
        "  项目累计(辅助成功/辅助调用/主力AI调用): {}",
        format_usage_scope(usage.get("project_total"))
    );
    for line in usage_detail_lines(usage.get("project_total")) {
        println!("    {line}");
    }
    if let Some(by_task) = usage.get("by_task").and_then(JsonValue::as_object)
        && !by_task.is_empty()
    {
        println!("  按任务:");
        for (task, scope) in by_task {
            println!("    {task}: {}", format_usage_scope(Some(scope)));
            for line in usage_detail_lines(Some(scope)) {
                println!("      {line}");
            }
        }
    }
    if let Some(by_provider) = usage.get("by_provider").and_then(JsonValue::as_object)
        && !by_provider.is_empty()
    {
        println!("  按Provider:");
        for (provider, scope) in by_provider {
            println!("    {provider}: {}", format_usage_scope(Some(scope)));
            for line in usage_detail_lines(Some(scope)) {
                println!("      {line}");
            }
        }
    }
    if let Some(path) = usage.get("state_path").and_then(JsonValue::as_str) {
        println!("  使用状态文件: {path}");
    }
    println!("{AUXILIARY_CONTRIBUTION_LABEL}:");
    println!(
        "  项目累计: {}",
        format_contribution_scope(report.get("project_total"))
    );
    println!(
        "  说明: 仅 implementation_validation 实际修正主力AI输出时计入；0 样本表示尚无实现验证纠错回合，不表示辅助AI无价值。"
    );
    println!("  本轮: 在 generate/API generate 的辅助AI输出中显示");
    if let Some(events) = report.get("events").and_then(JsonValue::as_array)
        && !events.is_empty()
    {
        println!("  最近贡献证据:");
        for event in events.iter().rev().take(5) {
            let task = event
                .get("task")
                .and_then(JsonValue::as_str)
                .unwrap_or("<unknown>");
            let counted = event
                .get("counted")
                .and_then(JsonValue::as_bool)
                .unwrap_or(false);
            let reason = event
                .get("reason")
                .and_then(JsonValue::as_str)
                .unwrap_or("no reason recorded");
            println!("    {task}: counted={counted} reason={reason}");
            if let Some(evidence) = event.get("evidence").and_then(JsonValue::as_array) {
                for item in evidence.iter().filter_map(JsonValue::as_str).take(3) {
                    println!("      - {item}");
                }
            }
        }
    }
    if let Some(path) = report.get("state_path").and_then(JsonValue::as_str) {
        println!("  贡献状态文件: {path}");
    }
    println!("Research Agent 统计:");
    println!(
        "  sessions={} active={} reconciled={} conflicts={} blocked={} policy_violations={} experiments={} unresolved_conflicts={}",
        research_stats.total_sessions,
        research_stats.active_sessions,
        research_stats.reconciled_sessions,
        research_stats.conflicted_sessions,
        research_stats.blocked_sessions,
        research_stats.policy_violations,
        research_stats.experiments,
        research_stats.unresolved_conflicts
    );
    println!("质量模式统计:");
    println!(
        "  incidents={} patterns={} workspace_patterns={}",
        quality_stats["incident_count"].as_u64().unwrap_or(0),
        quality_stats["pattern_count"].as_u64().unwrap_or(0),
        quality_stats["workspace_pattern_count"]
            .as_u64()
            .unwrap_or(0)
    );
    if let Some(patterns) = quality_stats.get("patterns").and_then(JsonValue::as_array)
        && !patterns.is_empty()
    {
        println!("  模式命中:");
        for pattern in patterns {
            println!(
                "    {}: incidents={} hits={}",
                pattern
                    .get("id")
                    .and_then(JsonValue::as_str)
                    .unwrap_or("<unknown>"),
                pattern
                    .get("incident_count")
                    .and_then(JsonValue::as_u64)
                    .unwrap_or(0),
                pattern
                    .get("hit_count")
                    .and_then(JsonValue::as_u64)
                    .unwrap_or(0)
            );
        }
    }
    if let Some(path) = quality_stats.get("state_path").and_then(JsonValue::as_str) {
        println!("  质量模式文件: {path}");
    }
    Ok(())
}

fn cmd_audit() -> Result<()> {
    audit::assert_repository_clean(&root()?)?;
    instruction::assert_stale_instructions_released(&root()?)?;
    println!("仓库审计通过");
    Ok(())
}

fn manager(args: &ModelArgs) -> Result<AgentManager> {
    let mut manager = AgentManager::new(
        root()?,
        args.model_type.clone(),
        args.model_name.clone(),
        args.route_mode.clone(),
        args.no_fallback,
    )?;
    manager.task_override = args.task.clone();
    Ok(manager)
}

fn research_run_agent(root: PathBuf, args: &ResearchRunArgs) -> Result<Option<AgentManager>> {
    let explicit_route_requested = args.model_type.is_some()
        || args.model_name.is_some()
        || args.route_mode.is_some()
        || args.no_fallback;
    match AgentManager::new(
        root,
        args.model_type.clone(),
        args.model_name.clone(),
        args.route_mode.clone(),
        args.no_fallback,
    ) {
        Ok(agent) => Ok(Some(agent)),
        Err(_) if !explicit_route_requested => Ok(None),
        Err(error) => Err(error),
    }
}

fn explicit_model_override(
    model_type: &Option<String>,
    model_name: &Option<String>,
) -> Result<Option<ModelRef>> {
    match (model_type, model_name) {
        (Some(provider), Some(model)) => Ok(Some(ModelRef {
            provider: provider.clone(),
            model: model.clone(),
        })),
        (None, None) => Ok(None),
        _ => bail!("--model-type and --model-name must be provided together"),
    }
}

fn model_updates_enabled(config: &ConfigManager) -> bool {
    model_update_auto(config)
        .and_then(|value| rayman_core::config::mapping_get(value, "enabled"))
        .and_then(yaml::Value::as_bool)
        .unwrap_or(false)
}

fn model_updates_interval_days(config: &ConfigManager) -> i64 {
    model_update_auto(config)
        .and_then(|value| rayman_core::config::mapping_get(value, "interval_days"))
        .and_then(yaml::Value::as_i64)
        .unwrap_or(0)
}

fn model_updates_last_update_missing(config: &ConfigManager) -> bool {
    config
        .referenced
        .get("model_updates")
        .and_then(|value| rayman_core::config::mapping_get(value, "last_update"))
        .map(yaml::Value::is_null)
        .unwrap_or(true)
}

fn model_update_auto(config: &ConfigManager) -> Option<&yaml::Value> {
    config
        .referenced
        .get("model_updates")
        .and_then(|value| rayman_core::config::mapping_get(value, "auto_update"))
}

fn maybe_auto_compile(language: &str, output_path: Option<&Path>, disabled: bool) -> Result<()> {
    if disabled {
        println!("自动编译: skipped: disabled");
        return Ok(());
    }
    let result = auto_compile_generated(root()?, language, output_path)?;
    print_auto_compile_result(&result);
    if result.failed() {
        bail!("自动编译失败: {}", compile_result_summary(&result));
    }
    Ok(())
}

fn print_auto_compile_result(result: &AutoCompileResult) {
    println!("自动编译: {}", compile_result_summary(result));
    if result.status == "compiled"
        && let Some(executable) = &result.executable_path
    {
        println!("可运行文件: {executable}");
    }
}

fn emit_workflow_report(report: &workflow::ExecutionReport, path: Option<&PathBuf>) -> Result<()> {
    let text = serde_json::to_string_pretty(report)?;
    if let Some(path) = path {
        fs::write(path, &text)?;
        println!("目标导向执行报告已保存到: {}", path.display());
    }
    println!("目标导向工作流报告:");
    println!("{}", report.summary);
    if let Some(auxiliary) = report.artifacts.get("auxiliary_ai") {
        print_auxiliary_value(auxiliary);
    }
    if report.status != "success" {
        bail!("目标导向工作流未通过: {}", report.summary);
    }
    Ok(())
}

fn enforce_review_gate(code: Option<&str>, reviewed_path: Option<&Path>) -> Result<()> {
    let manager = SessionManager::new(root()?)?;
    let blockers = manager.review_blockers(code, reviewed_path)?;
    if blockers.is_empty() {
        return Ok(());
    }
    bail!(
        "代码审查被待完成/未完成标记阻断:\n{}",
        format_review_blockers(&blockers)
    );
}

fn format_review_blockers(blockers: &[JsonValue]) -> String {
    blockers
        .iter()
        .map(|blocker| {
            let kind = blocker
                .get("type")
                .and_then(JsonValue::as_str)
                .unwrap_or("blocker");
            if kind == "pending_work" {
                return format!(
                    "- pending {} [{}]: {}",
                    blocker
                        .get("id")
                        .and_then(JsonValue::as_str)
                        .unwrap_or("unknown"),
                    blocker
                        .get("status")
                        .and_then(JsonValue::as_str)
                        .unwrap_or("unknown"),
                    blocker
                        .get("title")
                        .and_then(JsonValue::as_str)
                        .unwrap_or("")
                );
            }
            if kind == "subagent_ledger" {
                return format!(
                    "- subagent ledger: {}",
                    blocker
                        .get("reason")
                        .and_then(JsonValue::as_str)
                        .unwrap_or("unresolved subagent ledger blocker")
                );
            }
            let path = blocker
                .get("path")
                .and_then(JsonValue::as_str)
                .unwrap_or("<current>");
            let line = blocker.get("line").and_then(JsonValue::as_u64).unwrap_or(0);
            let reason = blocker
                .get("reason")
                .and_then(JsonValue::as_str)
                .unwrap_or("unfinished");
            let snippet = blocker
                .get("snippet")
                .and_then(JsonValue::as_str)
                .unwrap_or("");
            format!("- {path}:{line} {reason} - {snippet}")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn print_agent_results(title: &str, results: &[rayman_core::agent_skill::AgentSkillResult]) {
    println!("{title}");
    for result in results {
        println!(
            "  {} {}: {} ({})",
            if result.ok() { "✓" } else { "✗" },
            result.agent,
            display_path(&result.path),
            result
                .sha256
                .as_deref()
                .map(|hash| &hash[..hash.len().min(12)])
                .unwrap_or("未安装")
        );
        if let Some(error) = &result.error {
            println!("    error: {error}");
        }
    }
}

fn value_name<T: ValueEnum>(value: T) -> String {
    let Some(possible) = value.to_possible_value() else {
        return "unknown".to_string();
    };
    possible.get_name().replace('-', "_")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_model_override_requires_provider_and_model() {
        assert!(explicit_model_override(&Some("openai".into()), &None).is_err());
        assert!(explicit_model_override(&None, &Some("gpt-4o".into())).is_err());
        assert!(explicit_model_override(&Some("openai".into()), &Some("gpt-4o".into())).is_ok());
    }

    #[test]
    fn research_run_agent_rejects_partial_model_override() {
        let temp = tempfile::tempdir().unwrap();
        let args = ResearchRunArgs {
            id: Some("research_123".into()),
            model_type: Some("openai".into()),
            model_name: None,
            route_mode: None,
            no_fallback: false,
        };

        let error = research_run_agent(temp.path().to_path_buf(), &args)
            .unwrap_err()
            .to_string();

        assert!(error.contains("model_type and model_name must be provided together"));
    }

    #[test]
    fn model_commands_get_contribution_footer() {
        let cli = Cli::try_parse_from(["rayman", "session", "status"]).unwrap();
        assert!(!cli.show_stats);
        assert!(!command_prints_auxiliary_stats(&cli.command));
    }

    #[test]
    fn model_backed_commands_avoid_duplicate_contribution_footer() {
        let cli = Cli::try_parse_from(["rayman", "stats"]).unwrap();
        assert!(command_prints_auxiliary_stats(&cli.command));
    }

    #[test]
    fn global_show_stats_is_required_for_default_footer() {
        let cli = Cli::try_parse_from(["rayman", "--show-stats", "session", "status"]).unwrap();
        assert!(cli.show_stats);
        assert!(!command_prints_auxiliary_stats(&cli.command));
    }

    #[test]
    fn coverage_text_summary_is_compact_and_human_readable() {
        let report = feature_coverage::FeatureCoverageReport {
            workspace_path: PathBuf::from("workspace"),
            generated_at: "now".into(),
            manifest_path: PathBuf::from("config/feature_coverage.yaml"),
            status: "passed".into(),
            strict: true,
            feature_count: 2,
            finding_count: 0,
            findings: Vec::new(),
            covered_document_paths: Vec::new(),
            expected_document_paths: Vec::new(),
            documented_public_commands: vec!["rayman coverage status".into()],
            implemented_public_commands: vec!["rayman coverage".into()],
            registered_public_commands: vec!["rayman coverage status".into()],
            documented_api_endpoints: Vec::new(),
            implemented_api_endpoints: Vec::new(),
            registered_api_endpoints: Vec::new(),
            required_actions: vec!["Feature coverage matrix is current.".into()],
            features: Vec::new(),
        };

        let text = render_feature_coverage_text_summary(&report);

        assert!(text.contains("Feature coverage: passed"));
        assert!(text.contains("features=2 findings=0 strict=true"));
        assert!(text.contains("findings: none"));
        assert!(!text.contains("\"features\""));
    }

    #[test]
    fn shell_probe_state_names_profile_noise_and_timeouts() {
        assert_eq!(
            probe_state(
                &ShellProbe {
                    ok: true,
                    timed_out: false,
                    stdout: "RAYMAN_PROFILE_PROBE\n".into(),
                    stderr: String::new(),
                    error: String::new(),
                },
                "RAYMAN_PROFILE_PROBE"
            ),
            "clean"
        );
        assert_eq!(
            probe_state(
                &ShellProbe {
                    ok: true,
                    timed_out: false,
                    stdout: "RAYMAN_PROFILE_PROBE\n".into(),
                    stderr: "profile error".into(),
                    error: String::new(),
                },
                "RAYMAN_PROFILE_PROBE"
            ),
            "stderr"
        );
        assert_eq!(
            probe_state(
                &ShellProbe {
                    ok: true,
                    timed_out: false,
                    stdout: "profile banner\nRAYMAN_PROFILE_PROBE\n".into(),
                    stderr: String::new(),
                    error: String::new(),
                },
                "RAYMAN_PROFILE_PROBE"
            ),
            "stdout"
        );
        assert_eq!(
            probe_state(
                &ShellProbe {
                    ok: true,
                    timed_out: false,
                    stdout: String::new(),
                    stderr: String::new(),
                    error: String::new(),
                },
                "RAYMAN_PROFILE_PROBE"
            ),
            "stdout"
        );
        assert_eq!(
            probe_state(
                &ShellProbe {
                    ok: false,
                    timed_out: true,
                    stdout: String::new(),
                    stderr: String::new(),
                    error: String::new(),
                },
                "RAYMAN_PROFILE_PROBE"
            ),
            "timeout"
        );
    }

    #[test]
    fn shell_probe_noise_detail_reports_stdout_profile_noise() {
        assert_eq!(
            probe_noise_detail(
                &ShellProbe {
                    ok: true,
                    timed_out: false,
                    stdout: "profile banner\nRAYMAN_PROFILE_PROBE\n".into(),
                    stderr: String::new(),
                    error: String::new(),
                },
                "RAYMAN_PROFILE_PROBE"
            ),
            Some("profile banner".into())
        );
        assert_eq!(
            probe_noise_detail(
                &ShellProbe {
                    ok: true,
                    timed_out: false,
                    stdout: "RAYMAN_PROFILE_PROBE\n".into(),
                    stderr: String::new(),
                    error: String::new(),
                },
                "RAYMAN_PROFILE_PROBE"
            ),
            None
        );
    }

    #[test]
    fn auxiliary_cli_format_reports_skip_reason_and_error() {
        let usage = serde_json::json!({
            "enabled": true,
            "model": "aux/auto",
            "status": "skipped_task_disabled",
            "skip_reason": "task is not listed in auxiliary_ai.tasks",
            "error": "boom"
        });

        assert_eq!(
            auxiliary_status_line(&usage),
            "辅助AI: enabled=true model=aux/auto status=skipped_task_disabled"
        );
        assert_eq!(
            auxiliary_detail_lines(&usage),
            vec![
                "辅助AI跳过原因: task is not listed in auxiliary_ai.tasks".to_string(),
                "辅助AI错误: boom".to_string()
            ]
        );
    }

    #[test]
    fn usage_scope_format_uses_success_call_main_ai_ratio() {
        let usage = serde_json::json!({
            "success_count": 3,
            "attempt_count": 6,
            "call_count": 4,
            "main_ai_count": 5,
            "failed_count": 1,
            "skipped_count": 1,
            "queued_count": 1,
            "auxiliary_call_success_rate": 75.0
        });

        assert_eq!(
            format_usage_scope(Some(&usage)),
            "3/4/5 (75.0%, records=6, failed=1, skipped=1, queued=1, avg_ms=0.0, provider_attempts=0)"
        );
    }

    #[test]
    fn contribution_scope_format_names_missing_validation_sample() {
        let contribution = serde_json::json!({
            "production_count": 0,
            "contribution_count": 0,
            "contribution_percentage": 0.0
        });

        assert_eq!(
            format_contribution_scope(Some(&contribution)),
            "暂无实现验证纠错样本"
        );
    }

    #[test]
    fn contribution_scope_format_reports_real_validation_corrections() {
        let contribution = serde_json::json!({
            "production_count": 4,
            "contribution_count": 1,
            "contribution_percentage": 25.0
        });

        assert_eq!(
            format_contribution_scope(Some(&contribution)),
            "1/4 (25.0%)"
        );
    }
}
