//! 结果聚合与报告渲染。
//!
//! 报告同时保留 ITT（全部计划尝试为分母）和 evaluable（仅 pass/fail 为分母）指标。
//! 这避免把后端/基础设施错误悄悄从分母抹掉后，再把一个描述性差异称为因果效应。

use std::collections::BTreeMap;

use serde::Serialize;

use crate::grade::GradeOutcome;
use crate::task::TaskProvenance;

pub const WITH_SKILL: &str = "with_skill";
pub const CONTROL: &str = "control";
const MIN_EVALUABLE_PER_CONDITION: usize = 2;

#[derive(Debug, Clone, Serialize)]
pub struct ArtifactHash {
    pub path: String,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct HostProvenance {
    pub os: String,
    pub family: String,
    pub arch: String,
}

/// The parsed identity returned by the selected rayman binary before a real trial.
/// Recording expected and reported values makes a release-binding decision auditable
/// rather than reducing it to an opaque successful process exit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReleaseContractProvenance {
    pub required: bool,
    /// Verified installed identity (binary/PATH/workspace SKILL), not a source rebuild proof.
    pub verified: bool,
    pub expected_contract: Option<String>,
    pub expected_version: Option<String>,
    pub reported_contract: Option<String>,
    pub reported_version: Option<String>,
    pub source_fresh_verified: Option<bool>,
    pub source_fresh_status: Option<String>,
    pub source_fresh_required_verifier: Option<String>,
}

impl ReleaseContractProvenance {
    pub fn not_required_for_mock() -> Self {
        Self {
            required: false,
            verified: false,
            expected_contract: None,
            expected_version: None,
            reported_contract: None,
            reported_version: None,
            source_fresh_verified: None,
            source_fresh_status: None,
            source_fresh_required_verifier: None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct RunProvenance {
    pub run_id: String,
    pub started_unix_seconds: u64,
    pub evaluator_version: String,
    pub backend: String,
    /// `mock` 或 `unsandboxed_host_execution`；不存在将 host shell 说成隔离环境的第三种状态。
    pub execution_mode: String,
    pub unsandboxed_host_execution: bool,
    /// 真实后端开始前，选定二进制已在固定 PATH 下通过 doctor 的 installed-identity
    /// 校验，且选定 skill 与工作区 SKILL.md 的内容一致。这不是 source-fresh 证明；
    /// 后者单独记录在 `release_contract`。mock 不需要受管元数据。
    pub release_contract_verified: bool,
    pub release_contract: ReleaseContractProvenance,
    pub git_head: Option<String>,
    pub skill: ArtifactHash,
    pub rayman_binary: ArtifactHash,
    pub host: HostProvenance,
    pub seed: u64,
    pub order_strategy: String,
    pub tasks: Vec<TaskProvenance>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    Pass,
    Fail,
    Error,
}

/// 评测期间重新校验的输入身份结果。任何漂移都使整个 run 不可比较；报告仍会
/// 落盘，以便保留已经观察到的结果和具体失配原因。
#[derive(Debug, Clone, Default, Serialize)]
pub struct InputIntegrity {
    pub drift_detected: bool,
    pub events: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TrialResult {
    pub task: String,
    pub condition: String,
    pub trial: usize,
    /// 该 task/trial pair 中的执行顺序（0 先执行，1 后执行）。
    pub condition_order: usize,
    pub outcome: Outcome,
    /// 评分命令的细分类；`timed_out` / `infrastructure_error` 必须映射为 trial Error，
    /// 不能伪装成模型 Fail。
    pub grade_outcome: GradeOutcome,
    pub grade_exit: i32,
    pub steps: usize,
    pub tool_calls: usize,
    pub rayman_invocations: usize,
    pub finished: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct EvalReport {
    pub model: String,
    pub trials_per_cell: usize,
    pub task_count: usize,
    pub provenance: RunProvenance,
    pub input_integrity: InputIntegrity,
    pub results: Vec<TrialResult>,
}

#[derive(Debug, Clone, Serialize)]
struct CellStat {
    planned: usize,
    observed: usize,
    pass: usize,
    fail: usize,
    error: usize,
}

impl CellStat {
    fn new(planned: usize) -> Self {
        Self {
            planned,
            observed: 0,
            pass: 0,
            fail: 0,
            error: 0,
        }
    }

    fn evaluable(&self) -> usize {
        self.pass + self.fail
    }

    fn evaluable_rate(&self) -> f64 {
        let evaluable = self.evaluable();
        if evaluable == 0 {
            0.0
        } else {
            self.pass as f64 / evaluable as f64
        }
    }

    fn itt_rate(&self) -> f64 {
        if self.planned == 0 {
            0.0
        } else {
            self.pass as f64 / self.planned as f64
        }
    }

    fn json(&self) -> serde_json::Value {
        serde_json::json!({
            "planned": self.planned,
            "observed": self.observed,
            "pass": self.pass,
            "fail": self.fail,
            "error": self.error,
            "evaluable": self.evaluable(),
            "evaluable_rate": self.evaluable_rate(),
            "intention_to_treat_rate": self.itt_rate(),
            // 保留旧字段，明确它是 evaluable rate，避免旧消费者静默误解。
            "rate": self.evaluable_rate(),
        })
    }

    fn markdown(&self) -> String {
        format!(
            "eval {}/{} ({:.0}%); ITT {}/{} ({:.0}%); {} err",
            self.pass,
            self.evaluable(),
            self.evaluable_rate() * 100.0,
            self.pass,
            self.planned,
            self.itt_rate() * 100.0,
            self.error
        )
    }
}

#[derive(Debug, Clone, Serialize)]
struct Interpretation {
    status: &'static str,
    /// 仅用于描述性比较，绝不代表因果识别条件已经满足。
    descriptive_comparison_eligible: bool,
    /// 保留机器可读的显式否定，避免旧消费者把“comparison eligible”误读成因果结论。
    causal_claim_eligible: bool,
    reasons: Vec<String>,
}

impl EvalReport {
    fn expected_per_condition(&self) -> usize {
        self.task_count.saturating_mul(self.trials_per_cell)
    }

    fn stat(&self, mut keep: impl FnMut(&TrialResult) -> bool, planned: usize) -> CellStat {
        let mut stat = CellStat::new(planned);
        for result in self.results.iter().filter(|result| keep(result)) {
            stat.observed += 1;
            match result.outcome {
                Outcome::Pass => stat.pass += 1,
                Outcome::Fail => stat.fail += 1,
                Outcome::Error => stat.error += 1,
            }
        }
        stat
    }

    fn cell(&self, task: &str, condition: &str) -> CellStat {
        self.stat(
            |result| result.task == task && result.condition == condition,
            self.trials_per_cell,
        )
    }

    fn overall(&self, condition: &str) -> CellStat {
        self.stat(
            |result| result.condition == condition,
            self.expected_per_condition(),
        )
    }

    fn tasks(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .results
            .iter()
            .map(|result| result.task.clone())
            .collect();
        names.sort();
        names.dedup();
        names
    }

    fn avg_rayman(&self, condition: &str) -> f64 {
        let cells: Vec<usize> = self
            .results
            .iter()
            .filter(|result| result.condition == condition)
            .map(|result| result.rayman_invocations)
            .collect();
        if cells.is_empty() {
            0.0
        } else {
            cells.iter().sum::<usize>() as f64 / cells.len() as f64
        }
    }

    fn first_order_counts(&self) -> (usize, usize) {
        let with_first = self
            .results
            .iter()
            .filter(|result| result.condition == WITH_SKILL && result.condition_order == 0)
            .count();
        let control_first = self
            .results
            .iter()
            .filter(|result| result.condition == CONTROL && result.condition_order == 0)
            .count();
        (with_first, control_first)
    }

    fn interpretation(&self, with: &CellStat, control: &CellStat) -> Interpretation {
        let mut reasons = Vec::new();
        let mut non_comparative = false;
        if self.provenance.unsandboxed_host_execution {
            non_comparative = true;
            reasons.push(
                "真实模型的 run 工具在未隔离宿主机执行；agent 可访问评测器输入，无法保证隐藏评分或处理/控制隔离，因此本 run 仅供调试，不可比较。"
                    .into(),
            );
        }
        if self.input_integrity.drift_detected {
            non_comparative = true;
            reasons.push(
                "评测期间检测到技能、rayman 二进制或任务输入漂移；报告保留为取证，结果不可比较。"
                    .into(),
            );
        }
        if non_comparative {
            return Interpretation {
                status: "non_comparative",
                descriptive_comparison_eligible: false,
                causal_claim_eligible: false,
                reasons,
            };
        }
        if self.trials_per_cell < 2 {
            reasons.push(format!(
                "每个单元仅 {} 次，少于最低 {} 次可评估尝试要求",
                self.trials_per_cell, MIN_EVALUABLE_PER_CONDITION
            ));
        }
        if with.observed != with.planned || control.observed != control.planned {
            reasons.push(format!(
                "计划/观察数量不一致（with_skill {}/{}, control {}/{})",
                with.observed, with.planned, control.observed, control.planned
            ));
        }
        if with.evaluable() < MIN_EVALUABLE_PER_CONDITION
            || control.evaluable() < MIN_EVALUABLE_PER_CONDITION
        {
            reasons.push(format!(
                "可评估尝试不足（with_skill {}, control {}；最低各 {}）",
                with.evaluable(),
                control.evaluable(),
                MIN_EVALUABLE_PER_CONDITION
            ));
        }
        // Error means an attempted observation was not trustworthy (backend, workspace, or
        // grading infrastructure). Equal counts do not make the missing observations
        // exchangeable: the failed attempts may be different tasks, orders, or failure modes.
        // Keep the comparison fail-closed until every planned attempt has a pass/fail result.
        if with.error > 0 || control.error > 0 {
            reasons.push(format!(
                "存在基础设施错误（with_skill {}, control {}）；任何 Error 都使描述性比较不可用",
                with.error, control.error
            ));
        }
        if reasons.is_empty() {
            Interpretation {
                status: "comparison_eligible",
                descriptive_comparison_eligible: true,
                causal_claim_eligible: false,
                reasons: vec![
                    "重复数和观测数满足预设的最低条件，且没有基础设施错误；仅适合描述性比较，不构成因果证明。"
                        .into(),
                ],
            }
        } else {
            Interpretation {
                status: "inconclusive",
                descriptive_comparison_eligible: false,
                causal_claim_eligible: false,
                reasons,
            }
        }
    }

    /// 机器可读摘要，含完整身份链、ITT/evaluable 指标和每次 trial 的执行顺序。
    pub fn summary_json(&self) -> serde_json::Value {
        let with = self.overall(WITH_SKILL);
        let control = self.overall(CONTROL);
        let interpretation = self.interpretation(&with, &control);
        let mut per_task = BTreeMap::new();
        for task in self.tasks() {
            let with_task = self.cell(&task, WITH_SKILL);
            let control_task = self.cell(&task, CONTROL);
            per_task.insert(
                task,
                serde_json::json!({
                    "with_skill": with_task.json(),
                    "control": control_task.json(),
                }),
            );
        }
        let (with_first, control_first) = self.first_order_counts();
        serde_json::json!({
            "model": self.model,
            "trials_per_cell": self.trials_per_cell,
            "task_count": self.task_count,
            "provenance": self.provenance,
            "input_integrity": self.input_integrity,
            "overall": {
                "planned_per_condition": self.expected_per_condition(),
                "with_skill": with.json(),
                "control": control.json(),
                // Backward-compatible aliases: explicitly evaluable, never hidden-error rates.
                "with_skill_rate": with.evaluable_rate(),
                "control_rate": control.evaluable_rate(),
                "delta": with.evaluable_rate() - control.evaluable_rate(),
                "observed_delta": {
                    "evaluable_rate": with.evaluable_rate() - control.evaluable_rate(),
                    "intention_to_treat_rate": with.itt_rate() - control.itt_rate(),
                },
                "error_balance": {
                    "with_skill": with.error,
                    "control": control.error,
                    "balanced": with.error == control.error,
                },
                "first_condition_counts": {
                    "with_skill": with_first,
                    "control": control_first,
                },
            },
            "interpretation": interpretation,
            "avg_rayman_invocations_with_skill": self.avg_rayman(WITH_SKILL),
            "per_task": per_task,
            "trials": self.results,
        })
    }

    /// 人类可读 Markdown 报告。禁止把不足/错误不平衡的 run 写成因果结论。
    pub fn markdown(&self) -> String {
        let with = self.overall(WITH_SKILL);
        let control = self.overall(CONTROL);
        let interpretation = self.interpretation(&with, &control);
        let mut out = String::new();
        out.push_str("# RaymanCodingSkill A/B outcome eval\n\n");
        out.push_str(&format!(
            "- Run ID: `{}`\n- Model: `{}`\n- Trials per cell: {}\n- Seed: `{}`\n- Git HEAD: `{}`\n",
            self.provenance.run_id,
            self.model,
            self.trials_per_cell,
            self.provenance.seed,
            self.provenance.git_head.as_deref().unwrap_or("unavailable")
        ));
        if self.input_integrity.drift_detected {
            out.push_str("- Input integrity: **DRIFT DETECTED — this run is non-comparative**\n");
        } else {
            out.push_str("- Input integrity: baseline verified before and after every trial\n");
        }
        if self.provenance.unsandboxed_host_execution {
            out.push_str(
                "- Execution mode: **UNSANDBOXED HOST EXECUTION (explicitly acknowledged)**\n",
            );
            if self.provenance.release_contract_verified {
                let contract = self
                    .provenance
                    .release_contract
                    .reported_contract
                    .as_deref()
                    .unwrap_or("unreported");
                let version = self
                    .provenance
                    .release_contract
                    .reported_version
                    .as_deref()
                    .unwrap_or("unreported");
                out.push_str(&format!(
                    "- Release identity: {contract} v{version}; selected rayman and workspace SKILL.md verified before trials\n"
                ));
                if let Some(status) = &self.provenance.release_contract.source_fresh_status {
                    let verifier = self
                        .provenance
                        .release_contract
                        .source_fresh_required_verifier
                        .as_deref()
                        .unwrap_or("unavailable");
                    out.push_str(&format!(
                        "- Source freshness: {status} (verified={}; verifier: {verifier})\n",
                        self.provenance
                            .release_contract
                            .source_fresh_verified
                            .unwrap_or(false)
                    ));
                }
            } else {
                out.push_str(
                    "- Release contract: **NOT VERIFIED — this run is non-comparative**\n",
                );
            }
        } else {
            out.push_str("- Execution mode: mock backend (no real model request)\n");
        }
        out.push_str(&format!(
            "- Provenance: skill SHA-256 `{}`, rayman SHA-256 `{}`, {} task/fixture manifests in `report.json`\n\n",
            self.provenance.skill.sha256,
            self.provenance.rayman_binary.sha256,
            self.provenance.tasks.len()
        ));
        out.push_str("| Task | With skill | Control |\n|---|---|---|\n");
        for task in self.tasks() {
            let with_task = self.cell(&task, WITH_SKILL);
            let control_task = self.cell(&task, CONTROL);
            out.push_str(&format!(
                "| {} | {} | {} |\n",
                task,
                with_task.markdown(),
                control_task.markdown()
            ));
        }
        out.push_str(&format!(
            "| **Overall** | **{}** | **{}** |\n\n",
            with.markdown(),
            control.markdown()
        ));
        if interpretation.status == "non_comparative" {
            // Keep raw trial results and machine-readable arithmetic in report.json for
            // forensics, but do not print a headline delta that a reader could mistake for an
            // effect when host isolation or input identity has already failed.
            out.push_str(
                "**Interpretation: NON-COMPARATIVE — raw trial results are retained for forensic review; no observed delta is displayed, and arms must not be compared or used for a causal claim.**\n\n",
            );
        } else {
            out.push_str(&format!(
                "**Observed delta**: evaluable {:+.1} percentage points; ITT {:+.1} percentage points.\n\n",
                (with.evaluable_rate() - control.evaluable_rate()) * 100.0,
                (with.itt_rate() - control.itt_rate()) * 100.0
            ));
        }
        if interpretation.descriptive_comparison_eligible {
            out.push_str(
                "**Interpretation: descriptive comparison eligible, not a causal proof.**\n\n",
            );
        } else if interpretation.status != "non_comparative" {
            out.push_str("**Interpretation: INCONCLUSIVE — do not make a causal claim.**\n\n");
        }
        for reason in &interpretation.reasons {
            out.push_str(&format!("- {reason}\n"));
        }
        let (with_first, control_first) = self.first_order_counts();
        out.push_str(&format!(
            "\nCondition-first counts: with_skill={}, control={}; strategy: {}\n\n",
            with_first, control_first, self.provenance.order_strategy
        ));
        out.push_str(&format!(
            "Avg `rayman` invocations per with-skill attempt: {:.1}\n",
            self.avg_rayman(WITH_SKILL)
        ));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provenance() -> RunProvenance {
        RunProvenance {
            run_id: "run-test".into(),
            started_unix_seconds: 0,
            evaluator_version: "test".into(),
            backend: "mock".into(),
            execution_mode: "mock".into(),
            unsandboxed_host_execution: false,
            release_contract_verified: false,
            release_contract: ReleaseContractProvenance::not_required_for_mock(),
            git_head: Some("head".into()),
            skill: ArtifactHash {
                path: "SKILL.md".into(),
                sha256: "skill".into(),
            },
            rayman_binary: ArtifactHash {
                path: "rayman".into(),
                sha256: "binary".into(),
            },
            host: HostProvenance {
                os: "test".into(),
                family: "test".into(),
                arch: "test".into(),
            },
            seed: 7,
            order_strategy: "test".into(),
            tasks: Vec::new(),
        }
    }

    fn trial(task: &str, condition: &str, outcome: Outcome, order: usize) -> TrialResult {
        TrialResult {
            task: task.into(),
            condition: condition.into(),
            trial: 0,
            condition_order: order,
            outcome,
            grade_outcome: match outcome {
                Outcome::Pass => GradeOutcome::Passed,
                Outcome::Fail => GradeOutcome::Failed,
                Outcome::Error => GradeOutcome::InfrastructureError,
            },
            grade_exit: if outcome == Outcome::Pass { 0 } else { 1 },
            steps: 1,
            tool_calls: 1,
            rayman_invocations: if condition == WITH_SKILL { 2 } else { 0 },
            finished: true,
            error: if outcome == Outcome::Error {
                Some("backend down".into())
            } else {
                None
            },
        }
    }

    fn report(task_count: usize, trials_per_cell: usize, results: Vec<TrialResult>) -> EvalReport {
        EvalReport {
            model: "mock".into(),
            trials_per_cell,
            task_count,
            provenance: provenance(),
            input_integrity: InputIntegrity::default(),
            results,
        }
    }

    #[test]
    fn aggregates_observed_evaluable_and_itt_deltas() {
        let report = report(
            2,
            1,
            vec![
                trial("t1", WITH_SKILL, Outcome::Pass, 0),
                trial("t1", CONTROL, Outcome::Fail, 1),
                trial("t2", WITH_SKILL, Outcome::Pass, 1),
                trial("t2", CONTROL, Outcome::Pass, 0),
            ],
        );
        let summary = report.summary_json();
        assert_eq!(summary["overall"]["with_skill_rate"], 1.0);
        assert_eq!(summary["overall"]["control_rate"], 0.5);
        assert_eq!(summary["overall"]["observed_delta"]["evaluable_rate"], 0.5);
        assert_eq!(
            summary["overall"]["observed_delta"]["intention_to_treat_rate"],
            0.5
        );
        assert_eq!(summary["provenance"]["release_contract"]["required"], false);
        assert_eq!(summary["provenance"]["release_contract"]["verified"], false);
        assert!(report.markdown().contains("Observed delta"));
    }

    #[test]
    fn errors_stay_visible_in_itt_and_mark_unbalanced_runs_inconclusive() {
        let report = report(
            1,
            2,
            vec![
                trial("t1", WITH_SKILL, Outcome::Pass, 0),
                trial("t1", WITH_SKILL, Outcome::Error, 1),
                trial("t1", CONTROL, Outcome::Fail, 1),
                trial("t1", CONTROL, Outcome::Fail, 0),
            ],
        );
        let summary = report.summary_json();
        assert_eq!(summary["overall"]["with_skill"]["evaluable_rate"], 1.0);
        assert_eq!(
            summary["overall"]["with_skill"]["intention_to_treat_rate"],
            0.5
        );
        assert_eq!(summary["overall"]["with_skill"]["error"], 1);
        assert_eq!(summary["overall"]["control"]["fail"], 2);
        assert_eq!(summary["interpretation"]["status"], "inconclusive");
        let markdown = report.markdown();
        assert!(
            markdown.contains("do not make a causal claim"),
            "{markdown}"
        );
        assert!(markdown.contains("存在基础设施错误"), "{markdown}");
    }

    #[test]
    fn equal_nonzero_errors_are_still_inconclusive() {
        let report = report(
            1,
            3,
            vec![
                trial("t1", WITH_SKILL, Outcome::Pass, 0),
                trial("t1", CONTROL, Outcome::Fail, 1),
                trial("t1", WITH_SKILL, Outcome::Fail, 1),
                trial("t1", CONTROL, Outcome::Pass, 0),
                trial("t1", WITH_SKILL, Outcome::Error, 0),
                trial("t1", CONTROL, Outcome::Error, 1),
            ],
        );

        let summary = report.summary_json();
        assert_eq!(summary["overall"]["with_skill"]["error"], 1);
        assert_eq!(summary["overall"]["control"]["error"], 1);
        assert_eq!(summary["interpretation"]["status"], "inconclusive");
        assert_eq!(
            summary["interpretation"]["descriptive_comparison_eligible"],
            false
        );
        assert!(
            report
                .markdown()
                .contains("任何 Error 都使描述性比较不可用")
        );
    }

    #[test]
    fn balanced_two_trial_run_is_only_descriptively_eligible() {
        let report = report(
            1,
            2,
            vec![
                trial("t1", WITH_SKILL, Outcome::Pass, 0),
                trial("t1", CONTROL, Outcome::Fail, 1),
                trial("t1", WITH_SKILL, Outcome::Fail, 1),
                trial("t1", CONTROL, Outcome::Pass, 0),
            ],
        );
        let summary = report.summary_json();
        assert_eq!(summary["interpretation"]["status"], "comparison_eligible");
        assert_eq!(
            summary["interpretation"]["descriptive_comparison_eligible"],
            true
        );
        assert_eq!(summary["interpretation"]["causal_claim_eligible"], false);
        assert!(report.markdown().contains("not a causal proof"));
    }

    #[test]
    fn unsandboxed_host_execution_is_always_non_comparative() {
        let mut report = report(
            1,
            2,
            vec![
                trial("t1", WITH_SKILL, Outcome::Pass, 0),
                trial("t1", CONTROL, Outcome::Fail, 1),
                trial("t1", WITH_SKILL, Outcome::Fail, 1),
                trial("t1", CONTROL, Outcome::Pass, 0),
            ],
        );
        report.provenance.execution_mode = "unsandboxed_host_execution".into();
        report.provenance.unsandboxed_host_execution = true;

        let summary = report.summary_json();
        assert_eq!(summary["interpretation"]["status"], "non_comparative");
        assert_eq!(
            summary["interpretation"]["descriptive_comparison_eligible"],
            false
        );
        assert_eq!(summary["interpretation"]["causal_claim_eligible"], false);
        assert!(summary["overall"]["observed_delta"].is_object());
        let markdown = report.markdown();
        assert!(markdown.contains("NON-COMPARATIVE"));
        assert!(markdown.contains("forensic review"));
        assert!(!markdown.contains("**Observed delta**"), "{markdown}");
    }

    #[test]
    fn input_drift_is_non_comparative_even_for_mock() {
        let mut report = report(
            1,
            2,
            vec![
                trial("t1", WITH_SKILL, Outcome::Pass, 0),
                trial("t1", CONTROL, Outcome::Fail, 1),
                trial("t1", WITH_SKILL, Outcome::Fail, 1),
                trial("t1", CONTROL, Outcome::Pass, 0),
            ],
        );
        report.input_integrity = InputIntegrity {
            drift_detected: true,
            events: vec!["fixture hash changed".into()],
        };

        let summary = report.summary_json();
        assert_eq!(summary["interpretation"]["status"], "non_comparative");
        assert_eq!(summary["input_integrity"]["drift_detected"], true);
        assert!(report.markdown().contains("DRIFT DETECTED"));
    }
}
