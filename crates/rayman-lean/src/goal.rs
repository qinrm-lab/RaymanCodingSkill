//! 最小目标契约 + 待完成项续接。
//!
//! 只保留真正有用的那一条门禁：**关闭为 success 时，每个 `must` 需求都必须带证据**。
//! 砍掉 counterexample_challenges / search_effort / claim_ledger 等仪式化元数据。

use std::path::PathBuf;

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::fsutil::{now_iso, read_json, write_json};

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
            })
            .collect();
        let goal = Goal {
            id: id.clone(),
            title: title.into(),
            status: "active".into(),
            created_at: now.clone(),
            updated_at: now,
            requirements,
        };
        write_json(&self.goal_path(&id), &goal)?;
        Ok(goal)
    }

    pub fn get(&self, id: &str) -> Result<Option<Goal>> {
        read_json(&self.goal_path(id))
    }

    pub fn list(&self) -> Result<Vec<Goal>> {
        let dir = self.root.join(GOALS_DIR);
        let mut goals = Vec::new();
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return Ok(goals);
        };
        for entry in entries.flatten() {
            if entry.path().extension().and_then(|ext| ext.to_str()) == Some("json") {
                // 单个损坏文件不拖垮列表。
                if let Ok(Some(goal)) = read_json::<Goal>(&entry.path()) {
                    goals.push(goal);
                }
            }
        }
        goals.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        Ok(goals)
    }

    /// 记录某个需求的证据并标记完成。
    pub fn record_evidence(&self, id: &str, req_id: &str, evidence: &str) -> Result<Goal> {
        let Some(mut goal) = self.get(id)? else {
            bail!("目标不存在: {id}");
        };
        let Some(req) = goal.requirements.iter_mut().find(|req| req.id == req_id) else {
            bail!("需求不存在: {req_id}");
        };
        req.evidence = Some(evidence.into());
        req.status = "done".into();
        goal.updated_at = now_iso();
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
                    "拒绝关闭为 success：以下 must 需求缺少证据: {}。用 `rayman-lean goal evidence` 记录后再关闭。",
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
