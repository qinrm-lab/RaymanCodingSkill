//! 最小目标契约 + 待完成项续接。
//!
//! 只保留真正有用的那一条门禁：**关闭为 success 时，每个 `must` 需求都必须带证据**。
//! 砍掉 counterexample_challenges / search_effort / claim_ledger 等仪式化元数据。

use std::collections::BTreeSet;
use std::fmt;
use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, SystemTime};

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::state_paths;
use crate::state_store::{now_iso, read_json, write_json};

const GOALS_DIR: &str = ".RaymanCodingSkill/goals";
#[cfg(test)]
const PENDING_PATH: &str = ".RaymanCodingSkill/pending.json";
const GOALS_RELATIVE: &str = "goals";
const PENDING_RELATIVE: &str = "pending.json";
pub const GOAL_SCHEMA_VERSION: u32 = 2;

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RequirementKind {
    #[default]
    Must,
    Should,
}

impl RequirementKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Must => "must",
            Self::Should => "should",
        }
    }
}

impl fmt::Display for RequirementKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RequirementStatus {
    #[default]
    Open,
    Done,
}

impl RequirementStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Done => "done",
        }
    }
}

impl fmt::Display for RequirementStatus {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum GoalStatus {
    Active,
    Success,
    Partial,
    Blocked,
}

impl GoalStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Success => "success",
            Self::Partial => "partial",
            Self::Blocked => "blocked",
        }
    }
}

impl fmt::Display for GoalStatus {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Requirement {
    pub id: String,
    pub text: String,
    #[serde(default)]
    pub kind: RequirementKind,
    #[serde(default)]
    pub status: RequirementStatus,
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
    /// 旧的人工声明没有实际退出码/工作区绑定，只能作为迁移信息保留。
    #[serde(default)]
    pub receipt: Option<ValidationReceipt>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ValidationReceipt {
    pub exit_code: i32,
    pub cwd: String,
    pub workspace_fingerprint_before: String,
    pub workspace_fingerprint_after: String,
    pub stdout_sha256: String,
    pub stderr_sha256: String,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Goal {
    #[serde(default)]
    pub schema_version: u32,
    pub id: String,
    pub title: String,
    pub status: GoalStatus,
    pub created_at: String,
    pub updated_at: String,
    pub requirements: Vec<Requirement>,
    #[serde(default, skip)]
    pub loaded_from_legacy: bool,
}

impl Goal {
    pub fn is_current_schema(&self) -> bool {
        self.schema_version == GOAL_SCHEMA_VERSION && !self.loaded_from_legacy
    }

    /// Validate the persisted contract before a readiness gate trusts it.  The
    /// CLI cannot be the only enforcement point: a hand-written or corrupted
    /// JSON file can otherwise claim `success` with no mandatory requirement.
    /// Legacy records are deliberately handled by the migration branch in the
    /// caller and are not reinterpreted as v2.
    pub fn current_schema_error(&self) -> Option<String> {
        if self.loaded_from_legacy {
            return None;
        }
        if self.schema_version != GOAL_SCHEMA_VERSION {
            return Some(format!(
                "不支持的 goal schema_version={}（当前只接受 v{}；请迁移或重新创建目标）",
                self.schema_version, GOAL_SCHEMA_VERSION
            ));
        }
        if self.id.trim().is_empty() || self.title.trim().is_empty() {
            return Some("goal id 或标题为空".into());
        }
        let mut ids = BTreeSet::new();
        let mut must_count = 0usize;
        for requirement in &self.requirements {
            if requirement.id.trim().is_empty() || requirement.text.trim().is_empty() {
                return Some("goal 包含空的 requirement id 或文本".into());
            }
            if !ids.insert(requirement.id.as_str()) {
                return Some(format!("goal 包含重复 requirement id: {}", requirement.id));
            }
            if requirement.kind == RequirementKind::Must {
                must_count += 1;
            }
        }
        if must_count == 0 {
            return Some("goal 至少需要一个 must 需求".into());
        }
        None
    }
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
    #[serde(default = "legacy_must_kind")]
    priority: String,
    #[serde(default = "legacy_open_status")]
    status: String,
    #[serde(default)]
    evidence: Option<String>,
    #[serde(default)]
    validation_commands: Vec<String>,
}

fn legacy_must_kind() -> String {
    "must".into()
}

fn legacy_open_status() -> String {
    "open".into()
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

/// 进程间互斥：原子 rename 只能避免半写，不能避免两个 agent 的 read-modify-write
/// 相互覆盖。锁文件只保护极短的状态事务；异常退出后的旧锁会在宽限期后被回收。
struct StateLock {
    path: PathBuf,
}

impl Drop for StateLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn acquire_state_lock(target: &Path) -> Result<StateLock> {
    let parent = target.parent().unwrap_or_else(|| Path::new("."));
    state_paths::ensure_real_directory(parent)?;
    let name = target
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("state");
    let lock_path = parent.join(format!(".{name}.rayman.lock"));
    const ATTEMPTS: usize = 100;
    const STALE_AFTER: Duration = Duration::from_secs(300);
    for _ in 0..ATTEMPTS {
        match OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&lock_path)
        {
            Ok(mut file) => {
                use std::io::Write;
                let _ = writeln!(file, "pid={}", std::process::id());
                return Ok(StateLock { path: lock_path });
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let stale = fs::metadata(&lock_path)
                    .and_then(|metadata| metadata.modified())
                    .and_then(|modified| {
                        SystemTime::now()
                            .duration_since(modified)
                            .map_err(std::io::Error::other)
                    })
                    .map(|age| age >= STALE_AFTER)
                    .unwrap_or(false);
                if stale {
                    let _ = fs::remove_file(&lock_path);
                    continue;
                }
                thread::sleep(Duration::from_millis(25));
            }
            Err(error) => return Err(error.into()),
        }
    }
    bail!("状态正在被另一个 rayman 进程修改: {}", target.display())
}

impl GoalStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// id 直接拼进文件名；拒绝分隔符等字符，防止 `--id ../../x` 越出 goals 目录读写。
    fn goal_path(&self, id: &str) -> Result<PathBuf> {
        if id.is_empty()
            || !id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            bail!("非法目标 id: {id}（只允许字母、数字、下划线和连字符）");
        }
        state_paths::managed_state_file(
            &self.root,
            &Path::new(GOALS_RELATIVE).join(format!("{id}.json")),
            false,
        )
    }

    /// 新建目标。`requirements` 为 (text, is_must) 列表。
    pub fn start(&self, title: &str, requirements: &[(String, bool)]) -> Result<Goal> {
        if title.trim().is_empty() {
            bail!("目标标题不能为空");
        }
        if !requirements
            .iter()
            .any(|(text, is_must)| *is_must && !text.trim().is_empty())
        {
            bail!("新目标至少需要一个非空 --must 需求");
        }
        let goals_dir =
            state_paths::managed_state_dir(&self.root, Path::new(GOALS_RELATIVE), true)?
                .ok_or_else(|| anyhow::anyhow!("无法创建目标状态目录"))?;
        let _lock = acquire_state_lock(&goals_dir.join(".store"))?;
        let now = now_iso();
        let id = short_id("goal", &format!("{title}{now}"));
        let requirements = requirements
            .iter()
            .enumerate()
            .map(|(index, (text, is_must))| Requirement {
                id: format!("req_{}", index + 1),
                text: text.clone(),
                kind: if *is_must {
                    RequirementKind::Must
                } else {
                    RequirementKind::Should
                },
                status: RequirementStatus::Open,
                evidence: None,
                validations: Vec::new(),
                impacts: Vec::new(),
            })
            .collect();
        let goal = Goal {
            schema_version: GOAL_SCHEMA_VERSION,
            id: id.clone(),
            title: title.into(),
            status: GoalStatus::Active,
            created_at: now.clone(),
            updated_at: now,
            requirements,
            loaded_from_legacy: false,
        };
        write_json(&self.goal_path(&id)?, &goal)?;
        Ok(goal)
    }

    pub fn get(&self, id: &str) -> Result<Option<Goal>> {
        Self::load_goal_file(&self.goal_path(id)?)
    }

    pub fn list(&self) -> Result<Vec<Goal>> {
        let (goals, _) = self.list_with_issues()?;
        Ok(goals)
    }

    pub fn list_with_issues(&self) -> Result<(Vec<Goal>, Vec<GoalLoadIssue>)> {
        let mut goals = Vec::new();
        let mut issues = Vec::new();
        let dir = match state_paths::managed_state_dir(&self.root, Path::new(GOALS_RELATIVE), false)
        {
            Ok(Some(dir)) => dir,
            Ok(None) => return Ok((goals, issues)),
            Err(error) => {
                issues.push(GoalLoadIssue {
                    path: self.root.join(GOALS_DIR).display().to_string(),
                    error: error.to_string(),
                });
                return Ok((goals, issues));
            }
        };
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
            let name = entry.file_name();
            let listed_path = dir.join(&name);
            if Path::new(&name).extension().and_then(|ext| ext.to_str()) == Some("json") {
                // `read_dir` returns a lexical entry path.  Do not read it
                // directly: a goal file can itself be a symlink/junction even
                // when the goals directory was verified above.
                let path = match state_paths::managed_state_file(
                    &self.root,
                    &Path::new(GOALS_RELATIVE).join(&name),
                    false,
                ) {
                    Ok(path) => path,
                    Err(error) => {
                        issues.push(GoalLoadIssue {
                            path: listed_path.display().to_string(),
                            error: error.to_string(),
                        });
                        continue;
                    }
                };
                match Self::load_goal_file(&path) {
                    Ok(Some(goal)) => goals.push(goal),
                    Ok(None) => {}
                    Err(error) => issues.push(GoalLoadIssue {
                        path: listed_path.display().to_string(),
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
                // Earlier lean goals were already serialized in a shape close
                // to `Goal`, but had no schema marker.  Mutating one of those
                // records writes schema_version=0; keep treating that exact
                // migration shape as legacy on every later load.  A nonzero
                // unknown version remains a current-format incompatibility
                // and is rejected by the standard gate instead of silently
                // downgraded to history.
                goal.loaded_from_legacy = goal.schema_version == 0;
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
        let path = self.goal_path(id)?;
        let _lock = acquire_state_lock(&path)?;
        let Some(mut goal) = Self::load_goal_file(&path)? else {
            bail!("目标不存在: {id}");
        };
        let Some(req) = goal.requirements.iter_mut().find(|req| req.id == req_id) else {
            bail!("需求不存在: {req_id}");
        };
        let now = now_iso();
        req.evidence = Some(evidence.into());
        // 追加而非覆写：补记一条说明不应销毁先前的验证与影响面审计记录。
        for command in validation_commands {
            if !req.validations.iter().any(|v| v.command == command) {
                req.validations.push(ValidationEvidence {
                    command,
                    recorded_at: now.clone(),
                    receipt: None,
                });
            }
        }
        req.impacts.extend(impacts);
        req.status = RequirementStatus::Done;
        goal.updated_at = now;
        write_json(&path, &goal)?;
        Ok(goal)
    }

    /// 记录由 rayman 实际执行后生成的验证 receipt。只有 current schema 的 success
    /// 目标可把这种 receipt 当作 standard/release 证据。
    pub fn record_validation_receipt(
        &self,
        id: &str,
        req_id: &str,
        evidence: &str,
        command: String,
        receipt: ValidationReceipt,
        impacts: Vec<ImpactEvidence>,
    ) -> Result<Goal> {
        if evidence.trim().is_empty() {
            bail!("验证证据说明不能为空");
        }
        let path = self.goal_path(id)?;
        let _lock = acquire_state_lock(&path)?;
        let Some(mut goal) = Self::load_goal_file(&path)? else {
            bail!("目标不存在: {id}");
        };
        if !goal.is_current_schema() {
            bail!("目标 {id} 不是当前 schema，不能写入可验证 receipt；请新建目标");
        }
        let Some(req) = goal.requirements.iter_mut().find(|req| req.id == req_id) else {
            bail!("需求不存在: {req_id}");
        };
        let now = now_iso();
        req.evidence = Some(evidence.into());
        req.validations.push(ValidationEvidence {
            command,
            recorded_at: now.clone(),
            receipt: Some(receipt),
        });
        req.impacts.extend(impacts);
        req.status = RequirementStatus::Done;
        goal.updated_at = now;
        write_json(&path, &goal)?;
        Ok(goal)
    }

    /// 关闭目标。status=success 时，每个 must 需求必须已带证据，否则拒绝。
    pub fn close(&self, id: &str, status: &str) -> Result<Goal> {
        let status = match status {
            "success" => GoalStatus::Success,
            "partial" => GoalStatus::Partial,
            "blocked" => GoalStatus::Blocked,
            _ => {
                bail!("未知的关闭状态: {status}（可用: success | partial | blocked）");
            }
        };
        let path = self.goal_path(id)?;
        let _lock = acquire_state_lock(&path)?;
        let Some(mut goal) = Self::load_goal_file(&path)? else {
            bail!("目标不存在: {id}");
        };
        if status == GoalStatus::Success {
            let must: Vec<_> = goal
                .requirements
                .iter()
                .filter(|req| req.kind == RequirementKind::Must)
                .collect();
            if must.is_empty() {
                bail!("拒绝关闭为 success：目标必须至少包含一个 must 需求");
            }
            let missing: Vec<&str> = must
                .iter()
                .filter(|req| {
                    req.status != RequirementStatus::Done
                        || req.evidence.as_deref().unwrap_or("").trim().is_empty()
                })
                .map(|req| req.id.as_str())
                .collect();
            if !missing.is_empty() {
                bail!(
                    "拒绝关闭为 success：以下 must 需求未完成或缺少证据: {}。",
                    missing.join(", ")
                );
            }
        }
        goal.status = status;
        goal.updated_at = now_iso();
        write_json(&path, &goal)?;
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
                kind: match req.priority.as_str() {
                    "must" => RequirementKind::Must,
                    "should" => RequirementKind::Should,
                    other => {
                        // Legacy data must never silently downgrade an unknown mandatory kind.
                        // Keep it as must so standard check can require migration/receipt.
                        let _ = other;
                        RequirementKind::Must
                    }
                },
                status: match req.status.as_str() {
                    "satisfied" | "done" => RequirementStatus::Done,
                    _ => RequirementStatus::Open,
                },
                evidence: req.evidence,
                validations: validation_commands
                    .into_iter()
                    .map(|command| ValidationEvidence {
                        command,
                        recorded_at: updated_at.clone(),
                        receipt: None,
                    })
                    .collect(),
                impacts: Vec::new(),
            }
        })
        .collect();
    Goal {
        schema_version: 0,
        id: legacy.id,
        title: legacy.contract.goal,
        status: match legacy.status.as_str() {
            "success" => GoalStatus::Success,
            "partial" => GoalStatus::Partial,
            "blocked" => GoalStatus::Blocked,
            _ => GoalStatus::Active,
        },
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

    fn path(&self, create_parents: bool) -> Result<PathBuf> {
        state_paths::managed_state_file(&self.root, Path::new(PENDING_RELATIVE), create_parents)
    }

    /// 损坏的 pending.json 必须报错：静默当空列表会让 check 放行，
    /// 且下一次 add/resolve 的写回会用空列表覆盖销毁原有数据。
    fn load(&self) -> Result<PendingList> {
        Ok(read_json(&self.path(false)?)?.unwrap_or_default())
    }

    pub fn list(&self) -> Result<Vec<PendingItem>> {
        Ok(self.load()?.items)
    }

    pub fn add(&self, title: &str, detail: &str) -> Result<PendingItem> {
        let path = self.path(true)?;
        let _lock = acquire_state_lock(&path)?;
        let mut list = self.load()?;
        let now = now_iso();
        let item = PendingItem {
            id: short_id("pending", &format!("{title}{now}{}", list.items.len())),
            title: title.into(),
            detail: detail.into(),
            created_at: now,
        };
        list.items.push(item.clone());
        write_json(&path, &list)?;
        Ok(item)
    }

    pub fn resolve(&self, id: &str) -> Result<bool> {
        let path = self.path(true)?;
        let _lock = acquire_state_lock(&path)?;
        let mut list = self.load()?;
        let before = list.items.len();
        list.items.retain(|item| item.id != id);
        let removed = list.items.len() != before;
        if removed {
            write_json(&path, &list)?;
        }
        Ok(removed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn successful_receipt() -> ValidationReceipt {
        ValidationReceipt {
            exit_code: 0,
            cwd: "fixture".into(),
            workspace_fingerprint_before: "before".into(),
            workspace_fingerprint_after: "after".into(),
            stdout_sha256: "a".repeat(64),
            stderr_sha256: "b".repeat(64),
        }
    }

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
        assert_eq!(
            store.close(&goal.id, "partial").unwrap().status,
            GoalStatus::Partial
        );

        // 关闭语义只要求 must evidence；standard/release 会另外要求 receipt。
        store
            .record_evidence(&goal.id, "req_1", "src/parser.rs + cargo test passed")
            .unwrap();
        let closed = store.close(&goal.id, "success").unwrap();
        assert_eq!(closed.status, GoalStatus::Success);
        // receipt carries the stronger evidence required by standard/release.
        store
            .record_validation_receipt(
                &goal.id,
                "req_1",
                "cargo test passed",
                "cargo test".into(),
                successful_receipt(),
                Vec::new(),
            )
            .unwrap();
    }

    #[test]
    fn start_rejects_empty_must_contract() {
        let dir = tempfile::tempdir().unwrap();
        let store = GoalStore::new(dir.path());
        assert!(store.start("empty", &[]).is_err());
        assert!(store.start("empty", &[(" ".into(), true)]).is_err());
    }

    #[test]
    fn current_schema_contract_rejects_forged_or_empty_must_requirements() {
        let dir = tempfile::tempdir().unwrap();
        let store = GoalStore::new(dir.path());
        let goal = store
            .start("task", &[("must prove it".into(), true)])
            .unwrap();
        assert_eq!(goal.current_schema_error(), None);

        let mut zero_must = goal.clone();
        zero_must.requirements.clear();
        assert!(
            zero_must
                .current_schema_error()
                .is_some_and(|error| error.contains("must"))
        );

        let mut unknown_version = goal;
        unknown_version.schema_version = GOAL_SCHEMA_VERSION + 1;
        assert!(
            unknown_version
                .current_schema_error()
                .is_some_and(|error| error.contains("schema_version"))
        );
    }

    #[test]
    fn pending_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let store = PendingStore::new(dir.path());
        let item = store.add("finish gate", "wire up CI").unwrap();
        assert_eq!(store.list().unwrap().len(), 1);
        assert!(store.resolve(&item.id).unwrap());
        assert!(store.list().unwrap().is_empty());
    }

    #[test]
    fn concurrent_pending_and_goal_writes_do_not_lose_records() {
        use std::collections::BTreeSet;
        use std::sync::{Arc, Barrier};

        const WORKERS: usize = 8;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();

        let pending_barrier = Arc::new(Barrier::new(WORKERS + 1));
        let pending_handles: Vec<_> = (0..WORKERS)
            .map(|index| {
                let root = root.clone();
                let barrier = Arc::clone(&pending_barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    PendingStore::new(root)
                        .add(&format!("pending {index}"), "parallel regression")
                        .unwrap();
                })
            })
            .collect();
        pending_barrier.wait();
        for handle in pending_handles {
            handle.join().unwrap();
        }
        let pending = PendingStore::new(&root).list().unwrap();
        assert_eq!(pending.len(), WORKERS);
        assert_eq!(
            pending
                .iter()
                .map(|item| &item.id)
                .collect::<BTreeSet<_>>()
                .len(),
            WORKERS
        );

        let requirements: Vec<_> = (0..WORKERS)
            .map(|index| (format!("must {index}"), true))
            .collect();
        let goal = GoalStore::new(&root)
            .start("parallel goal", &requirements)
            .unwrap();
        let goal_barrier = Arc::new(Barrier::new(WORKERS + 1));
        let goal_handles: Vec<_> = (0..WORKERS)
            .map(|index| {
                let root = root.clone();
                let id = goal.id.clone();
                let barrier = Arc::clone(&goal_barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    GoalStore::new(root)
                        .record_evidence(
                            &id,
                            &format!("req_{}", index + 1),
                            &format!("parallel evidence {index}"),
                        )
                        .unwrap();
                })
            })
            .collect();
        goal_barrier.wait();
        for handle in goal_handles {
            handle.join().unwrap();
        }
        let persisted = GoalStore::new(&root).get(&goal.id).unwrap().unwrap();
        assert!(
            persisted
                .requirements
                .iter()
                .all(|requirement| requirement.status == RequirementStatus::Done)
        );
    }

    #[test]
    fn corrupt_pending_store_errors_instead_of_wiping() {
        let dir = tempfile::tempdir().unwrap();
        let store = PendingStore::new(dir.path());
        store.add("keep me", "important").unwrap();
        let path = dir.path().join(PENDING_PATH);
        std::fs::write(&path, "{ not json").unwrap();

        // 损坏文件必须报错，且 add/resolve 不得覆盖原文件。
        assert!(store.list().is_err());
        assert!(store.add("new", "item").is_err());
        assert!(store.resolve("pending_x").is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ not json");
    }

    #[test]
    fn close_rejects_unknown_status_and_traversal_ids() {
        let dir = tempfile::tempdir().unwrap();
        let store = GoalStore::new(dir.path());
        let goal = store.start("task", &[("req".into(), true)]).unwrap();

        // 未知状态（含大小写/拼写错误）不得绕过证据门禁。
        assert!(store.close(&goal.id, "done").is_err());
        assert!(store.close(&goal.id, "Success").is_err());

        // id 含路径分隔符/.. 时拒绝，防止越出 goals 目录。
        assert!(store.get("../../x").is_err());
        assert!(store.close("..\\evil", "partial").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn list_with_issues_rejects_a_linked_goal_file() {
        use std::os::unix::fs::symlink;

        let workspace = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let goals = workspace.path().join(GOALS_DIR);
        fs::create_dir_all(&goals).unwrap();
        let external_goal = outside.path().join("external.json");
        fs::write(&external_goal, r#"{"id":"external","title":"outside"}"#).unwrap();
        symlink(&external_goal, goals.join("external.json")).unwrap();

        let (goals, issues) = GoalStore::new(workspace.path()).list_with_issues().unwrap();
        assert!(goals.is_empty());
        assert_eq!(issues.len(), 1);
        assert!(issues[0].error.contains("链接/reparse"));
    }
}
