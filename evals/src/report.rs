//! 结果聚合与报告渲染。核心指标：A 组（有技能）相对 B 组（无技能）的通过率差。
//!
//! pass/fail/error 三分：error 是基础设施问题（工作区准备失败、后端故障、响应截断），
//! 不进通过率分母（分母 = pass + fail），单列在报告里，避免把环境问题算到模型头上。

use std::collections::BTreeMap;

use serde::Serialize;

pub const WITH_SKILL: &str = "with_skill";
pub const CONTROL: &str = "control";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    Pass,
    Fail,
    Error,
}

#[derive(Debug, Clone, Serialize)]
pub struct TrialResult {
    pub task: String,
    pub condition: String,
    pub trial: usize,
    pub outcome: Outcome,
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
    pub results: Vec<TrialResult>,
}

#[derive(Debug, Clone, Serialize)]
struct CellStat {
    pass: usize,
    fail: usize,
    error: usize,
}

impl CellStat {
    fn rate(&self) -> f64 {
        let graded = self.pass + self.fail;
        if graded == 0 {
            0.0
        } else {
            self.pass as f64 / graded as f64
        }
    }

    fn json(&self) -> serde_json::Value {
        serde_json::json!({
            "pass": self.pass,
            "fail": self.fail,
            "error": self.error,
            "rate": self.rate(),
        })
    }

    fn markdown(&self) -> String {
        let mut text = format!(
            "{}/{} ({:.0}%)",
            self.pass,
            self.pass + self.fail,
            self.rate() * 100.0
        );
        if self.error > 0 {
            text.push_str(&format!(" +{}err", self.error));
        }
        text
    }
}

impl EvalReport {
    fn stat(&self, mut keep: impl FnMut(&TrialResult) -> bool) -> CellStat {
        let mut stat = CellStat {
            pass: 0,
            fail: 0,
            error: 0,
        };
        for result in self.results.iter().filter(|r| keep(r)) {
            match result.outcome {
                Outcome::Pass => stat.pass += 1,
                Outcome::Fail => stat.fail += 1,
                Outcome::Error => stat.error += 1,
            }
        }
        stat
    }

    fn cell(&self, task: &str, condition: &str) -> CellStat {
        self.stat(|r| r.task == task && r.condition == condition)
    }

    fn overall(&self, condition: &str) -> CellStat {
        self.stat(|r| r.condition == condition)
    }

    fn tasks(&self) -> Vec<String> {
        let mut names: Vec<String> = self.results.iter().map(|r| r.task.clone()).collect();
        names.sort();
        names.dedup();
        names
    }

    fn avg_rayman(&self, condition: &str) -> f64 {
        let cells: Vec<usize> = self
            .results
            .iter()
            .filter(|r| r.condition == condition)
            .map(|r| r.rayman_invocations)
            .collect();
        if cells.is_empty() {
            0.0
        } else {
            cells.iter().sum::<usize>() as f64 / cells.len() as f64
        }
    }

    /// 机器可读摘要（含 per-trial 明细，便于事后排查基础设施错误）。
    pub fn summary_json(&self) -> serde_json::Value {
        let with = self.overall(WITH_SKILL);
        let control = self.overall(CONTROL);
        let mut per_task = BTreeMap::new();
        for task in self.tasks() {
            let w = self.cell(&task, WITH_SKILL);
            let c = self.cell(&task, CONTROL);
            per_task.insert(
                task,
                serde_json::json!({
                    "with_skill": w.json(),
                    "control": c.json(),
                }),
            );
        }
        serde_json::json!({
            "model": self.model,
            "trials_per_cell": self.trials_per_cell,
            "overall": {
                "with_skill_rate": with.rate(),
                "control_rate": control.rate(),
                "delta": with.rate() - control.rate(),
                "with_skill": with.json(),
                "control": control.json(),
            },
            "avg_rayman_invocations_with_skill": self.avg_rayman(WITH_SKILL),
            "per_task": per_task,
            "trials": self.results,
        })
    }

    /// 人类可读 Markdown 报告。
    pub fn markdown(&self) -> String {
        let mut out = String::new();
        out.push_str("# RaymanCodingSkill A/B outcome eval\n\n");
        out.push_str(&format!(
            "- Model: `{}`\n- Trials per cell: {}\n\n",
            self.model, self.trials_per_cell
        ));
        out.push_str("| Task | With skill | Control |\n|---|---|---|\n");
        for task in self.tasks() {
            let w = self.cell(&task, WITH_SKILL);
            let c = self.cell(&task, CONTROL);
            out.push_str(&format!(
                "| {} | {} | {} |\n",
                task,
                w.markdown(),
                c.markdown()
            ));
        }
        let with = self.overall(WITH_SKILL);
        let control = self.overall(CONTROL);
        out.push_str(&format!(
            "| **Overall** | **{}** | **{}** |\n\n",
            with.markdown(),
            control.markdown()
        ));
        let delta = (with.rate() - control.rate()) * 100.0;
        let error_note = if with.error + control.error > 0 {
            format!(
                "; infrastructure errors excluded from rates: with_skill={}, control={}",
                with.error, control.error
            )
        } else {
            String::new()
        };
        out.push_str(&format!(
            "**Skill effect: {delta:+.0} percentage points** (with-skill minus control{error_note}).\n\n"
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

    fn trial(task: &str, condition: &str, outcome: Outcome) -> TrialResult {
        TrialResult {
            task: task.into(),
            condition: condition.into(),
            trial: 0,
            outcome,
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

    #[test]
    fn aggregates_pass_rates_and_delta() {
        let report = EvalReport {
            model: "mock".into(),
            trials_per_cell: 1,
            results: vec![
                trial("t1", WITH_SKILL, Outcome::Pass),
                trial("t1", CONTROL, Outcome::Fail),
                trial("t2", WITH_SKILL, Outcome::Pass),
                trial("t2", CONTROL, Outcome::Pass),
            ],
        };
        let summary = report.summary_json();
        assert_eq!(summary["overall"]["with_skill_rate"], 1.0);
        assert_eq!(summary["overall"]["control_rate"], 0.5);
        assert_eq!(summary["overall"]["delta"], 0.5);
        assert!(report.markdown().contains("Skill effect: +50"));
    }

    #[test]
    fn errors_stay_out_of_rate_denominator_and_land_in_details() {
        let report = EvalReport {
            model: "mock".into(),
            trials_per_cell: 2,
            results: vec![
                trial("t1", WITH_SKILL, Outcome::Pass),
                trial("t1", WITH_SKILL, Outcome::Error),
                trial("t1", CONTROL, Outcome::Fail),
                trial("t1", CONTROL, Outcome::Fail),
            ],
        };
        let summary = report.summary_json();
        // error 不摊分母：with_skill 1 pass / (1 pass + 0 fail) = 100%。
        assert_eq!(summary["overall"]["with_skill_rate"], 1.0);
        assert_eq!(summary["overall"]["with_skill"]["error"], 1);
        assert_eq!(summary["overall"]["control"]["fail"], 2);
        // per-trial 明细落盘。
        let trials = summary["trials"].as_array().unwrap();
        assert_eq!(trials.len(), 4);
        assert_eq!(trials[1]["outcome"], "error");
        assert_eq!(trials[1]["error"], "backend down");
        // delta 旁标注 error 数。
        let md = report.markdown();
        assert!(md.contains("with_skill=1, control=0"), "{md}");
        assert!(md.contains("+1err"), "{md}");
    }
}
