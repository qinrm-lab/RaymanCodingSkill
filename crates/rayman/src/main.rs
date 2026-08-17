#[macro_use]
mod i18n;
mod checkpoint_cli;
mod cli;
mod codex_hook_cli;
mod doctor;
mod goal_cli;
mod readiness;
mod state_audit_cli;
mod task_workflow;

use std::path::Path;
use std::process::Command as ProcessCommand;

use anyhow::{Context, Result, bail};
use clap::Parser;
use serde_json::json;
use sha2::{Digest, Sha256};

use cli::{
    AutosaveAction, AutosaveCmd, CheckCmd, CheckpointAction, CheckpointCmd, Cli, Command,
    ContextAction, ContextCmd, Format, GoalAction, GoalCmd, HandoffAction, MapAction, MapCmd,
    QualityProfile, StateAction, StateCmd, TempAction, TempCmd, WorkspaceAction, WorkspaceCmd,
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
            return readiness::run_check_with_terminal_hook(
                &root,
                json,
                CheckCmd {
                    profile: cmd.profile,
                    goal: Some(cmd.goal),
                    require_current_goal: true,
                    refresh_context: true,
                },
                true,
                || {},
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
                temp::release_pytest_lease(&root, &id)?;
                if json {
                    print(&json!({ "id": id, "removed": true }));
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

        Command::Check(cmd) => return readiness::run_check(&root, json, cmd),

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
            let activation_metadata = workspace::activation_metadata_capability_probe(root);
            let host_patch = rayman::codex_host::patch_probe(None);
            let execution_context = rayman::execution_context::execution_context_probe();
            if json {
                print(&json!({
                    "activation": activation,
                    "source": source,
                    "state_write": state_write,
                    "activation_metadata": activation_metadata,
                    "host_patch": host_patch,
                    "execution_context": execution_context,
                }));
            } else {
                println!(
                    "RaymanCodingSkill workspace activation: {} (active={}, config_present={})",
                    activation.status, activation.active, activation.config_present
                );
                print_source_state(&source);
                print_state_write_probe(&state_write);
                print_activation_metadata_probe(&activation_metadata);
                print_host_patch_probe(&host_patch);
                print_execution_context_probe(&execution_context);
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

/// Keep execution identity distinct from filesystem elevation. A caller can
/// bind the relevant identity/profile axis before attempting another broker;
/// the SID fingerprint alone is never an ACL-capability claim.
pub(crate) fn print_execution_context_probe(
    probe: &rayman::execution_context::ExecutionContextProbe,
) {
    if !probe.applicable {
        return;
    }
    println!(
        "  execution context: status={:?} principal_match={:?} profile_match={:?} account={} sid={} principal_fingerprint={} token_profile={} environment_profile={} environment_matches_token={:?} capability_key_hint={}",
        probe.status,
        probe.principal_match,
        probe.profile_match,
        probe.principal_account.as_deref().unwrap_or("unknown"),
        probe.principal_sid.as_deref().unwrap_or("unknown"),
        probe.principal_fingerprint.as_deref().unwrap_or("unknown"),
        probe.token_profile.as_deref().unwrap_or("unknown"),
        probe.environment_profile.as_deref().unwrap_or("unknown"),
        probe.environment_profile_matches_token,
        probe.capability_key_hint.as_deref().unwrap_or("none")
    );
    use rayman::execution_context::ExecutionContextStatus;
    match probe.status {
        ExecutionContextStatus::PrincipalMismatch => println!(
            "    principal mismatch: a principal-bound retry must prove the required SID/account changed; transport or elevation alone is not evidence — {}",
            probe.reason
        ),
        ExecutionContextStatus::ProfileMismatch => println!(
            "    profile mismatch: the same SID may be valid only after proving the required profile changed — {}",
            probe.reason
        ),
        ExecutionContextStatus::Unknown | ExecutionContextStatus::PlatformMismatch => println!(
            "    execution context unresolved: do not claim this requirement satisfied — {}",
            probe.reason
        ),
        ExecutionContextStatus::Match
        | ExecutionContextStatus::NotRequired
        | ExecutionContextStatus::NotApplicable => {}
    }
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

/// The generic state-write probe covers ordinary managed tmp files only. This
/// separate line reports whether the current process can stage the activation
/// file's exact authorization metadata without touching the canonical target.
pub(crate) fn print_activation_metadata_probe(
    probe: &rayman::workspace::ActivationMetadataCapabilityProbe,
) {
    if probe.ready {
        println!("  激活元数据写探针: 就绪（原授权元数据 staging 已验证，激活文件未变）");
    } else if !probe.applicable && !probe.probed {
        println!("  激活元数据写探针: 无激活合同或平台不支持，未探测");
    } else {
        println!(
            "  激活元数据写探针: 失败 phase={:?} class={:?} os_error={} activation_unchanged={:?} cleanup_complete={:?}: {}",
            probe.phase,
            probe.failure_class,
            probe
                .os_error_code
                .map(|code| code.to_string())
                .unwrap_or_else(|| "none".into()),
            probe.activation_unchanged,
            probe.cleanup_complete,
            probe.error.as_deref().unwrap_or("unknown error")
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
            workspace_snapshot,
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
            if workspace_snapshot && !authority {
                bail!("--workspace-snapshot 只允许与 --authority 一起使用");
            }
            let authority_goal = if authority {
                let goal_record = store
                    .get(&id)?
                    .ok_or_else(|| anyhow::anyhow!("目标不存在: {id}"))?;
                goal::validate_authority_command_for_goal(root, &goal_record, &command)?;
                Some(goal_record)
            } else {
                None
            };
            let impacts = impact_evidence_for_changed_paths(root, &changed)?;
            goal::validate_command_for_scope(
                root,
                &command,
                &impacts,
                non_code,
                workspace_snapshot,
            )?;
            let parsed = goal::parse_validation_command(&command)?;
            let contract_sha256 = store.validation_contract_hash(&id, &req)?;
            let snapshot_baseline = if workspace_snapshot {
                let goal_record = authority_goal
                    .as_ref()
                    .expect("workspace snapshot requires authority");
                let current = goal::workspace_baseline(root)?;
                let delta = goal::goal_plan_delta(goal_record, &current)?;
                if !delta.actual_changed_paths.is_empty() {
                    bail!(
                        "--workspace-snapshot 要求 goal baseline delta 为空；发现真实变更: {}。验证命令尚未执行",
                        delta.actual_changed_paths.join(", ")
                    );
                }
                Some(current)
            } else {
                None
            };
            let before = snapshot_baseline
                .as_ref()
                .map(|baseline| baseline.workspace_fingerprint.clone())
                .unwrap_or(goal::workspace_fingerprint(root)?);
            let mut validation_session = goal::ValidationExecutionSession::prepare(root, &parsed)?;
            let execution = (|| -> Result<_> {
                let (listed_tests, list_stdout_sha256, list_stderr_sha256) =
                    if let Some(list_command) = goal::validation_list_command(&parsed)? {
                        let list_output = run_validation_command_in_session(
                            root,
                            &list_command,
                            &validation_session,
                        )
                        .context("独立 test list proof 执行失败；不会写入 receipt")?;
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
                    let output =
                        run_validation_command_in_session(root, &parsed, &validation_session)
                            .with_context(|| {
                                format!(
                                    "验证命令第 {run_index}/{repeat} 次执行失败；不会写入 receipt"
                                )
                            })?;
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
                Ok((
                    listed_tests,
                    list_stdout_sha256,
                    list_stderr_sha256,
                    stable_runs,
                    final_output.expect("repeat is nonzero"),
                    final_test_proof,
                ))
            })();
            let (
                _listed_tests,
                list_stdout_sha256,
                list_stderr_sha256,
                stable_runs,
                output,
                final_test_proof,
            ) = validation_session.finish_with(execution)?;
            let after = before.clone();
            let test_proof = final_test_proof;
            let impact_scopes = goal::validation_scopes_for_impacts(&impacts);
            let receipt = goal::ValidationReceipt {
                exit_code: output.status.code().unwrap_or(0),
                cwd: root.display().to_string(),
                workspace_identity: context::workspace_identity(root),
                workspace_fingerprint_before: before.clone(),
                workspace_fingerprint_after: after,
                stdout_sha256: sha256_hex(&output.stdout),
                stderr_sha256: sha256_hex(&output.stderr),
                invocation_sha256: goal::validation_invocation_sha256_scoped_mode(
                    &command,
                    &impact_scopes,
                    non_code,
                    workspace_snapshot,
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
                            workspace_snapshot,
                            invocation_sha256: goal::authority_invocation_sha256_mode(
                                &command,
                                &req,
                                repeat,
                                &impact_scopes,
                                non_code,
                                workspace_snapshot,
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
            let fingerprint = goal::workspace_fingerprint(root)?;
            let mut validation_session = goal::ValidationExecutionSession::prepare(root, &parsed)?;
            let execution = (|| -> Result<_> {
                let listed_tests = if let Some(list_command) =
                    goal::validation_list_command(&parsed)?
                {
                    let output =
                        run_validation_command_in_session(root, &list_command, &validation_session)
                            .context(
                                "lifecycle authority 独立 test list proof 执行失败；不会写入 proof",
                            )?;
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
                    let output = run_validation_command_in_session(
                        root,
                        &parsed,
                        &validation_session,
                    )
                    .with_context(|| {
                        format!(
                            "lifecycle authority 第 {run_index}/{repeat} 次执行失败；不会写入 proof"
                        )
                    })?;
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
                Ok(runs)
            })();
            let runs = validation_session.finish_with(execution)?;
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
        GoalAction::Pending(command) => goal_cli::run_pending(&store, &pending, json, *command)?,
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
    let mut session = goal::ValidationExecutionSession::prepare(root, command)?;
    let output = run_validation_command_in_session(root, command, &session);
    session.finish_with(output)
}

fn run_validation_command_in_session(
    root: &Path,
    command: &goal::ParsedValidationCommand,
    session: &goal::ValidationExecutionSession,
) -> Result<std::process::Output> {
    let mut executable = command.clone();
    if let Some(script) = goal::resolve_live_powershell_script(root, command)? {
        // The receipt preserves the user's canonical logical command text. The
        // process gets a PowerShell-compatible path whose canonical round trip
        // was proven equal to the live identity that passed preflight.
        executable.args[2] = script.launch_argument().to_owned();
    }
    goal::run_with_managed_pytest_lease(root, &executable, |effective, environment| {
        let mut process = ProcessCommand::new(&effective.program);
        process.args(&effective.args).current_dir(root);
        session.apply(&mut process)?;
        if let Some(environment) = environment {
            // Parent-level pytest configuration is untrusted input. The lease
            // owns every temp/cache path, so inherited addopts must not be able
            // to inject a second basetemp, cache_dir, or non-executing mode.
            process.env_remove("PYTEST_ADDOPTS");
            process.envs(environment);
        }
        process
            .output()
            .with_context(|| format!("无法执行验证程序: {}", effective.program))
    })
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
