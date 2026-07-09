//! 最小目标契约 + 待完成项续接。
//!
//! 只保留真正有用的那一条门禁：**关闭为 success 时，每个 `must` 需求都必须带证据**。
//! 砍掉 counterexample_challenges / search_effort / claim_ledger 等仪式化元数据。

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::state_store::{now_iso, read_json, write_json};

const GOALS_DIR: &str = ".RaymanCodingSkill/goals";
const PENDING_PATH: &str = ".RaymanCodingSkill/pending.json";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Requirement {
    pub id: String,
    pub text: String,
    #[serde(default = "must_kind")]
    pub kind: String, // must | should
    #[serde(default = "open_status")]
    pub status: String, // open | done
    #[serde(default)]
    pub evidence: Option<String>,
    #[serde(default)]
    pub validations: Vec<ValidationEvidence>,
    #[serde(default)]
    pub impacts: Vec<ImpactEvidence>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ValidationEvidence {
    pub command: String,
    pub recorded_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ImpactEvidence {
    pub changed_path: String,
    pub direct_dependencies: Vec<String>,
    pub direct_dependents: Vec<String>,
    pub candidate_tests: Vec<String>,
    pub recommended_checks: Vec<String>,
    pub recommendation_basis: String,
    pub recorded_at: String,
}

fn must_kind() -> String {
    "must".into()
}
fn open_status() -> String {
    "open".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Goal {
    pub id: String,
    pub title: String,
    pub status: String, // active | success | partial | blocked
    pub created_at: String,
    pub updated_at: String,
    pub requirements: Vec<Requirement>,
    #[serde(default, skip)]
    pub loaded_from_legacy: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LegacyGoal {
    id: String,
    status: String,
    #[serde(default)]
    created_at: Option<String>,
    #[serde(default)]
    updated_at: Option<String>,
    contract: LegacyContract,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LegacyContract {
    goal: String,
    #[serde(default)]
    requirements: Vec<LegacyRequirement>,
    #[serde(default)]
    verification: Vec<String>,
    #[serde(default)]
    created_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LegacyRequirement {
    id: String,
    text: String,
    #[serde(default = "must_kind")]
    priority: String,
    #[serde(default = "open_status")]
    status: String,
    #[serde(default)]
    evidence: Option<String>,
    #[serde(default)]
    validation_commands: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct GoalLoadIssue {
    pub path: String,
    pub error: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PendingList {
    pub items: Vec<PendingItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingItem {
    pub id: String,
    pub title: String,
    pub detail: String,
    pub created_at: String,
}

pub struct GoalStore {
    root: PathBuf,
}

fn short_id(prefix: &str, seed: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(seed.as_bytes());
    let digest = format!("{:x}", hasher.finalize());
    format!("{prefix}_{}", &digest[..10])
}

impl GoalStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn goal_path(&self, id: &str) -> PathBuf {
        self.root.join(GOALS_DIR).join(format!("{id}.json"))
    }

    /// 新建目标。`requirements` 为 (text, is_must) 列表。
    pub fn start(&self, title: &str, requirements: &[(String, bool)]) -> Result<Goal> {
        let now = now_iso();
        let id = short_id("goal", &format!("{title}{now}"));
        let requirements = requirements
            .iter()
            .enumerate()
            .map(|(index, (text, is_must))| Requirement {
                id: format!("req_{}", index + 1),
                text: text.clone(),
                kind: if *is_must { "must" } else { "should" }.into(),
                status: "open".into(),
                evidence: None,
                validations: Vec::new(),
                impacts: Vec::new(),
            })
            .collect();
        let goal = Goal {
            id: id.clone(),
            title: title.into(),
            status: "active".into(),
            created_at: now.clone(),
            updated_at: now,
            requirements,
            loaded_from_legacy: false,
        };
        write_json(&self.goal_path(&id), &goal)?;
        Ok(goal)
    }

    pub fn get(&self, id: &str) -> Result<Option<Goal>> {
        Self::load_goal_file(&self.goal_path(id))
    }

    pub fn list(&self) -> Result<Vec<Goal>> {
        let (goals, _) = self.list_with_issues()?;
        Ok(goals)
    }

    pub fn list_with_issues(&self) -> Result<(Vec<Goal>, Vec<GoalLoadIssue>)> {
        let dir = self.root.join(GOALS_DIR);
        let mut goals = Vec::new();
        let mut issues = Vec::new();
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok((goals, issues));
            }
            Err(error) => {
                issues.push(GoalLoadIssue {
                    path: dir.display().to_string(),
                    error: error.to_string(),
                });
                return Ok((goals, issues));
            }
        };
        for entry_result in entries {
            let entry = match entry_result {
                Ok(entry) => entry,
                Err(error) => {
                    issues.push(GoalLoadIssue {
                        path: dir.display().to_string(),
                        error: error.to_string(),
                    });
                    continue;
                }
            };
            if entry.path().extension().and_then(|ext| ext.to_str()) == Some("json") {
                match Self::load_goal_file(&entry.path()) {
                    Ok(Some(goal)) => goals.push(goal),
                    Ok(None) => {}
                    Err(error) => issues.push(GoalLoadIssue {
                        path: entry.path().display().to_string(),
                        error: error.to_string(),
                    }),
                }
            }
        }
        goals.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        Ok((goals, issues))
    }

    fn load_goal_file(path: &Path) -> Result<Option<Goal>> {
        let Some(value) = read_json::<serde_json::Value>(path)? else {
            return Ok(None);
        };
        match serde_json::from_value::<Goal>(value.clone()) {
            Ok(mut goal) => {
                goal.loaded_from_legacy = false;
                Ok(Some(goal))
            }
            Err(current_error) => match serde_json::from_value::<LegacyGoal>(value) {
                Ok(legacy) => Ok(Some(goal_from_legacy(legacy))),
                Err(legacy_error) => bail!(
                    "无法解析 goal 文件: current schema: {current_error}; legacy schema: {legacy_error}"
                ),
            },
        }
    }

    /// 记录某个需求的证据并标记完成。
    pub fn record_evidence(&self, id: &str, req_id: &str, evidence: &str) -> Result<Goal> {
        self.record_evidence_with_context(id, req_id, evidence, Vec::new(), Vec::new())
    }

    /// 记录某个需求的证据、验证命令和变更影响快照，并标记完成。
    pub fn record_evidence_with_context(
        &self,
        id: &str,
        req_id: &str,
        evidence: &str,
        validation_commands: Vec<String>,
        impacts: Vec<ImpactEvidence>,
    ) -> Result<Goal> {
        let Some(mut goal) = self.get(id)? else {
            bail!("目标不存在: {id}");
        };
        let Some(req) = goal.requirements.iter_mut().find(|req| req.id == req_id) else {
            bail!("需求不存在: {req_id}");
        };
        let now = now_iso();
        req.evidence = Some(evidence.into());
        req.validations = validation_commands
            .into_iter()
            .map(|command| ValidationEvidence {
                command,
                recorded_at: now.clone(),
            })
            .collect();
        req.impacts = impacts;
        req.status = "done".into();
        goal.updated_at = now;
        write_json(&self.goal_path(id), &goal)?;
        Ok(goal)
    }

    /// 关闭目标。status=success 时，每个 must 需求必须已带证据，否则拒绝。
    pub fn close(&self, id: &str, status: &str) -> Result<Goal> {
        let Some(mut goal) = self.get(id)? else {
            bail!("目标不存在: {id}");
        };
        if status == "success" {
            let missing: Vec<&str> = goal
                .requirements
                .iter()
                .filter(|req| {
                    req.kind == "must" && req.evidence.as_deref().unwrap_or("").is_empty()
                })
                .map(|req| req.id.as_str())
                .collect();
            if !missing.is_empty() {
                bail!(
                    "拒绝关闭为 success：以下 must 需求缺少证据: {}。用 `rayman goal evidence` 记录后再关闭。",
                    missing.join(", ")
                );
            }
        }
        goal.status = status.into();
        goal.updated_at = now_iso();
        write_json(&self.goal_path(id), &goal)?;
        Ok(goal)
    }
}

fn goal_from_legacy(legacy: LegacyGoal) -> Goal {
    let created_at = legacy
        .created_at
        .or_else(|| legacy.contract.created_at.clone())
        .unwrap_or_default();
    let updated_at = legacy.updated_at.unwrap_or_else(|| created_at.clone());
    let requirements = legacy
        .contract
        .requirements
        .into_iter()
        .map(|req| {
            let validation_commands = if req.validation_commands.is_empty() {
                legacy.contract.verification.clone()
            } else {
                req.validation_commands
            };
            Requirement {
                id: req.id,
                text: req.text,
                kind: req.priority,
                status: match req.status.as_str() {
                    "satisfied" => "done".into(),
                    other => other.into(),
                },
                evidence: req.evidence,
                validations: validation_commands
                    .into_iter()
                    .map(|command| ValidationEvidence {
                        command,
                        recorded_at: updated_at.clone(),
                    })
                    .collect(),
                impacts: Vec::new(),
            }
        })
        .collect();
    Goal {
        id: legacy.id,
        title: legacy.contract.goal,
        status: legacy.status,
        created_at,
        updated_at,
        requirements,
        loaded_from_legacy: true,
    }
}

pub struct PendingStore {
    root: PathBuf,
}

impl PendingStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn path(&self) -> PathBuf {
        self.root.join(PENDING_PATH)
    }

    fn load(&self) -> PendingList {
        read_json(&self.path()).ok().flatten().unwrap_or_default()
    }

    pub fn list(&self) -> Vec<PendingItem> {
        self.load().items
    }

    pub fn add(&self, title: &str, detail: &str) -> Result<PendingItem> {
        let mut list = self.load();
        let now = now_iso();
        let item = PendingItem {
            id: short_id("pending", &format!("{title}{now}{}", list.items.len())),
            title: title.into(),
            detail: detail.into(),
            created_at: now,
        };
        list.items.push(item.clone());
        write_json(&self.path(), &list)?;
        Ok(item)
    }

    pub fn resolve(&self, id: &str) -> Result<bool> {
        let mut list = self.load();
        let before = list.items.len();
        list.items.retain(|item| item.id != id);
        let removed = list.items.len() != before;
        if removed {
            write_json(&self.path(), &list)?;
        }
        Ok(removed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn close_success_requires_evidence_for_must_requirements() {
        let dir = tempfile::tempdir().unwrap();
        let store = GoalStore::new(dir.path());
        let goal = store
            .start(
                "add parser",
                &[
                    ("implement parser".into(), true),
                    ("nice errors".into(), false),
                ],
            )
            .unwrap();

        // 缺 must 证据 → 拒绝 success。
        assert!(store.close(&goal.id, "success").is_err());
        // partial 允许。
        assert_eq!(store.close(&goal.id, "partial").unwrap().status, "partial");

        // 记录 must 证据后允许 success（should 无需证据）。
        store
            .record_evidence(&goal.id, "req_1", "src/parser.rs + cargo test passed")
            .unwrap();
        let closed = store.close(&goal.id, "success").unwrap();
        assert_eq!(closed.status, "success");
    }

    #[test]
    fn pending_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let store = PendingStore::new(dir.path());
        let item = store.add("finish gate", "wire up CI").unwrap();
        assert_eq!(store.list().len(), 1);
        assert!(store.resolve(&item.id).unwrap());
        assert!(store.list().is_empty());
    }
}
