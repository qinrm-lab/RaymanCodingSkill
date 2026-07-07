//! 结果聚合与报告渲染。核心指标：A 组（有技能）相对 B 组（无技能）的通过率差。

use std::collections::BTreeMap;

use serde::Serialize;

pub const WITH_SKILL: &str = "with_skill";
pub const CONTROL: &str = "control";

#[derive(Debug, Clone, Serialize)]
pub struct TrialResult {
    pub task: String,
    pub condition: String,
    pub trial: usize,
    pub passed: bool,
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
    passed: usize,
    total: usize,
}

impl CellStat {
    fn rate(&self) -> f64 {
        if self.total == 0 {
            0.0
        } else {
            self.passed as f64 / self.total as f64
        }
    }
}

impl EvalReport {
    fn cell(&self, task: &str, condition: &str) -> CellStat {
        let mut stat = CellStat {
            passed: 0,
            total: 0,
        };
        for result in &self.results {
            if result.task == task && result.condition == condition {
                stat.total += 1;
                if result.passed {
                    stat.passed += 1;
                }
            }
        }
        stat
    }

    fn tasks(&self) -> Vec<String> {
        let mut names: Vec<String> = self.results.iter().map(|r| r.task.clone()).collect();
        names.sort();
        names.dedup();
        names
    }

    fn overall(&self, condition: &str) -> CellStat {
        let mut stat = CellStat {
            passed: 0,
            total: 0,
        };
        for result in &self.results {
            if result.condition == condition {
                stat.total += 1;
                if result.passed {
                    stat.passed += 1;
                }
            }
        }
        stat
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

    /// 机器可读摘要。
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
                    "with_skill": {"passed": w.passed, "total": w.total, "rate": w.rate()},
                    "control": {"passed": c.passed, "total": c.total, "rate": c.rate()},
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
                "with_skill": {"passed": with.passed, "total": with.total},
                "control": {"passed": control.passed, "total": control.total},
            },
            "avg_rayman_invocations_with_skill": self.avg_rayman(WITH_SKILL),
            "per_task": per_task,
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
                "| {} | {}/{} ({:.0}%) | {}/{} ({:.0}%) |\n",
                task,
                w.passed,
                w.total,
                w.rate() * 100.0,
                c.passed,
                c.total,
                c.rate() * 100.0,
            ));
        }
        let with = self.overall(WITH_SKILL);
        let control = self.overall(CONTROL);
        out.push_str(&format!(
            "| **Overall** | **{}/{} ({:.0}%)** | **{}/{} ({:.0}%)** |\n\n",
            with.passed,
            with.total,
            with.rate() * 100.0,
            control.passed,
            control.total,
            control.rate() * 100.0,
        ));
        let delta = (with.rate() - control.rate()) * 100.0;
        out.push_str(&format!(
            "**Skill effect: {delta:+.0} percentage points** (with-skill minus control).\n\n"
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

    fn trial(task: &str, condition: &str, passed: bool) -> TrialResult {
        TrialResult {
            task: task.into(),
            condition: condition.into(),
            trial: 0,
            passed,
            grade_exit: if passed { 0 } else { 1 },
            steps: 1,
            tool_calls: 1,
            rayman_invocations: if condition == WITH_SKILL { 2 } else { 0 },
            finished: true,
            error: None,
        }
    }

    #[test]
    fn aggregates_pass_rates_and_delta() {
        let report = EvalReport {
            model: "mock".into(),
            trials_per_cell: 1,
            results: vec![
                trial("t1", WITH_SKILL, true),
                trial("t1", CONTROL, false),
                trial("t2", WITH_SKILL, true),
                trial("t2", CONTROL, true),
            ],
        };
        let summary = report.summary_json();
        assert_eq!(summary["overall"]["with_skill_rate"], 1.0);
        assert_eq!(summary["overall"]["control_rate"], 0.5);
        assert_eq!(summary["overall"]["delta"], 0.5);
        assert!(report.markdown().contains("Skill effect: +50"));
    }
}
