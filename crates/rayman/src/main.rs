#[macro_use]
mod i18n;
mod checkpoint_cli;
mod cli;
mod codex_hook_cli;
mod doctor;
mod goal_cli;
mod state_audit_cli;
mod task_workflow;

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command as ProcessCommand;

use anyhow::{Context, Result, bail};
use clap::Parser;
use serde_json::json;
use sha2::{Digest, Sha256};

use cli::{
    AutosaveAction, AutosaveCmd, CheckCmd, CheckProfile, CheckpointAction, CheckpointCmd, Cli,
    Command, ContextAction, ContextCmd, Format, GoalAction, GoalCmd, HandoffAction, MapAction,
    MapCmd, PendingAction, QualityProfile, StateAction, StateCmd, TempAction, TempCmd,
    WorkspaceAction, WorkspaceCmd,
};
use rayman::{assets, autosave, context, goal, map, source_state, temp, workspace, workspace_root};

fn main() {
    i18n::preconfigure_from_process_args();
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => {
            let use_stdout = matches!(
                error.kind(),
                clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
            );
            let exit_code = error.exit_code();
            let rendered = i18n::localize_text(error.to_string());
            if use_stdout {
                println!("{rendered}");
            } else {
                eprintln!("{rendered}");
            }
            std::process::exit(exit_code);
        }
    };
    let json = i18n::configure(cli.language, matches!(cli.format, Format::Json));
    if let Err(error) = run(cli) {
        if json {
            // `error.to_string()` is only the outermost context, so JSON callers
            // used to lose the underlying cause (for example the `os error 5`
            // behind a checkpoint lock failure) that the text path prints via
            // `{:#}`. Emit the same full chain, plus the causes as structured
            // data, so a machine reader is never told strictly less.
            let causes = error.chain().map(ToString::to_string).collect::<Vec<_>>();
            let payload = json!({
                "error": format!("{error:#}"),
                "causes": causes,
            });
            match serde_json::to_string_pretty(&payload) {
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
    if let Command::CodexHook(command) = &cli.command {
        return codex_hook_cli::run(json, command);
    }
    let root = workspace_root()?;
    if !matches!(
        &cli.command,
        Command::Workspace(_)
            | Command::Assets
            | Command::State(_)
            | Command::Doctor(_)
            | Command::LegacyAudit(_)
            | Command::LegacyWorkspaceSkill(_)
            | Command::LegacySubagent(_)
            | Command::Checkpoint(CheckpointCmd {
                action: CheckpointAction::SalvageSave
                    | CheckpointAction::List
                    | CheckpointAction::Status
                    | CheckpointAction::Verify { .. },
                ..
            })
            | Command::Context(ContextCmd {
                action: ContextAction::LegacyOs { .. } | ContextAction::LegacyTask { .. }
            })
            // tick/status/stop 不能走顶层门禁：tick 由计划任务触发，激活一破
            // （升级 CLI、SKILL.md 变动）就会在失败落盘之前 exit 1，autosave
            // 的"每次结果必须持久化"契约失效，status 无法暴露死亡、stop 无法
            // 注销任务。激活检查在 autosave 内部对目标工作区执行并记账。
            // start 仍要求激活。
            | Command::Autosave(AutosaveCmd {
                action: AutosaveAction::Tick { .. }
                    | AutosaveAction::Status
                    | AutosaveAction::Stop { .. },
                ..
            })
    ) {
        workspace::require_active(&root)?;
    }

    match cli.command {
        Command::Workspace(cmd) => run_workspace(&root, json, cmd)?,

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
            ContextAction::LegacyOs { args } => {
                let suffix = if args.is_empty() {
                    String::new()
                } else {
                    format!(" {}", args.join(" "))
                };
                bail!(
                    "`rayman context os{suffix}` 已退役；使用 `rayman context refresh` 更新内容索引，使用 `rayman check --goal <id>` 验证任务"
                );
            }
            ContextAction::LegacyTask { args } => {
                let suffix = if args.is_empty() {
                    String::new()
                } else {
                    format!(" {}", args.join(" "))
                };
                bail!(
                    "`rayman context task{suffix}` 已退役；使用 `rayman prepare --goal <id>` 或 `rayman goal show <id>`"
                );
            }
        },

        Command::Goal(GoalCmd { action }) => run_goal(&root, json, action)?,
        Command::Prepare(cmd) => return task_workflow::run_prepare(&root, json, cmd),

        Command::Finish(cmd) => {
            task_workflow::require_stable_authority(&root, &cmd.goal)?;
            return run_check(
                &root,
                json,
                CheckCmd {
                    profile: cmd.profile,
                    goal: Some(cmd.goal),
                    require_current_goal: true,
                    refresh_context: true,
                },
            );
        }

        Command::Assets => {
            let report = assets::scan(&root)?;
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
                        "entry_count": status.entry_count,
                        "file_count": status.file_count,
                        "directory_count": status.directory_count,
                        "total_bytes": status.total_bytes,
                        "traversal_error_count": status.traversal_error_count,
                        "traversal_errors": status.traversal_errors,
                    }));
                } else {
                    println!(
                        "托管临时目录: {} (exists={}, entries={}, files={}, dirs={}, {:.1} MB, traversal_errors={})",
                        status.root,
                        status.exists,
                        status.entry_count,
                        status.file_count,
                        status.directory_count,
                        status.total_bytes as f64 / 1_048_576.0,
                        status.traversal_error_count,
                    );
                    for error in status.traversal_errors {
                        println!("  error: {error}");
                    }
                }
            }
            TempAction::Scratch { label } => {
                let dir = temp::scratch_dir(&root, &label)?;
                // This path is meant to be pasted into another command, so it
                // must not carry the Windows `\\?\` verbatim prefix that many
                // tools and shells refuse. Pytest lease paths were normalized
                // for exactly this reason; scratch was the remaining leak.
                let path = rayman::pathfmt::display_path(&dir);
                if json {
                    print(&json!({ "path": path }));
                } else {
                    println!("{path}");
                }
            }
            TempAction::PytestLease { label } => {
                let lease = temp::create_pytest_lease(&root, &label)?;
                if json {
                    print(&serde_json::to_value(&lease)?);
                } else {
                    println!("pytest lease {} 已创建并通过读写探针", lease.id);
                    println!("  root: {}", lease.root);
                    println!("  pytest args: {}", lease.pytest_args.join(" "));
                }
            }
            TempAction::PytestProbe { id } => {
                let lease = temp::verify_pytest_lease(&root, &id)?;
                if json {
                    print(&serde_json::to_value(&lease)?);
                } else {
                    println!("pytest lease {} 探针通过", lease.id);
                }
            }
            TempAction::PytestRelease { id } => {
                let removed = temp::release_pytest_lease(&root, &id)?;
                if json {
                    print(&json!({ "id": id, "removed": removed }));
                } else {
                    println!("pytest lease {id} 已释放");
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

        Command::State(StateCmd { action }) => match action {
            StateAction::Audit { check } => run_state_audit(&root, json, check)?,
        },

        Command::Check(cmd) => return run_check(&root, json, cmd),

        Command::Map(cmd) => return run_map(&root, json, cmd),

        Command::Checkpoint(cmd) => return checkpoint_cli::run_checkpoint(&root, json, cmd),

        Command::Autosave(cmd) => return run_autosave(&root, json, cmd),

        Command::Doctor(cmd) => return doctor::run(&root, json, cmd),
        Command::CodexHook(_) => unreachable!(),
        Command::LegacyAudit(_) => bail!(
            "`rayman audit` 已退役；工作区门禁使用 `rayman check --profile standard`，任务交付使用 `rayman finish --goal <id>`，状态卫生使用 `rayman state audit --check`"
        ),
        Command::LegacyWorkspaceSkill(_) => bail!(
            "`rayman workspace-skill` 已退役；使用 `rayman workspace status|inspect|activate|rebind|deactivate`"
        ),
        Command::LegacySubagent(_) => bail!(
            "`rayman subagent` 已退役且 v2 不维护 agent ledger；需要保留未完成工作时使用 `rayman goal pending add`"
        ),
    }
    Ok(())
}

fn run_workspace(root: &Path, json: bool, cmd: WorkspaceCmd) -> Result<()> {
    let report = match cmd.action {
        WorkspaceAction::Status => workspace::activation_status(root)?,
        WorkspaceAction::Inspect => {
            let activation = workspace::activation_status(root)?;
            let source = source_state::inspect(root);
            let state_write = rayman::state_paths::state_write_probe(root);
            let host_patch = rayman::codex_host::patch_probe(None);
            if json {
                print(&json!({
                    "activation": activation,
                    "source": source,
                    "state_write": state_write,
                    "host_patch": host_patch,
                }));
            } else {
                println!(
                    "RaymanCodingSkill workspace activation: {} (active={}, config_present={})",
                    activation.status, activation.active, activation.config_present
                );
                print_source_state(&source);
                print_state_write_probe(&state_write);
                print_host_patch_probe(&host_patch);
            }
            return Ok(());
        }
        WorkspaceAction::Activate { skill_file, yes } => {
            if !yes {
                bail!("activation writes a hash-bound workspace_skill.yaml; add --yes to confirm");
            }
            workspace::activate(root, &skill_file.unwrap_or_else(|| root.join("SKILL.md")))?
        }
        WorkspaceAction::Rebind { yes } => {
            if !yes {
                bail!("rebind rewrites workspace_skill.yaml; add --yes to confirm");
            }
            let report = workspace::rebind(root)?;
            if json {
                print(&serde_json::to_value(&report)?);
            } else {
                print_workspace_activation(&report.activation);
                println!("  changed: {}", report.changed);
            }
            return Ok(());
        }
        WorkspaceAction::InstallBind { skill_file, yes } => {
            if !yes {
                bail!("install-bind updates workspace_skill.yaml; add --yes to confirm");
            }
            let report = workspace::install_bind(root, &skill_file)?;
            if json {
                print(&serde_json::to_value(&report)?);
            } else {
                print_workspace_activation(&report.activation);
                println!("  changed: {}", report.changed);
            }
            return Ok(());
        }
        WorkspaceAction::Deactivate { yes } => {
            if !yes {
                bail!("deactivation rewrites workspace_skill.yaml; add --yes to confirm");
            }
            workspace::deactivate(root)?
        }
    };
    if json {
        print(&serde_json::to_value(&report)?);
    } else {
        print_workspace_activation(&report);
        // Text only: `workspace status` JSON is the activation report itself and
        // callers parse that shape. The agent-facing surface is the text one.
        print_host_patch_probe(&rayman::codex_host::patch_probe(None));
    }
    Ok(())
}

fn print_workspace_activation(report: &workspace::WorkspaceActivationReport) {
    println!(
        "RaymanCodingSkill workspace activation: {} (active={}, config_present={})",
        report.status, report.active, report.config_present
    );
    for issue in &report.issues {
        println!("  issue: {issue}");
    }
    if let Some(command) = &report.recovery_command {
        println!("  recovery: `{command}`");
    }
}

fn print_source_state(source: &source_state::SourceState) {
    println!(
        "  source: kind={} available={} clean={} HEAD={} tracked_dirty={} untracked={} path_encoding_lossy={}",
        source.kind,
        source.available,
        source
            .clean
            .map(|value| value.to_string())
            .unwrap_or_else(|| "unknown".into()),
        source.head.as_deref().unwrap_or("unknown"),
        source.tracked_dirty,
        source.untracked,
        source.path_encoding_lossy
    );
    if let Some(error) = &source.error {
        println!("    source error: {error}");
    }
}

/// A structurally broken host patch tool is otherwise rediagnosed from scratch
/// after every context compaction, so every read-only status command repeats it.
pub(crate) fn print_host_patch_probe(probe: &rayman::codex_host::HostPatchProbe) {
    if probe.patch_tool_usable {
        return;
    }
    // The probe's own `reason`/`fix` strings stay stable English in JSON for
    // machine consumers; the human line is rendered from the typed catalog.
    println!(
        "{}",
        i18n::message(
            i18n::MessageId::HostPatchUnusable,
            &[probe
                .sandbox_mode
                .clone()
                .unwrap_or_else(|| "unknown".into())],
        )
    );
    println!("{}", i18n::message(i18n::MessageId::HostPatchFix, &[]));
}

/// Sandboxed hosts deny state writes with ACL errors that otherwise surface
/// mid-transaction; the probe line lets an agent escalate before starting one.
pub(crate) fn print_state_write_probe(probe: &rayman::state_paths::StateWriteProbe) {
    if probe.probed && probe.writable {
        match probe.error.as_deref() {
            None => println!("  状态写探针: 可写"),
            Some(error) => println!("  状态写探针: 可写；清理探针失败: {}", error),
        }
    } else if !probe.state_dir_present {
        println!("  状态写探针: 状态目录不存在，未探测");
    } else {
        println!(
            "  状态写探针: 写入被拒或探测失败（权限或 ACL）: {}",
            probe.error.as_deref().unwrap_or("")
        );
    }
}

fn run_state_audit(root: &Path, json: bool, check: bool) -> Result<()> {
    state_audit_cli::run_state_audit(root, json, check)
}

const SOURCE_FRESH_VERIFIER: &str = "scripts/verify-release-contract.ps1 -RequireSourceFresh";

fn run_map(root: &std::path::Path, json: bool, cmd: MapCmd) -> Result<()> {
    // Queries must remain read-only. Only the explicit `map refresh` action persists
    // a derived cache; every other map command builds an ephemeral current view.
    let project_map = if matches!(&cmd.action, MapAction::Refresh) {
        map::build(root)?
    } else {
        map::build_readonly(root)?
    };
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
        AutosaveAction::Status => autosave::status(root)?,
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

fn run_goal(root: &std::path::Path, json: bool, action: GoalAction) -> Result<()> {
    let store = goal::GoalStore::new(root);
    let pending = goal::PendingStore::new(root);
    match action {
        GoalAction::Start {
            title,
            must,
            must_proof,
            should,
        } => {
            let mut requirements = must
                .into_iter()
                .map(|text| goal::RequirementSpec {
                    text,
                    kind: goal::RequirementKind::Must,
                    proof_kind: None,
                })
                .collect::<Vec<_>>();
            for value in must_proof {
                let (kind, text) = value
                    .split_once("::")
                    .ok_or_else(|| anyhow::anyhow!("--must-proof must use KIND::TEXT"))?;
                if text.trim().is_empty() {
                    bail!("--must-proof text cannot be empty");
                }
                requirements.push(goal::RequirementSpec {
                    text: text.trim().to_string(),
                    kind: goal::RequirementKind::Must,
                    proof_kind: Some(kind.parse()?),
                });
            }
            requirements.extend(should.into_iter().map(|text| goal::RequirementSpec {
                text,
                kind: goal::RequirementKind::Should,
                proof_kind: None,
            }));
            let goal = store.start_with_specs(&title, &requirements)?;
            if json {
                print(&serde_json::to_value(&goal)?);
            } else {
                println!(
                    "{}",
                    i18n::message(
                        i18n::MessageId::GoalCreated,
                        &[goal.id, goal.requirements.len().to_string()],
                    )
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
                    println!(
                        "{}  [{}/{}]  {}",
                        goal.id, goal.lifecycle, goal.status, goal.title
                    );
                }
            }
        }
        GoalAction::Show { id } => goal_cli::run_show(&store, json, id)?,
        GoalAction::Summary { id } => goal_cli::run_summary(&store, json, id)?,
        GoalAction::Handoff(command) => match command.action {
            HandoffAction::Start { from_goal, commit } => {
                let goal = store.start_handoff(&from_goal, &commit)?;
                if json {
                    print(&serde_json::to_value(&goal)?);
                } else {
                    println!(
                        "{}",
                        i18n::message(
                            i18n::MessageId::HandoffCreated,
                            &[goal.id, from_goal, commit],
                        )
                    );
                }
            }
        },
        GoalAction::Plan {
            id,
            paths,
            check,
            extend,
        } => goal_cli::run_plan(root, &store, json, id, paths, check, extend)?,
        GoalAction::Review {
            id,
            reviewer,
            message,
        } => goal_cli::run_review(&store, json, id, reviewer, message)?,
        GoalAction::Package(command) => goal_cli::run_package(&store, json, *command)?,
        GoalAction::Lane(command) => goal_cli::run_lane(&store, json, *command)?,
        GoalAction::Progress {
            id,
            package,
            message,
            command,
        } => goal_cli::run_progress(root, &store, json, id, package, message, command)?,
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
        GoalAction::Validate {
            id,
            req,
            message,
            changed,
            non_code,
            command,
            authority,
            repeat,
        } => {
            if message.trim().is_empty() || command.trim().is_empty() {
                bail!("`--message` 与 `--command` 都不能为空");
            }
            if repeat == 0 || repeat > 10 {
                bail!("--repeat 必须在 1..=10 范围内");
            }
            if authority && repeat < 2 {
                bail!("--authority 要求 --repeat >= 2，以证明稳定固定点");
            }
            if !authority && repeat != 1 {
                bail!("重复执行只用于 authority gate；请同时传 --authority");
            }
            let impacts = impact_evidence_for_changed_paths(root, &changed)?;
            goal::validate_command_for_impacts(root, &command, &impacts, non_code)?;
            if authority {
                goal::validate_authority_command(root, &command)?;
            }
            let parsed = goal::parse_validation_command(&command)?;
            let contract_sha256 = store.validation_contract_hash(&id, &req)?;
            let before = goal::workspace_fingerprint(root)?;
            let (listed_tests, list_stdout_sha256, list_stderr_sha256) =
                if let Some(list_command) = goal::validation_list_command(&parsed)? {
                    let list_output = run_validation_command(root, &list_command)?;
                    if !list_output.status.success() {
                        bail!(
                            "独立 test list proof 失败（exit={}）；不会写入 receipt",
                            list_output.status.code().unwrap_or(-1)
                        );
                    }
                    (
                        Some(goal::listed_test_count(
                            &list_command,
                            &list_output.stdout,
                            &list_output.stderr,
                        )?),
                        Some(sha256_hex(&list_output.stdout)),
                        Some(sha256_hex(&list_output.stderr)),
                    )
                } else {
                    (None, None, None)
                };
            let mut stable_runs = Vec::new();
            let mut final_output = None;
            let mut final_test_proof = None;
            for run_index in 1..=repeat {
                let run_before = goal::workspace_fingerprint(root)?;
                if run_before != before {
                    bail!(
                        "authority validation 第 {run_index} 次运行前 workspace fingerprint 漂移；不会写入 receipt"
                    );
                }
                let output = run_validation_command(root, &parsed)?;
                let run_after = goal::workspace_fingerprint(root)?;
                if !output.status.success() {
                    bail!(
                        "验证命令第 {run_index}/{repeat} 次失败（exit={}）；不会写入 receipt。stdout_sha256={} stderr_sha256={}",
                        output.status.code().unwrap_or(-1),
                        sha256_hex(&output.stdout),
                        sha256_hex(&output.stderr)
                    );
                }
                let test_proof = goal::validation_execution_proof(
                    &parsed,
                    &output.stdout,
                    &output.stderr,
                    listed_tests,
                )?;
                if run_before != run_after || run_after != before {
                    bail!(
                        "验证命令第 {run_index}/{repeat} 次修改了工作区内容；不会写入 receipt。before={} after={}",
                        run_before,
                        run_after
                    );
                }
                stable_runs.push(goal::AuthorityRunReceipt {
                    exit_code: output.status.code().unwrap_or(0),
                    workspace_fingerprint_before: run_before,
                    workspace_fingerprint_after: run_after,
                    stdout_sha256: sha256_hex(&output.stdout),
                    stderr_sha256: sha256_hex(&output.stderr),
                });
                final_test_proof = test_proof;
                final_output = Some(output);
            }
            let output = final_output.expect("repeat is nonzero");
            let after = before.clone();
            let test_proof = final_test_proof;
            let impact_scopes = goal::validation_scopes_for_impacts(&impacts);
            let receipt = goal::ValidationReceipt {
                exit_code: output.status.code().unwrap_or(0),
                cwd: root.display().to_string(),
                workspace_fingerprint_before: before.clone(),
                workspace_fingerprint_after: after,
                stdout_sha256: sha256_hex(&output.stdout),
                stderr_sha256: sha256_hex(&output.stderr),
                invocation_sha256: goal::validation_invocation_sha256_scoped(
                    &command,
                    &impact_scopes,
                    non_code,
                ),
                passed_tests: test_proof.map(|proof| proof.passed),
                listed_tests: test_proof.map(|proof| proof.listed),
                ignored_tests: test_proof.map(|proof| proof.ignored),
                list_stdout_sha256,
                list_stderr_sha256,
                contract_sha256: contract_sha256.clone(),
            };
            let submission = goal::ValidationReceiptSubmission {
                evidence: message,
                command: command.clone(),
                receipt,
                impacts,
                non_code,
            };
            let goal = if authority {
                store.record_authority_validation_receipt(
                    &id,
                    &req,
                    goal::AuthorityReceiptSubmission {
                        authority: goal::AuthorityReceipt {
                            requirement_id: req.clone(),
                            command: command.clone(),
                            recorded_at: rayman::timefmt::now_iso(),
                            workspace_fingerprint: before,
                            repeat,
                            impact_scopes: impact_scopes.clone(),
                            non_code,
                            invocation_sha256: goal::authority_invocation_sha256(
                                &command,
                                &req,
                                repeat,
                                &impact_scopes,
                                non_code,
                            ),
                            contract_sha256: contract_sha256.clone(),
                            runs: stable_runs,
                        },
                        validation: submission,
                    },
                )?
            } else {
                store.record_validation_receipt(&id, &req, submission)?
            };
            if json {
                print(&serde_json::to_value(&goal)?);
            } else {
                println!("已执行并记录 {} 的可验证 receipt（目标 {}）", req, goal.id);
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
        GoalAction::Archive {
            id,
            reason,
            migrate_unreceipted,
            migrate_receipt_policy,
            quarantine_invalid_history,
        } => {
            let goal = if quarantine_invalid_history {
                store.quarantine_invalid_history(&id, &reason)?
            } else {
                store.archive_with_receipt_policy(
                    &id,
                    &reason,
                    migrate_unreceipted,
                    migrate_receipt_policy.as_deref(),
                )?
            };
            if json {
                print(&serde_json::to_value(&goal)?);
            } else {
                println!("目标 {} 已归档：{}", goal.id, reason.trim());
            }
        }
        GoalAction::AuthorizeReplacement {
            id,
            predecessors,
            authority_goal,
            command,
            maintenance_cycle_rebind,
            repeat,
        } => {
            if !(2..=10).contains(&repeat) {
                bail!("lifecycle authority --repeat 必须在 2..=10 范围内");
            }
            goal::validate_authority_command(root, &command)?;
            let (parsed, command_rebind) = match maintenance_cycle_rebind.as_deref() {
                Some(current_cycle) => {
                    let (parsed, rebind) =
                        goal::prepare_maintenance_cycle_rebind(root, &command, current_cycle)?;
                    (parsed, Some(rebind))
                }
                None => (goal::parse_validation_command(&command)?, None),
            };
            let listed_tests = if let Some(list_command) = goal::validation_list_command(&parsed)? {
                let output = run_validation_command(root, &list_command)?;
                if !output.status.success() {
                    bail!("lifecycle authority 独立 test list proof 失败；不会写入 proof");
                }
                Some(goal::listed_test_count(
                    &list_command,
                    &output.stdout,
                    &output.stderr,
                )?)
            } else {
                None
            };
            let fingerprint = goal::workspace_fingerprint(root)?;
            let mut runs = Vec::new();
            for run_index in 1..=repeat {
                if let Some(rebind) = command_rebind.as_ref() {
                    goal::verify_maintenance_cycle_rebind_artifact(root, rebind)?;
                }
                let before = goal::workspace_fingerprint(root)?;
                if before != fingerprint {
                    bail!(
                        "lifecycle authority 第 {run_index} 次运行前 source fingerprint 漂移；不会写入 proof"
                    );
                }
                let output = run_validation_command(root, &parsed)?;
                if let Some(rebind) = command_rebind.as_ref() {
                    goal::verify_maintenance_cycle_rebind_artifact(root, rebind)?;
                }
                let after = goal::workspace_fingerprint(root)?;
                if !output.status.success() {
                    bail!(
                        "lifecycle authority 第 {run_index}/{repeat} 次失败（exit={}）；不会写入 proof",
                        output.status.code().unwrap_or(-1)
                    );
                }
                goal::validation_execution_proof(
                    &parsed,
                    &output.stdout,
                    &output.stderr,
                    listed_tests,
                )?;
                if before != after || after != fingerprint {
                    bail!(
                        "lifecycle authority 第 {run_index}/{repeat} 次修改了工作区；不会写入 proof"
                    );
                }
                runs.push(goal::AuthorityRunReceipt {
                    exit_code: output.status.code().unwrap_or(0),
                    workspace_fingerprint_before: before,
                    workspace_fingerprint_after: after,
                    stdout_sha256: sha256_hex(&output.stdout),
                    stderr_sha256: sha256_hex(&output.stderr),
                });
            }
            let invocation_sha256 = goal::replacement_authority_invocation_sha256_with_rebind(
                &command,
                &id,
                &authority_goal,
                &predecessors,
                repeat,
                command_rebind.as_ref(),
            );
            let live_authority = goal::ReplacementAuthorityReceipt {
                command: command.clone(),
                command_rebind,
                recorded_at: rayman::timefmt::now_iso(),
                workspace_fingerprint: fingerprint,
                repeat,
                invocation_sha256,
                runs,
            };
            let goal =
                store.authorize_replacement(&id, &predecessors, &authority_goal, live_authority)?;
            if json {
                print(&serde_json::to_value(&goal)?);
            } else {
                println!(
                    "目标 {} 已获 lifecycle-only replacement authority（source={}，predecessors={}）",
                    goal.id,
                    goal.replacement_authority
                        .as_ref()
                        .map(|proof| proof.workspace_fingerprint.as_str())
                        .unwrap_or("unknown"),
                    predecessors.join(",")
                );
            }
        }
        GoalAction::Supersede { id, replacement } => {
            let goal = store.supersede(&id, &replacement)?;
            if json {
                print(&serde_json::to_value(&goal)?);
            } else {
                println!("目标 {} 已由 {} 取代", goal.id, replacement);
            }
        }
        GoalAction::Current { id } => {
            if let Some(id) = id {
                let goal = store.mark_current(&id)?;
                if json {
                    print(&serde_json::to_value(&goal)?);
                } else {
                    println!("目标 {} 已恢复为 current", goal.id);
                }
            } else {
                let goals = store
                    .list()?
                    .into_iter()
                    .filter(|goal| goal.lifecycle == goal::GoalLifecycle::Current)
                    .collect::<Vec<_>>();
                if json {
                    print(&serde_json::to_value(&goals)?);
                } else if goals.is_empty() {
                    println!("暂无 current 目标。");
                } else {
                    for goal in goals {
                        println!("{}  [{}]  {}", goal.id, goal.status, goal.title);
                    }
                }
            }
        }
        GoalAction::Frontier { id } => {
            let Some(selected) = store.get(&id)? else {
                bail!("目标不存在: {id}");
            };
            let report = pending.frontier(&selected)?;
            if json {
                print(&serde_json::to_value(&report)?);
            } else {
                println!(
                    "goal {} frontier={:?} execution={:?} consultation={:?} ask_user_allowed={} background_execution_allowed={} — {}",
                    report.goal_id,
                    report.decision,
                    report.execution,
                    report.consultation,
                    report.ask_user_allowed,
                    report.background_execution_allowed,
                    report.reason
                );
                for blocker in report.blockers {
                    println!(
                        "  {} owner={} kind={} {}",
                        blocker.id, blocker.owner, blocker.kind, blocker.title
                    );
                }
            }
        }
        GoalAction::Pending(pending_cmd) => match pending_cmd.action {
            PendingAction::Add {
                title,
                message,
                goal: goal_id,
                owner,
                kind,
                attempts,
                evidence_paths,
                minimum_input,
                recommended,
                alternatives,
                risk,
                resume_command,
                auto_resume_condition,
                consultation_timing,
                background_mechanism,
                background_authority_evidence,
                background_isolation_evidence,
            } => {
                if let Some(goal_id) = goal_id.as_deref()
                    && store.get(goal_id)?.is_none()
                {
                    bail!("pending 绑定的 goal 不存在: {goal_id}");
                }
                let item = pending.add_structured(goal::PendingSubmission {
                    title,
                    detail: message,
                    goal_id,
                    owner: goal::PendingOwner::parse(&owner)?,
                    kind: goal::PendingKind::parse(&kind)?,
                    attempts,
                    evidence_paths,
                    minimum_input,
                    recommended_action: recommended,
                    alternatives,
                    risk,
                    resume_command,
                    auto_resume_condition,
                    consultation_timing: goal::ConsultationTiming::parse(&consultation_timing)?,
                    background_mechanism,
                    background_authority_evidence,
                    background_isolation_evidence,
                })?;
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
                // 与 goal show 同理：解决一个不存在的待完成项必须非零退出，
                // 否则调用方会把"没找到"当成"已解决"。
                if !pending.resolve(&id)? {
                    bail!("待完成项不存在: {id}");
                }
                if json {
                    print(&json!({ "resolved": true, "id": id }));
                } else {
                    println!("已解决待完成项。");
                }
            }
        },
    }
    Ok(())
}

fn impact_evidence_for_changed_paths(
    root: &Path,
    changed: &[String],
) -> Result<Vec<goal::ImpactEvidence>> {
    if changed.is_empty() {
        return Ok(Vec::new());
    }
    let project_map = map::build_readonly(root)?;
    changed
        .iter()
        .map(|path| {
            let report = map::impact_report(&project_map, path)?;
            Ok(impact_evidence_from_report(&report))
        })
        .collect()
}

fn run_validation_command(
    root: &Path,
    command: &goal::ParsedValidationCommand,
) -> Result<std::process::Output> {
    ProcessCommand::new(&command.program)
        .args(&command.args)
        .current_dir(root)
        .output()
        .with_context(|| format!("无法执行验证程序: {}", command.program))
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

fn impact_evidence_from_report(report: &map::ImpactReport) -> goal::ImpactEvidence {
    goal::ImpactEvidence {
        changed_path: report.changed_path.clone(),
        package: report.package.clone(),
        manifest_path: report.manifest_path.clone(),
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
        recorded_at: rayman::timefmt::now_iso(),
    }
}

fn task_proof_blockers(
    goal_blockers: &BTreeMap<String, Vec<String>>,
    shared_blockers: &[String],
    goal_id: &str,
) -> Vec<String> {
    let mut blockers = goal_blockers.get(goal_id).cloned().unwrap_or_default();
    blockers.extend_from_slice(shared_blockers);
    blockers.sort();
    blockers.dedup();
    blockers
}

/// 一次性就绪检查：聚合激活状态、源码状态、上下文新鲜度、资产扫描、待完成项、
/// 项目地图、质量档位与（绑定 `--goal` 时）任务门禁。任一硬阻塞都以非零码退出，
/// 便于脚本/agent 门禁。
///
/// 只在不带 `--refresh-context` 时是只读的：带上该标志（`finish` 总是带）会重建
/// 并落盘上下文索引。阻塞项远不止上下文与待完成项两类——完整清单见下方各
/// `blockers.push` 分支与 `goal_gate_verdict`。
fn run_check(root: &std::path::Path, json: bool, cmd: CheckCmd) -> Result<()> {
    let activation = workspace::activation_status(root)?;
    let refresh_report = if cmd.refresh_context {
        Some(context::refresh(root)?.1)
    } else {
        None
    };
    let source = source_state::inspect(root);
    let mut task_goal_id = cmd.goal.clone();
    let mut task_blockers = Vec::new();

    // `check` is a readiness claim, not a cheap UI probe: use content hashes.
    let freshness = context::strong_freshness(root);
    let asset_report = assets::scan(root)?;
    let goal_store = goal::GoalStore::new(root);
    // 损坏的 pending.json 是阻塞项而非"零待办"：静默放行会让门禁失效。
    let (pending, pending_error) = match goal::PendingStore::new(root).list() {
        Ok(items) => (items, None),
        Err(error) => (Vec::new(), Some(format!("{error:#}"))),
    };

    let context_blocked = freshness.status != "ready";
    let mut standard_blockers = Vec::new();
    let mut goal_blockers = BTreeMap::<String, Vec<String>>::new();
    let mut shared_task_proof_blockers = Vec::new();
    let mut standard_warnings = Vec::new();
    let mut map_summary = None;
    let mut map_quality = None;

    if matches!(cmd.profile, CheckProfile::Standard | CheckProfile::Release) {
        let (goals, goal_load_issues) = goal_store.list_with_issues()?;
        if task_goal_id.is_none() && cmd.require_current_goal {
            let current_ids = goals
                .iter()
                .filter(|goal| goal.lifecycle == goal::GoalLifecycle::Current)
                .map(|goal| goal.id.clone())
                .collect::<Vec<_>>();
            match current_ids.as_slice() {
                [id] => task_goal_id = Some(id.clone()),
                [] => task_blockers.push(
                    "要求绑定 current goal，但当前没有 current goal；先运行 goal start".into(),
                ),
                _ => task_blockers.push(format!(
                    "要求绑定唯一 current goal，但当前有 {} 个；请显式传 --goal <id>",
                    current_ids.len()
                )),
            }
        }
        if let Some(id) = task_goal_id.as_deref() {
            match goals.iter().find(|goal| goal.id == id) {
                None => task_blockers.push(format!("绑定的 goal 不存在: {id}")),
                Some(selected) => {
                    if selected.lifecycle != goal::GoalLifecycle::Current {
                        task_blockers.push(format!(
                            "绑定的 goal {id} lifecycle={}，必须为 current",
                            selected.lifecycle
                        ));
                    }
                    if selected.status != goal::GoalStatus::Success {
                        task_blockers.push(format!(
                            "绑定的 goal {id} status={}，必须完成验证并 close success",
                            selected.status
                        ));
                    }
                }
            }
        }
        for issue in goal_load_issues {
            let blocker = format!("goal 文件不可读取: {} ({})", issue.path, issue.error);
            standard_blockers.push(blocker.clone());
            shared_task_proof_blockers.push(blocker);
        }
        if !context_blocked {
            match map::build_readonly(root) {
                Ok(project_map) => {
                    map_summary = Some(map::summary(&project_map));
                    if !map::topology_is_authoritative(root, &project_map) {
                        // Still fail closed — unproven topology is unproven — but
                        // lead with the repair when the cause is the operator's
                        // environment rather than the repository. The actionable
                        // half used to be buried at the tail of a provenance string.
                        standard_blockers.push(
                            if map::topology_blocked_by_missing_cargo(
                                &project_map.topology_provenance,
                            ) {
                                format!(
                                    "环境未就绪: {}；无法确认 Cargo 拓扑",
                                    rayman::toolchain::unreachable_tool_advice("cargo")
                                )
                            } else {
                                format!(
                                    "Cargo workspace 拓扑未获 cargo metadata 权威确认: {}",
                                    project_map.topology_provenance
                                )
                            },
                        );
                    }
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
        let current_fingerprint = match goal::workspace_fingerprint(root) {
            Ok(fingerprint) => Some(fingerprint),
            Err(error) => {
                let blocker = format!("无法计算工作区内容指纹: {error}");
                standard_blockers.push(blocker.clone());
                shared_task_proof_blockers.push(blocker);
                None
            }
        };
        for checked_goal in &goals {
            // 门禁判定只有这一份实现，autosave 的"工作是否已完成"共用它。
            let verdict =
                goal::goal_gate_verdict(checked_goal, &goals, root, current_fingerprint.as_deref());
            goal_blockers.insert(checked_goal.id.clone(), verdict.blockers.clone());
            // Unbound `check` is a workspace-health claim. Goal lifecycle and
            // completion evidence belong only to an explicitly bound task
            // check/finish; otherwise an active goal makes the repository's
            // own authority gate circular and impossible to record.
            if !verdict.blockers.is_empty() {
                standard_warnings.push(format!(
                    "goal {} is not task-ready (bind with --goal to enforce): {}",
                    checked_goal.id,
                    verdict.blockers.join("; ")
                ));
            }
            standard_warnings.extend(verdict.warnings);
        }
    }

    // quick 档不解析目标绑定，所以 `--require-current-goal` 下 task_goal_id 恒为
    // None。只按 task_goal_id 判断会让这条路径以空 blocker 列表退出，用户拿不到
    // 任何原因——门禁本身是对的，缺的是诊断。
    if (task_goal_id.is_some() || cmd.require_current_goal) && cmd.profile == CheckProfile::Quick {
        task_blockers
            .push("goal-bound completion gate requires standard or release profile".into());
    }
    if context_blocked && task_goal_id.is_some() {
        shared_task_proof_blockers
            .push("任务门禁要求 ready context；使用 --refresh-context 或 prepare/finish".into());
    }
    if let Some(id) = task_goal_id.as_deref() {
        task_blockers.extend(task_proof_blockers(
            &goal_blockers,
            &shared_task_proof_blockers,
            id,
        ));
    }
    task_blockers.sort();
    task_blockers.dedup();
    let workspace_blocked = context_blocked
        || !pending.is_empty()
        || pending_error.is_some()
        || !standard_blockers.is_empty();
    let task_requested = task_goal_id.is_some() || cmd.require_current_goal;
    let task_ready = if task_requested {
        Some(task_goal_id.is_some() && task_blockers.is_empty())
    } else {
        None
    };
    let blocked = workspace_blocked || task_ready == Some(false);

    let readiness_scope = check_readiness_scope(cmd.profile);
    let release_contract = if cmd.profile == CheckProfile::Release {
        json!({
            "checked": false,
            "status": "not_checked",
            "detail": "release profile proves workspace strict-quality only; it is not installed release identity or source freshness",
            "required_verifier": SOURCE_FRESH_VERIFIER,
        })
    } else {
        json!({
            "checked": false,
            "status": "not_applicable",
        })
    };

    if json {
        print(&json!({
            "ready": !blocked,
            "workspace_ready": !workspace_blocked,
            "task": {
                "requested": task_goal_id.is_some() || cmd.require_current_goal,
                "goal_id": &task_goal_id,
                "ready": task_ready,
                "blockers": &task_blockers,
            },
            "source": &source,
            "context_refresh": &refresh_report,
            "activation": &activation,
            "profile": format!("{:?}", cmd.profile).to_ascii_lowercase(),
            "readiness_scope": readiness_scope,
            "release_contract": release_contract,
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
            "工作区就绪检查({readiness_scope}): {}",
            if blocked { "BLOCKED" } else { "READY" }
        );
        println!("  activation: {}", activation.status);
        print_source_state(&source);
        if let Some(refresh) = &refresh_report {
            println!(
                "  context refresh: total={} reused={} rehashed={} removed={}",
                refresh.total, refresh.reused, refresh.rehashed, refresh.removed
            );
        }
        // The hint is framework text, not user content, so it must not ride
        // through a placeholder: captures are reinserted verbatim, which left
        // `check --language en` printing a Chinese instruction. Two complete
        // authored messages instead of one message plus a captured tail.
        if context_blocked {
            println!(
                "  上下文: {} → 运行 `rayman context refresh`",
                freshness.status
            );
        } else {
            println!("  上下文: {}", freshness.status);
        }
        println!(
            "  资产: 过时候选 {}，未完成标记 {}（提示，不阻塞）",
            asset_report.obsolete.len(),
            asset_report.markers.len()
        );
        println!("  待完成项: {}", pending.len());
        if let Some(error) = &pending_error {
            println!("    BLOCKER: pending.json 不可读取: {error}");
        }
        if task_goal_id.is_some() || cmd.require_current_goal {
            println!(
                "  task: goal={} ready={}",
                task_goal_id.as_deref().unwrap_or("unresolved"),
                task_ready.unwrap_or(false)
            );
            for blocker in &task_blockers {
                println!("    TASK BLOCKER: {blocker}");
            }
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
        if cmd.profile == CheckProfile::Release {
            println!("  发布交接状态: 未检查（本结果仅是工作区 strict-quality）");
            println!("  交接/CI 必须运行 `{SOURCE_FRESH_VERIFIER}`");
        }
    }

    if blocked {
        std::process::exit(1);
    }
    Ok(())
}

fn check_readiness_scope(profile: CheckProfile) -> &'static str {
    match profile {
        CheckProfile::Quick => "workspace_base_snapshot",
        CheckProfile::Standard => "workspace_standard",
        CheckProfile::Release => "workspace_strict_quality",
    }
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
    if !report.findings_by_role.is_empty() {
        println!("  findings by role:");
        for (role, summary) in &report.findings_by_role {
            println!(
                "    {role}: total={} errors={} warnings={} info={}",
                summary.findings, summary.error_count, summary.warning_count, summary.info_count
            );
        }
    }
    for finding in &report.findings {
        println!(
            "    [{}][{}] {} {} — {}",
            finding.severity, finding.role, finding.kind, finding.path, finding.detail
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_fingerprint_refuses_an_incomplete_workspace_walk() {
        let dir = tempfile::tempdir().unwrap();
        assert!(goal::workspace_fingerprint(&dir.path().join("missing-workspace")).is_err());
    }

    #[test]
    fn task_proof_blockers_use_structured_goal_ownership() {
        let mut by_goal = BTreeMap::new();
        by_goal.insert("goal_selected".into(), vec!["selected blocker".into()]);
        by_goal.insert(
            "goal_other".into(),
            vec!["unrelated message mentions goal_selected".into()],
        );
        let shared = vec!["shared proof blocker".into()];

        assert_eq!(
            task_proof_blockers(&by_goal, &shared, "goal_selected"),
            vec!["selected blocker", "shared proof blocker"]
        );
        assert!(!task_proof_blockers(&by_goal, &shared, "missing").is_empty());
    }
}
