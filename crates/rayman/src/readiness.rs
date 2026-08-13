use std::collections::BTreeMap;
use std::path::Path;

use anyhow::Result;
use serde_json::json;

use crate::cli::{CheckCmd, CheckProfile};
use crate::{SOURCE_FRESH_VERIFIER, print, print_source_state};
use rayman::readiness_state::{ReadinessCapture, ReadinessStateSeal, changed_sections};

#[cfg(test)]
thread_local! {
    static READINESS_CAPTURE_COUNT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

fn capture_readiness(root: &Path) -> Result<ReadinessCapture> {
    #[cfg(test)]
    READINESS_CAPTURE_COUNT.with(|count| count.set(count.get() + 1));
    ReadinessCapture::capture(root)
}

#[cfg(test)]
fn reset_readiness_capture_count() {
    READINESS_CAPTURE_COUNT.with(|count| count.set(0));
}

#[cfg(test)]
fn readiness_capture_count() -> usize {
    READINESS_CAPTURE_COUNT.with(std::cell::Cell::get)
}
use rayman::{assets, context, goal, map, source_state, workspace};
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
pub(crate) fn run_check(root: &std::path::Path, json: bool, cmd: CheckCmd) -> Result<()> {
    run_check_with_terminal_hook(root, json, cmd, false, || {})
}

struct CheckOnce {
    refresh_report: Option<context::RefreshReport>,
}

struct ReadinessRound {
    evaluation: ReadinessEvaluation,
    refresh_report: Option<context::RefreshReport>,
    decision_seal: ReadinessStateSeal,
    terminal_seal: ReadinessStateSeal,
}

#[derive(Debug, Clone, serde::Serialize)]
struct GoalVerdictSnapshot {
    blockers: Vec<String>,
    warnings: Vec<String>,
}

#[derive(Clone)]
struct ReadinessEvaluation {
    source: source_state::SourceState,
    asset_report: assets::AssetReport,
    activation: workspace::WorkspaceActivationReport,
    freshness: context::FreshnessReport,
    goals: Vec<goal::Goal>,
    active_pending: Vec<goal::PendingItem>,
    historical_pending: Vec<goal::PendingItem>,
    pending_error: Option<String>,
    project_map: Option<map::ProjectMap>,
    topology_authoritative: Option<bool>,
    map_quality: Option<map::QualityReport>,
    current_fingerprint: Option<String>,
    goal_verdicts: BTreeMap<String, GoalVerdictSnapshot>,
    task_requested: bool,
    task_goal_id: Option<String>,
    task_blockers: Vec<String>,
    standard_blockers: Vec<String>,
    standard_warnings: Vec<String>,
    workspace_blocked: bool,
    task_ready: Option<bool>,
    blocked: bool,
}

#[derive(serde::Serialize)]
struct FinishReadinessBinding<'a> {
    source: &'a source_state::SourceState,
    asset_report: &'a assets::AssetReport,
    activation: &'a workspace::WorkspaceActivationReport,
    freshness: &'a context::FreshnessReport,
    workspace_fingerprint: Option<&'a str>,
    goals: Vec<&'a goal::Goal>,
    active_pending: Vec<&'a goal::PendingItem>,
    historical_pending: Vec<&'a goal::PendingItem>,
    pending_error: &'a Option<String>,
    project_map: Option<serde_json::Value>,
    topology_authoritative: Option<bool>,
    map_quality: &'a Option<map::QualityReport>,
    goal_verdicts: &'a BTreeMap<String, GoalVerdictSnapshot>,
    task_requested: bool,
    task_goal_id: &'a Option<String>,
    task_blockers: &'a [String],
    standard_blockers: &'a [String],
    standard_warnings: &'a [String],
    workspace_blocked: bool,
    task_ready: Option<bool>,
    blocked: bool,
}

fn semantic_project_map_value(
    project_map: Option<&map::ProjectMap>,
) -> Result<Option<serde_json::Value>> {
    project_map
        .map(|project_map| {
            let mut value = serde_json::to_value(project_map)?;
            let object = value
                .as_object_mut()
                .ok_or_else(|| anyhow::anyhow!("project map did not serialize as an object"))?;
            // `generated_at` is observation metadata. Every structural field,
            // including Cargo topology and provenance, remains in the binding.
            object.remove("generated_at");
            Ok(value)
        })
        .transpose()
}

fn finish_readiness_sha256(evaluation: &ReadinessEvaluation) -> Result<String> {
    // Directory enumeration order is not authority. Canonicalize independently
    // stored ledgers while preserving the semantic order within map/quality data.
    let mut goals = evaluation.goals.iter().collect::<Vec<_>>();
    goals.sort_by(|left, right| left.id.cmp(&right.id));
    let mut active_pending = evaluation.active_pending.iter().collect::<Vec<_>>();
    active_pending.sort_by(|left, right| left.id.cmp(&right.id));
    let mut historical_pending = evaluation.historical_pending.iter().collect::<Vec<_>>();
    historical_pending.sort_by(|left, right| left.id.cmp(&right.id));
    let binding = FinishReadinessBinding {
        source: &evaluation.source,
        asset_report: &evaluation.asset_report,
        activation: &evaluation.activation,
        freshness: &evaluation.freshness,
        workspace_fingerprint: evaluation.current_fingerprint.as_deref(),
        goals,
        active_pending,
        historical_pending,
        pending_error: &evaluation.pending_error,
        project_map: semantic_project_map_value(evaluation.project_map.as_ref())?,
        topology_authoritative: evaluation.topology_authoritative,
        map_quality: &evaluation.map_quality,
        goal_verdicts: &evaluation.goal_verdicts,
        task_requested: evaluation.task_requested,
        task_goal_id: &evaluation.task_goal_id,
        task_blockers: &evaluation.task_blockers,
        standard_blockers: &evaluation.standard_blockers,
        standard_warnings: &evaluation.standard_warnings,
        workspace_blocked: evaluation.workspace_blocked,
        task_ready: evaluation.task_ready,
        blocked: evaluation.blocked,
    };
    Ok(rayman::hash::sha256_bytes(&serde_json::to_vec(&binding)?))
}

fn activation_readiness_blocker(
    activation: &workspace::WorkspaceActivationReport,
) -> Option<String> {
    if activation.active {
        None
    } else if let Some(command) = activation.recovery_command.as_deref() {
        Some(format!(
            "RaymanCodingSkill 工作区激活身份已漂移（status={}）：运行 `{command}`",
            activation.status
        ))
    } else {
        Some(format!(
            "RaymanCodingSkill 工作区未显式激活（status={}）：运行 `rayman workspace activate --skill-file <canonical-SKILL.md> --yes`；历史 .RaymanCodingSkill 状态不会自动激活 skill",
            activation.status
        ))
    }
}

pub(crate) fn run_check_with_terminal_hook(
    root: &std::path::Path,
    json: bool,
    cmd: CheckCmd,
    verify_terminal_readiness: bool,
    between_finish_evaluations: impl FnOnce(),
) -> Result<()> {
    let (once, evaluation) = if verify_terminal_readiness {
        evaluate_finish_with_hook(root, &cmd, between_finish_evaluations)?
    } else {
        let mut no_gate = |_: &ReadinessCapture, _: &mut ReadinessEvaluation| {};
        let round = evaluate_readiness_round(root, &cmd, cmd.refresh_context, &mut no_gate)?;
        (
            CheckOnce {
                refresh_report: round.refresh_report,
            },
            round.evaluation,
        )
    };
    render_check(json, &cmd, &once, &evaluation)
}

fn evaluate_readiness_from_capture(
    capture: &ReadinessCapture,
    cmd: &CheckCmd,
) -> Result<ReadinessEvaluation> {
    let root = capture.root();
    let source = capture.source().clone();
    let asset_report = assets::scan_from_capture(capture.captured_files());
    let activation = workspace::activation_status_from_capture(
        root,
        capture.state_present(),
        capture.activation_config_bytes(),
        capture.activation_skill(),
    )?;
    let mut task_goal_id = cmd.goal.clone();
    let mut task_blockers = Vec::new();

    let (freshness, verified_index) = capture.verify_context();
    let goals = capture.goals().to_vec();
    let goal_load_issues = capture.goal_load_issues();
    // 损坏的 pending.json 是阻塞项而非"零待办"：静默放行会让门禁失效。
    let (pending, historical_pending, pending_error) = match capture.pending_readiness() {
        Ok(report) => (report.active, report.historical, None),
        Err(error) => (Vec::new(), Vec::new(), Some(format!("{error:#}"))),
    };

    let context_blocked = freshness.status != "ready";
    let mut standard_blockers = Vec::new();
    let mut goal_blockers = BTreeMap::<String, Vec<String>>::new();
    let mut shared_task_proof_blockers = Vec::new();
    let mut standard_warnings = Vec::new();
    let mut project_map = None;
    let mut topology_authoritative = None;
    let mut map_quality = None;
    let mut current_fingerprint = None;
    let mut goal_verdicts = BTreeMap::new();

    if let Some(blocker) = activation_readiness_blocker(&activation) {
        standard_blockers.push(blocker.clone());
        shared_task_proof_blockers.push(blocker);
    }

    if matches!(cmd.profile, CheckProfile::Standard | CheckProfile::Release) {
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
            let current_map = verified_index
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("ready context 缺少 verified index"))
                .and_then(|index| map::build_from_capture(root, index, capture.workspace_bytes()));
            match current_map {
                Ok(current_map) => {
                    let authoritative = map::topology_is_authoritative(root, &current_map);
                    topology_authoritative = Some(authoritative);
                    if !authoritative {
                        // Still fail closed — unproven topology is unproven — but
                        // lead with the repair when the cause is the operator's
                        // environment rather than the repository. The actionable
                        // half used to be buried at the tail of a provenance string.
                        standard_blockers.push(
                            if map::topology_blocked_by_missing_cargo(
                                &current_map.topology_provenance,
                            ) {
                                format!(
                                    "环境未就绪: {}；无法确认 Cargo 拓扑",
                                    rayman::toolchain::unreachable_tool_advice("cargo")
                                )
                            } else {
                                format!(
                                    "Cargo workspace 拓扑未获 cargo metadata 权威确认: {}",
                                    current_map.topology_provenance
                                )
                            },
                        );
                    }
                    let quality = if cmd.profile == CheckProfile::Release {
                        let config = map::load_quality_config_from_capture(
                            "strict",
                            capture.workspace_bytes(),
                        )?;
                        map::quality_report_with_config(&current_map, &config)
                    } else {
                        map::quality_report(&current_map)
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
                    project_map = Some(current_map);
                }
                Err(error) => {
                    standard_blockers.push(format!("项目地图不可用: {error}"));
                }
            }
        }
        current_fingerprint = Some(capture.baseline().workspace_fingerprint.clone());
        let goal_decision = capture.goal_decision_context();
        for checked_goal in &goals {
            // 门禁判定只有这一份实现，autosave 的"工作是否已完成"共用它。
            let verdict =
                goal::goal_gate_verdict_with_context(checked_goal, &goals, &goal_decision);
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
            standard_warnings.extend(verdict.warnings.clone());
            goal_verdicts.insert(
                checked_goal.id.clone(),
                GoalVerdictSnapshot {
                    blockers: verdict.blockers,
                    warnings: verdict.warnings,
                },
            );
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
    standard_blockers.sort();
    standard_blockers.dedup();
    standard_warnings.sort();
    standard_warnings.dedup();
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

    Ok(ReadinessEvaluation {
        source,
        asset_report,
        activation,
        freshness,
        goals,
        active_pending: pending,
        historical_pending,
        pending_error,
        project_map,
        topology_authoritative,
        map_quality,
        current_fingerprint,
        goal_verdicts,
        task_requested,
        task_goal_id,
        task_blockers,
        standard_blockers,
        standard_warnings,
        workspace_blocked,
        task_ready,
        blocked,
    })
}

fn mark_in_round_readiness_drift(evaluation: &mut ReadinessEvaluation, sections: &[&'static str]) {
    let blocker = format!(
        "readiness state changed during the complete evaluation (sections={}); retry after workspace and state writes stop",
        sections.join(",")
    );
    evaluation.standard_blockers.push(blocker.clone());
    if evaluation.task_requested {
        evaluation.task_blockers.push(blocker);
        evaluation.task_blockers.sort();
        evaluation.task_blockers.dedup();
        evaluation.task_ready = Some(false);
    }
    evaluation.standard_blockers.sort();
    evaluation.standard_blockers.dedup();
    evaluation.workspace_blocked = true;
    evaluation.blocked = true;
}

fn evaluate_readiness_round(
    root: &Path,
    cmd: &CheckCmd,
    refresh_context: bool,
    apply_gate: &mut impl FnMut(&ReadinessCapture, &mut ReadinessEvaluation),
) -> Result<ReadinessRound> {
    let mut decision = capture_readiness(root)?;
    let refresh_report = if refresh_context {
        Some(decision.refresh_context()?)
    } else {
        None
    };
    let decision_seal = decision.seal().clone();
    let mut evaluation = evaluate_readiness_from_capture(&decision, cmd)?;
    // Any authority/goal gate live observation must remain inside the raw
    // decision -> terminal bracket until GoalDecisionContext migration is
    // complete.
    apply_gate(&decision, &mut evaluation);
    let terminal = capture_readiness(root)?;
    let terminal_seal = terminal.seal().clone();
    let changed = changed_sections(&decision_seal, &terminal_seal);
    if !changed.is_empty() {
        mark_in_round_readiness_drift(&mut evaluation, &changed);
    }
    Ok(ReadinessRound {
        evaluation,
        refresh_report,
        decision_seal,
        terminal_seal,
    })
}

#[cfg(test)]
fn evaluate_readiness(root: &Path, cmd: &CheckCmd) -> Result<ReadinessEvaluation> {
    let mut no_gate = |_: &ReadinessCapture, _: &mut ReadinessEvaluation| {};
    Ok(evaluate_readiness_round(root, cmd, false, &mut no_gate)?.evaluation)
}

fn mark_finish_readiness_drift(evaluation: &mut ReadinessEvaluation) {
    let blocker =
        "finish readiness changed between the two complete evaluations; retry after workspace and state writes stop"
            .to_string();
    evaluation.standard_blockers.push(blocker.clone());
    if evaluation.task_requested {
        evaluation.task_blockers.push(blocker);
        evaluation.task_blockers.sort();
        evaluation.task_blockers.dedup();
        evaluation.task_ready = Some(false);
    }
    evaluation.standard_blockers.sort();
    evaluation.standard_blockers.dedup();
    evaluation.workspace_blocked = true;
    evaluation.blocked = true;
}

fn apply_finish_authority_gate(capture: &ReadinessCapture, evaluation: &mut ReadinessEvaluation) {
    let authority_ready = evaluation
        .task_goal_id
        .as_deref()
        .and_then(|goal_id| {
            evaluation
                .goals
                .iter()
                .find(|candidate| candidate.id == goal_id)
                .map(|selected| {
                    goal::has_current_stable_authority_receipt_with_context(
                        selected,
                        &evaluation.goals,
                        &capture.goal_decision_context(),
                    )
                })
        })
        .unwrap_or(false);
    if authority_ready {
        return;
    }

    let goal_id = evaluation.task_goal_id.as_deref().unwrap_or("unresolved");
    evaluation.task_blockers.push(format!(
        "finish 要求当前稳定 authority receipt；先运行 `rayman goal validate {goal_id} --req <req> --message <evidence> --command <project-gate> --changed <path> --authority --repeat 2`"
    ));
    evaluation.task_blockers.sort();
    evaluation.task_blockers.dedup();
    evaluation.task_ready = Some(false);
    evaluation.blocked = true;
}

fn evaluate_finish_with_hook(
    root: &Path,
    cmd: &CheckCmd,
    between_finish_evaluations: impl FnOnce(),
) -> Result<(CheckOnce, ReadinessEvaluation)> {
    evaluate_finish_with_gate_hook(
        root,
        cmd,
        between_finish_evaluations,
        apply_finish_authority_gate,
    )
}

fn evaluate_finish_with_gate_hook(
    root: &Path,
    cmd: &CheckCmd,
    between_finish_evaluations: impl FnOnce(),
    mut apply_gate: impl FnMut(&ReadinessCapture, &mut ReadinessEvaluation),
) -> Result<(CheckOnce, ReadinessEvaluation)> {
    let first = evaluate_readiness_round(root, cmd, cmd.refresh_context, &mut apply_gate)?;
    let first_sha256 = finish_readiness_sha256(&first.evaluation)?;
    between_finish_evaluations();
    let mut second = evaluate_readiness_round(root, cmd, false, &mut apply_gate)?;
    let second_sha256 = finish_readiness_sha256(&second.evaluation)?;
    let cross_round_sections = changed_sections(&first.terminal_seal, &second.decision_seal);
    if second_sha256 != first_sha256 || !cross_round_sections.is_empty() {
        mark_finish_readiness_drift(&mut second.evaluation);
    }
    Ok((
        CheckOnce {
            refresh_report: first.refresh_report,
        },
        second.evaluation,
    ))
}

fn render_check(
    json: bool,
    cmd: &CheckCmd,
    once: &CheckOnce,
    evaluation: &ReadinessEvaluation,
) -> Result<()> {
    let map_summary = evaluation.project_map.as_ref().map(map::summary);

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
            "ready": !evaluation.blocked,
            "workspace_ready": !evaluation.workspace_blocked,
            "task": {
                "requested": evaluation.task_requested,
                "goal_id": &evaluation.task_goal_id,
                "ready": evaluation.task_ready,
                "blockers": &evaluation.task_blockers,
            },
            "source": &evaluation.source,
            "context_refresh": &once.refresh_report,
            "activation": &evaluation.activation,
            "profile": format!("{:?}", cmd.profile).to_ascii_lowercase(),
            "readiness_scope": readiness_scope,
            "release_contract": release_contract,
            "context": serde_json::to_value(&evaluation.freshness)?,
            "assets": {
                "obsolete": evaluation.asset_report.obsolete.len(),
                "markers": evaluation.asset_report.markers.len(),
            },
            "pending": evaluation.active_pending.len(),
            "historical_pending": evaluation.historical_pending.len(),
            "pending_error": &evaluation.pending_error,
            "standard": {
                "blockers": &evaluation.standard_blockers,
                "warnings": &evaluation.standard_warnings,
                "project_map": map_summary,
                "quality": &evaluation.map_quality,
            },
        }));
    } else {
        println!(
            "工作区就绪检查({readiness_scope}): {}",
            if evaluation.blocked {
                "BLOCKED"
            } else {
                "READY"
            }
        );
        println!("  activation: {}", evaluation.activation.status);
        print_source_state(&evaluation.source);
        if let Some(refresh) = &once.refresh_report {
            println!(
                "  context refresh: total={} reused={} rehashed={} removed={}",
                refresh.total, refresh.reused, refresh.rehashed, refresh.removed
            );
        }
        // The hint is framework text, not user content, so it must not ride
        // through a placeholder: captures are reinserted verbatim, which left
        // `check --language en` printing a Chinese instruction. Two complete
        // authored messages instead of one message plus a captured tail.
        if evaluation.freshness.status != "ready" {
            println!(
                "  上下文: {} → 运行 `rayman context refresh`",
                evaluation.freshness.status
            );
        } else {
            println!("  上下文: {}", evaluation.freshness.status);
        }
        println!(
            "  资产: 过时候选 {}，未完成标记 {}（提示，不阻塞）",
            evaluation.asset_report.obsolete.len(),
            evaluation.asset_report.markers.len()
        );
        println!("  待完成项: {}", evaluation.active_pending.len());
        if !evaluation.historical_pending.is_empty() {
            println!(
                "  历史待完成项（保留，不阻塞）: {}",
                evaluation.historical_pending.len()
            );
        }
        if let Some(error) = &evaluation.pending_error {
            println!("    BLOCKER: pending.json 不可读取: {error}");
        }
        if evaluation.task_requested {
            println!(
                "  task: goal={} ready={}",
                evaluation.task_goal_id.as_deref().unwrap_or("unresolved"),
                evaluation.task_ready.unwrap_or(false)
            );
            for blocker in &evaluation.task_blockers {
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
            if let Some(quality) = &evaluation.map_quality {
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
            println!(
                "  standard blockers: {}",
                evaluation.standard_blockers.len()
            );
            for blocker in &evaluation.standard_blockers {
                println!("    BLOCKER: {blocker}");
            }
            println!(
                "  standard warnings: {}",
                evaluation.standard_warnings.len()
            );
            for warning in &evaluation.standard_warnings {
                println!("    warning: {warning}");
            }
        }
        if cmd.profile == CheckProfile::Release {
            println!("  发布交接状态: 未检查（本结果仅是工作区 strict-quality）");
            println!("  交接/CI 必须运行 `{SOURCE_FRESH_VERIFIER}`");
        }
    }

    if evaluation.blocked {
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

    fn quick_cmd() -> CheckCmd {
        CheckCmd {
            profile: CheckProfile::Quick,
            goal: None,
            require_current_goal: false,
            refresh_context: false,
        }
    }

    #[test]
    fn finish_readiness_rejects_inactive_activation() {
        let workspace = tempfile::tempdir().unwrap();
        reset_readiness_capture_count();
        let evaluation = evaluate_readiness(workspace.path(), &quick_cmd()).unwrap();

        assert_eq!(readiness_capture_count(), 2);
        assert!(evaluation.blocked);
        assert!(
            evaluation
                .standard_blockers
                .iter()
                .any(|blocker| blocker.contains("未显式激活")),
            "{:?}",
            evaluation.standard_blockers
        );
    }

    #[test]
    fn finish_runs_two_complete_rounds_even_when_the_first_is_blocked() {
        let workspace = tempfile::tempdir().unwrap();
        let between_called = std::cell::Cell::new(false);
        let gate_calls = std::cell::Cell::new(0usize);
        reset_readiness_capture_count();

        let (_, evaluation) = evaluate_finish_with_gate_hook(
            workspace.path(),
            &quick_cmd(),
            || between_called.set(true),
            |_, _| gate_calls.set(gate_calls.get() + 1),
        )
        .unwrap();

        assert!(
            evaluation.blocked,
            "inactive activation must block both rounds"
        );
        assert!(
            between_called.get(),
            "the between-round hook must still run"
        );
        assert_eq!(
            gate_calls.get(),
            2,
            "one gate evaluation belongs to each round"
        );
        assert_eq!(
            readiness_capture_count(),
            4,
            "finish must execute decision+terminal capture in both rounds"
        );
    }

    #[test]
    fn terminal_capture_rejects_a_gate_side_workspace_write() {
        let workspace = tempfile::tempdir().unwrap();
        let root = workspace.path();
        reset_readiness_capture_count();
        let mut gate = |_: &ReadinessCapture, _: &mut ReadinessEvaluation| {
            std::fs::write(
                root.join("gate-marker.txt"),
                "changed after decision capture",
            )
            .unwrap();
        };

        let round = evaluate_readiness_round(root, &quick_cmd(), false, &mut gate).unwrap();

        assert_eq!(readiness_capture_count(), 2);
        assert_eq!(
            changed_sections(&round.decision_seal, &round.terminal_seal),
            ["workspace"]
        );
        assert!(round.evaluation.standard_blockers.iter().any(|blocker| {
            blocker.contains("changed during the complete evaluation")
                && blocker.contains("sections=workspace")
        }));
    }

    #[test]
    fn finish_semantic_map_binding_ignores_only_generated_at() {
        let mut map = map::ProjectMap {
            generated_at: "first".into(),
            workspace: "workspace".into(),
            topology_provenance: "cargo_metadata".into(),
            source_files: 0,
            test_files: 0,
            docs_files: 0,
            config_files: 0,
            script_files: 0,
            asset_files: 0,
            modules: Vec::new(),
            symbols: Vec::new(),
            dependencies: Vec::new(),
            packages: Vec::new(),
            package_dependencies: Vec::new(),
            entrypoints: Vec::new(),
            tests: Vec::new(),
            risks: Vec::new(),
        };
        let first = semantic_project_map_value(Some(&map)).unwrap();
        map.generated_at = "second".into();
        assert_eq!(semantic_project_map_value(Some(&map)).unwrap(), first);
        map.topology_provenance = "heuristic_fallback: drift".into();
        assert_ne!(semantic_project_map_value(Some(&map)).unwrap(), first);
    }

    #[test]
    fn finish_between_hook_forces_a_complete_second_evaluation() {
        let workspace = tempfile::tempdir().unwrap();
        let root = workspace.path();
        let skill = root.join("SKILL.md");
        std::fs::write(&skill, include_bytes!("../../../SKILL.md")).unwrap();
        workspace::activate(root, &skill).unwrap();
        context::refresh(root).unwrap();

        let activation_path = root.join(".RaymanCodingSkill/workspace_skill.yaml");
        let (_, evaluation) = evaluate_finish_with_gate_hook(
            root,
            &quick_cmd(),
            || {
                let contract = std::fs::read_to_string(&activation_path).unwrap();
                std::fs::write(
                    &activation_path,
                    contract.replace("enabled: true", "enabled: false"),
                )
                .unwrap();
            },
            |_, _| {},
        )
        .unwrap();

        assert!(evaluation.blocked);
        assert!(!evaluation.activation.active);
        assert!(
            evaluation
                .standard_blockers
                .iter()
                .any(|blocker| blocker.contains("between the two complete evaluations")),
            "{:?}",
            evaluation.standard_blockers
        );
    }
    #[test]
    fn finish_binding_includes_terminal_source_and_assets() {
        let workspace = tempfile::tempdir().unwrap();
        let first = evaluate_readiness(workspace.path(), &quick_cmd()).unwrap();
        let mut second = first.clone();
        second.source.head = Some("b".repeat(40));
        assert_ne!(
            finish_readiness_sha256(&first).unwrap(),
            finish_readiness_sha256(&second).unwrap()
        );

        second = first.clone();
        second.asset_report.markers.push(assets::MarkerFinding {
            path: "terminal-marker.txt".into(),
            line: 1,
            marker: "TODO".into(),
            text: "TODO terminal snapshot".into(),
        });
        assert_ne!(
            finish_readiness_sha256(&first).unwrap(),
            finish_readiness_sha256(&second).unwrap()
        );
    }

    #[test]
    fn finish_terminal_observations_come_from_second_evaluation() {
        let workspace = tempfile::tempdir().unwrap();
        let root = workspace.path();
        let skill = root.join("SKILL.md");
        std::fs::write(&skill, include_bytes!("../../../SKILL.md")).unwrap();
        workspace::activate(root, &skill).unwrap();
        context::refresh(root).unwrap();

        let (_, evaluation) = evaluate_finish_with_gate_hook(
            root,
            &quick_cmd(),
            || {
                std::fs::write(root.join("terminal-marker.txt"), "TODO terminal snapshot").unwrap();
            },
            |_, _| {},
        )
        .unwrap();

        assert!(evaluation.blocked);
        assert!(
            evaluation.asset_report.markers.iter().any(|finding| {
                finding.path == "terminal-marker.txt" && finding.marker == "TODO"
            })
        );
        assert!(
            evaluation
                .standard_blockers
                .iter()
                .any(|blocker| blocker.contains("between the two complete evaluations"))
        );
    }
}
