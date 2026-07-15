mod cli;

use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;

use anyhow::{Context, Result, bail};
use clap::Parser;
use serde_json::json;
use sha2::{Digest, Sha256};

use cli::{
    AutosaveAction, AutosaveCmd, CheckCmd, CheckProfile, CheckpointAction, CheckpointCmd, Cli,
    Command, ContextAction, ContextCmd, DoctorCmd, Format, GoalAction, GoalCmd, MapAction, MapCmd,
    PendingAction, PendingCmd, QualityProfile, StateAction, StateCmd, TempAction, TempCmd,
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

        Command::State(StateCmd { action }) => match action {
            StateAction::Audit { check } => run_state_audit(&root, json, check)?,
        },

        Command::Check(cmd) => return run_check(&root, json, cmd),

        Command::Map(cmd) => return run_map(&root, json, cmd),

        Command::Checkpoint(cmd) => return run_checkpoint(&root, json, cmd),

        Command::Autosave(cmd) => return run_autosave(&root, json, cmd),

        Command::Doctor(cmd) => return run_doctor(&root, json, cmd),
    }
    Ok(())
}

fn run_state_audit(root: &Path, json: bool, check: bool) -> Result<()> {
    const V2_ALLOWED: &[&str] = &[
        "goals",
        "pending.json",
        "context",
        "autosave.json",
        "autosave.lock",
        "tmp",
        "workspace_skill.yaml",
        "quality.json",
    ];
    let state = root.join(".RaymanCodingSkill");
    let mut retired = Vec::new();
    let mut errors = Vec::new();
    match rayman::state_paths::managed_state_root(root, false) {
        Ok(None) => {}
        Ok(Some(verified_state)) => match std::fs::read_dir(&verified_state) {
            Ok(entries) => {
                for entry in entries {
                    match entry {
                        Ok(entry) => {
                            let name = entry.file_name().to_string_lossy().to_string();
                            if !V2_ALLOWED.contains(&name.as_str()) {
                                retired.push(name);
                            } else if let Err(error) = audit_allowed_state_entry(root, &name) {
                                errors
                                    .push(format!("允许的状态项 `{name}` 不安全或无效: {error:#}"));
                            }
                        }
                        Err(error) => errors.push(error.to_string()),
                    }
                }
            }
            Err(error) => errors.push(error.to_string()),
        },
        Err(error) => errors.push(format!("无法安全读取受管状态: {error:#}")),
    }
    retired.sort();
    let temp_status = temp::audit(root);
    let clean = retired.is_empty() && errors.is_empty() && temp_status.traversal_error_count == 0;
    if json {
        print(&json!({
            "state_root": state,
            "allowed_v2_entries": V2_ALLOWED,
            "retired_entries": retired,
            "errors": errors,
            "temp": {
                "root": temp_status.root,
                "files": temp_status.file_count,
                "directories": temp_status.directory_count,
                "bytes": temp_status.total_bytes,
                "traversal_errors": temp_status.traversal_errors,
            },
            "clean": clean,
            "destructive_action": "none; inspect and migrate or remove retired state only after explicit user approval",
        }));
    } else {
        println!("受管状态审计: clean={clean}");
        println!(
            "  temp: files={} dirs={} {:.1} MB",
            temp_status.file_count,
            temp_status.directory_count,
            temp_status.total_bytes as f64 / 1_048_576.0
        );
        if !retired.is_empty() {
            println!("  retired entries: {}", retired.join(", "));
        }
        for error in errors.iter().chain(temp_status.traversal_errors.iter()) {
            println!("  error: {error}");
        }
        println!("  no files were deleted");
    }
    if check && !clean {
        bail!("受管状态包含退役条目或遍历错误；先审阅 `rayman state audit` 输出")
    }
    Ok(())
}

/// An allowlisted name is not automatically safe: `read_dir` exposes a lexical
/// entry, and a link/reparse point or wrong type would otherwise make audit
/// report `clean=true` while another command follows or later fails on it.
/// Reuse the same state-path authority as the readers and writers.
fn audit_allowed_state_entry(root: &Path, name: &str) -> Result<()> {
    match name {
        "goals" => {
            let Some(_) = rayman::state_paths::managed_state_dir(root, Path::new("goals"), false)?
            else {
                bail!("目录在枚举后消失");
            };
            let (_, issues) = goal::GoalStore::new(root).list_with_issues()?;
            if !issues.is_empty() {
                let details = issues
                    .iter()
                    .map(|issue| format!("{}: {}", issue.path, issue.error))
                    .collect::<Vec<_>>()
                    .join("; ");
                bail!("目标目录含不可安全读取的记录: {details}");
            }
            Ok(())
        }
        "context" | "tmp" => {
            let Some(_) = rayman::state_paths::managed_state_dir(root, Path::new(name), false)?
            else {
                bail!("目录在枚举后消失");
            };
            Ok(())
        }
        "pending.json"
        | "autosave.json"
        | "autosave.lock"
        | "workspace_skill.yaml"
        | "quality.json" => {
            let path = rayman::state_paths::managed_state_file(root, Path::new(name), false)?;
            match std::fs::symlink_metadata(&path) {
                Ok(_) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    bail!("文件在枚举后消失: {}", path.display())
                }
                Err(error) => Err(error)
                    .with_context(|| format!("无法读取允许的状态文件: {}", path.display())),
            }
        }
        _ => bail!("未知的允许状态项"),
    }
}

const CLI_CONTRACT: &str = "rayman-cli-contract-v5";
const SOURCE_FRESH_VERIFIER: &str = "scripts/verify-release-contract.ps1 -RequireSourceFresh";

fn run_doctor(root: &Path, json: bool, cmd: DoctorCmd) -> Result<()> {
    let running = std::env::current_exe().context("无法定位当前 rayman 二进制")?;
    let running_hash = rayman::hash::sha256_file(&running)?;
    let path_candidate = find_path_rayman();
    let path_hash = path_candidate
        .as_deref()
        .map(rayman::hash::sha256_file)
        .transpose()?;
    let path_matches_running = path_hash.as_deref() == Some(running_hash.as_str());

    let skill_path = root.join("SKILL.md");
    let skill_hash = rayman::hash::sha256_file(&skill_path).ok();
    let metadata_hash = workspace_skill_hash(root)?;
    let metadata_matches = match (&skill_hash, &metadata_hash) {
        (Some(actual), Some(expected)) => actual.eq_ignore_ascii_case(expected),
        _ => false,
    };
    // This proves an installed identity tuple only. It cannot prove that a release artifact was
    // rebuilt from the checkout's current sources: that requires an isolated locked rebuild and
    // byte comparison, which the repository verifier performs on explicit handoff/CI paths.
    // A managed workspace can be an ordinary user project, not this source checkout.
    // Looking for `<workspace>/target/release/rayman` therefore made `doctor --check`
    // falsely unusable after a correct installation.  Source-to-artifact byte identity
    // belongs exclusively to the explicit repository verifier below.
    let identity_ready = path_matches_running && metadata_matches;
    let report = json!({
        "contract": CLI_CONTRACT,
        "version": env!("CARGO_PKG_VERSION"),
        "running": {
            "path": running,
            "sha256": running_hash,
        },
        "path_rayman": path_candidate.as_ref().map(|path| json!({
            "path": path,
            "sha256": path_hash,
            "matches_running": path_matches_running,
        })),
        "repo_release": {
            "checked": false,
            "status": "not_checked_by_doctor",
            "required_verifier": SOURCE_FRESH_VERIFIER,
        },
        "workspace_skill": {
            "path": skill_path,
            "sha256": skill_hash,
            "recorded_sha256": metadata_hash,
            "matches_recorded": metadata_matches,
        },
        "release_identity": {
            "ready": identity_ready,
            "scope": "running_binary_path_command_and_workspace_skill_identity",
        },
        "source_fresh": {
            "verified": false,
            "status": "not_checked_by_doctor",
            "required_verifier": SOURCE_FRESH_VERIFIER,
        },
    });
    if json {
        print(&report);
    } else {
        println!(
            "已安装身份契约: {CLI_CONTRACT} v{}",
            env!("CARGO_PKG_VERSION")
        );
        println!("  当前二进制: {}", running.display());
        println!("  PATH 命令一致: {path_matches_running}");
        println!("  仓库源码产物: 未由 doctor 检查；交接/CI 由 `{SOURCE_FRESH_VERIFIER}` 验证");
        println!("  workspace SKILL 一致: {metadata_matches}");
        println!("  已安装身份 READY: {identity_ready}");
        println!("  源码新鲜度: 未由 doctor 证明；交接/CI 必须运行 `{SOURCE_FRESH_VERIFIER}`");
    }
    if cmd.check && !identity_ready {
        bail!(
            "已安装身份契约不一致：请使用仓库 release 二进制同步安装，并更新 .RaymanCodingSkill/workspace_skill.yaml 的 skill_sha256"
        );
    }
    Ok(())
}

#[cfg(windows)]
fn find_path_rayman() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    // `cmd`/Windows command discovery uses PATHEXT, not just `.exe`.  Selecting the
    // first matching command in each PATH directory makes an earlier `.cmd`/`.bat`
    // wrapper visible to doctor instead of falsely accepting a later rayman.exe.
    // Fall back to the normal Windows order if PATHEXT is absent or empty.
    let extensions = std::env::var_os("PATHEXT")
        .map(|raw| {
            raw.to_string_lossy()
                .split(';')
                .map(str::trim)
                .filter(|extension| !extension.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .filter(|extensions| !extensions.is_empty())
        .unwrap_or_else(|| vec![".COM".into(), ".EXE".into(), ".BAT".into(), ".CMD".into()]);
    std::env::split_paths(&path).find_map(|dir| {
        extensions
            .iter()
            .map(|extension| dir.join(format!("rayman{extension}")))
            .find(|candidate| candidate.is_file())
    })
}

#[cfg(not(windows))]
fn find_path_rayman() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join("rayman"))
        .find(|candidate| candidate.is_file())
}

fn workspace_skill_hash(root: &Path) -> Result<Option<String>> {
    let path =
        rayman::state_paths::managed_state_file(root, Path::new("workspace_skill.yaml"), false)?;
    let content = match std::fs::read_to_string(&path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("无法读取工作区 skill 状态: {}", path.display()));
        }
    };
    Ok(content.lines().find_map(|line| {
        line.strip_prefix("skill_sha256:")
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    }))
}

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
                            "status": c.status,
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
                            "  {}  {:?}  {} 个文件  {:.1} MB",
                            c.id,
                            c.status,
                            m.file_count,
                            m.total_bytes as f64 / 1_048_576.0
                        ),
                        None => println!("  {}  {:?}  (缺或损坏 manifest)", c.id, c.status),
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
                        "status": c.status,
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
                        println!("最近完整快照: {} ({:?}, 保存于 {created})", c.id, c.status);
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
        CheckpointAction::Verify { id } => {
            let checkpoints = checkpoint::list(root, dir)?;
            let target = match id.as_deref() {
                None | Some("latest") => checkpoint::latest(root, dir)?,
                Some(id) => checkpoints
                    .into_iter()
                    .find(|checkpoint| checkpoint.id == id),
            }
            .ok_or_else(|| anyhow::anyhow!("找不到可验证的 checkpoint"))?;
            let manifest = checkpoint::verify_snapshot(&target.path)?;
            if json {
                print(&json!({
                    "id": target.id,
                    "status": "complete",
                    "file_count": manifest.file_count,
                    "total_bytes": manifest.total_bytes,
                    "schema": manifest.schema,
                    "version": manifest.version,
                }));
            } else {
                println!(
                    "checkpoint {} 已验证：{} 个文件，{:.1} MB",
                    target.id,
                    manifest.file_count,
                    manifest.total_bytes as f64 / 1_048_576.0
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
                    println!(
                        "{}  [{}/{}]  {}",
                        goal.id, goal.lifecycle, goal.status, goal.title
                    );
                }
            }
        }
        GoalAction::Show { id } => {
            let goal = store.get(&id)?;
            if json {
                print(&serde_json::to_value(&goal)?);
            } else if let Some(goal) = goal {
                println!(
                    "{} [{}/{}] {}",
                    goal.id, goal.lifecycle, goal.status, goal.title
                );
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
        GoalAction::Validate {
            id,
            req,
            message,
            changed,
            non_code,
            command,
        } => {
            if message.trim().is_empty() || command.trim().is_empty() {
                bail!("`--message` 与 `--command` 都不能为空");
            }
            let impacts = impact_evidence_for_changed_paths(root, &changed)?;
            goal::validate_command_for_impacts(root, &command, &impacts, non_code)?;
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
                            &list_output.stdout,
                            &list_output.stderr,
                        )?),
                        Some(sha256_hex(&list_output.stdout)),
                        Some(sha256_hex(&list_output.stderr)),
                    )
                } else {
                    (None, None, None)
                };
            let output = run_validation_command(root, &parsed)?;
            let after = goal::workspace_fingerprint(root)?;
            if !output.status.success() {
                bail!(
                    "验证命令失败（exit={}）；不会写入 receipt。stdout_sha256={} stderr_sha256={}",
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
            if before != after {
                bail!(
                    "验证命令修改了工作区内容；不会写入 receipt。before={} after={}",
                    before,
                    after
                );
            }
            let impact_scopes = goal::validation_scopes_for_impacts(&impacts);
            let receipt = goal::ValidationReceipt {
                exit_code: output.status.code().unwrap_or(0),
                cwd: root.display().to_string(),
                workspace_fingerprint_before: before,
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
                contract_sha256,
            };
            let goal = store.record_validation_receipt(
                &id,
                &req,
                goal::ValidationReceiptSubmission {
                    evidence: message,
                    command,
                    receipt,
                    impacts,
                    non_code,
                },
            )?;
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
        } => {
            let goal = store.archive(&id, &reason, migrate_unreceipted)?;
            if json {
                print(&serde_json::to_value(&goal)?);
            } else {
                println!("目标 {} 已归档：{}", goal.id, reason.trim());
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
        recorded_at: rayman::fsutil::now_iso(),
    }
}

/// 一次性只读就绪检查：聚合上下文新鲜度、资产扫描、待完成项。
/// 有硬阻塞（上下文缺失/陈旧、存在待完成项）时以非零码退出，便于脚本/agent 门禁。
fn run_check(root: &std::path::Path, json: bool, cmd: CheckCmd) -> Result<()> {
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
                    if !map::topology_is_authoritative(root, &project_map) {
                        standard_blockers.push(format!(
                            "Cargo workspace 拓扑未获 cargo metadata 权威确认: {}",
                            project_map.topology_provenance
                        ));
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
                standard_blockers.push(format!("无法计算工作区内容指纹: {error}"));
                None
            }
        };
        for checked_goal in &goals {
            if let Some(error) = checked_goal.current_schema_error() {
                standard_blockers.push(format!("goal {} 合约无效: {error}", checked_goal.id));
                continue;
            }
            if let Some(error) = checked_goal.lifecycle_proof_error(root) {
                standard_blockers.push(format!(
                    "goal {} lifecycle proof 无效: {error}",
                    checked_goal.id
                ));
                continue;
            }
            if let Some(fingerprint) = current_fingerprint.as_deref()
                && let Some(error) =
                    goal::supersession_error(checked_goal, &goals, root, fingerprint)
            {
                standard_blockers.push(format!(
                    "goal {} supersession 合约无效: {error}",
                    checked_goal.id
                ));
                continue;
            }
            if checked_goal.lifecycle != goal::GoalLifecycle::Current {
                standard_warnings.push(format!(
                    "historical goal {} lifecycle={} 已保留但不参与当前 readiness{}",
                    checked_goal.id,
                    checked_goal.lifecycle,
                    checked_goal
                        .superseded_by
                        .as_deref()
                        .map(|id| format!("（superseded_by={id}）"))
                        .unwrap_or_default()
                ));
                continue;
            }
            if checked_goal.loaded_from_legacy {
                standard_blockers.push(format!(
                    "legacy goal {} 仍为 current（status={}）；legacy 记录不能生成当前 receipt，请显式 archive 历史 success，或新建 current-schema replacement 后 supersede",
                    checked_goal.id, checked_goal.status
                ));
                continue;
            }
            let requires_receipt = checked_goal.is_current_schema();
            match checked_goal.status {
                goal::GoalStatus::Success => {}
                goal::GoalStatus::Active => standard_blockers.push(format!(
                    "goal {} 仍为 active；用 goal validate 记录实际验证后必须 goal close",
                    checked_goal.id
                )),
                goal::GoalStatus::Partial | goal::GoalStatus::Blocked => {
                    standard_blockers.push(format!(
                        "goal {} 状态为 {}，不能作为 standard READY",
                        checked_goal.id, checked_goal.status
                    ))
                }
            }
            for req in &checked_goal.requirements {
                let is_must = req.kind == goal::RequirementKind::Must;
                if checked_goal.status == goal::GoalStatus::Active
                    && is_must
                    && req.status != goal::RequirementStatus::Done
                {
                    standard_blockers.push(format!(
                        "active goal {} 的 must 需求 {} 仍未完成",
                        checked_goal.id, req.id
                    ));
                }
                if checked_goal.status == goal::GoalStatus::Success
                    && is_must
                    && req.status != goal::RequirementStatus::Done
                {
                    standard_blockers.push(format!(
                        "success goal {} 的 must 需求 {} 未处于 done 状态",
                        checked_goal.id, req.id
                    ));
                }
                if req.status == goal::RequirementStatus::Done
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
                if req.status == goal::RequirementStatus::Done && req.validations.is_empty() {
                    standard_blockers.push(format!(
                        "goal {} 需求 {} 缺少验证 receipt",
                        checked_goal.id, req.id
                    ));
                }
                if !req.impacts.is_empty()
                    && let Some(fingerprint) = current_fingerprint.as_deref()
                {
                    for gap in goal::validation_relevance_gaps(req, checked_goal, root, fingerprint)
                    {
                        standard_blockers
                            .push(format!("goal {} 需求 {} {gap}", checked_goal.id, req.id));
                    }
                }
                if requires_receipt && checked_goal.status == goal::GoalStatus::Success && is_must {
                    let has_current_receipt = req.validations.iter().any(|validation| {
                        current_fingerprint.as_deref().is_some_and(|fingerprint| {
                            goal::validation_has_current_receipt(
                                validation,
                                checked_goal,
                                req,
                                root,
                                fingerprint,
                            )
                        })
                    });
                    if !has_current_receipt {
                        standard_blockers.push(format!(
                            "success goal {} 的 must 需求 {} 没有绑定当前工作区的成功 validation receipt",
                            checked_goal.id, req.id
                        ));
                    }
                }
                if req.status == goal::RequirementStatus::Done
                    && req.impacts.is_empty()
                    && !req.validations.is_empty()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_fingerprint_refuses_an_incomplete_workspace_walk() {
        let dir = tempfile::tempdir().unwrap();
        assert!(goal::workspace_fingerprint(&dir.path().join("missing-workspace")).is_err());
    }

    #[test]
    fn state_audit_check_refuses_an_unreadable_or_invalid_state_root() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".RaymanCodingSkill"), "not a directory").unwrap();
        assert!(run_state_audit(dir.path(), false, true).is_err());
    }

    #[test]
    fn state_audit_check_refuses_a_wrong_type_for_an_allowed_state_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".RaymanCodingSkill/pending.json")).unwrap();

        assert!(run_state_audit(dir.path(), false, true).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn state_audit_check_refuses_a_linked_allowed_state_entry() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".RaymanCodingSkill")).unwrap();
        std::fs::create_dir_all(outside.path().join("goals")).unwrap();
        symlink(
            outside.path().join("goals"),
            dir.path().join(".RaymanCodingSkill/goals"),
        )
        .unwrap();

        assert!(run_state_audit(dir.path(), false, true).is_err());
    }

    #[test]
    fn workspace_skill_hash_refuses_an_invalid_state_root() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".RaymanCodingSkill"), "not a directory").unwrap();
        assert!(workspace_skill_hash(dir.path()).is_err());
    }
}
