//! Caller-owned readiness capture.
//!
//! One complete workspace walk supplies the bytes used by context, assets,
//! maps, and goal baselines. Mutable workflow state is captured separately
//! through the same handle-bound primitive and retained in a raw seal. A
//! terminal capture can therefore reject ordinary net drift without claiming
//! a global lock or protection from an exact A -> B -> A cycle.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use anyhow::{Context, Result, bail};

use crate::context::{self, ContextIndex, FileEntry};
use crate::file_io::{
    FileIdentity, is_link_or_reparse, read_handle_bound_file, read_optional_handle_bound_file,
};
use crate::goal::{self, Goal, GoalLoadIssue, PendingReadiness, WorkspaceBaseline};
use crate::hash::sha256_bytes;
use crate::source_state::{self, SourceState};
use crate::state_paths;
use crate::timefmt::now_iso;
use crate::walk::{relative_key, workspace_files_checked};
use crate::workspace;

const ACTIVATION_STATE_PATH: &str = "workspace_skill.yaml";
const CONTEXT_INDEX_STATE_PATH: &str = "context/index.json";
const GOALS_STATE_PATH: &str = "goals";
const PENDING_STATE_PATH: &str = "pending.json";

#[derive(Debug, Clone, PartialEq, Eq)]
struct FileSeal {
    sha256: String,
    identity: FileIdentity,
}

impl FileSeal {
    fn new(bytes: &[u8], identity: FileIdentity) -> Self {
        Self {
            sha256: sha256_bytes(bytes),
            identity,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadinessStateSeal {
    workspace: BTreeMap<String, FileSeal>,
    state_present: bool,
    activation_config: Option<FileSeal>,
    activation_skill: Option<ActivationSkillSeal>,
    goals: BTreeMap<String, FileSeal>,
    goals_error: Option<String>,
    pending: Option<FileSeal>,
    context_index: Option<FileSeal>,
    source: SourceState,
    maintenance_artifacts: BTreeMap<String, FileSeal>,
}

#[derive(Debug, Clone)]
pub struct ReadinessCapture {
    root: PathBuf,
    workspace_bytes: BTreeMap<String, Vec<u8>>,
    context_entries: Vec<FileEntry>,
    state_present: bool,
    goals: Vec<Goal>,
    goal_load_issues: Vec<GoalLoadIssue>,
    pending_bytes: Option<Vec<u8>>,
    context_index_bytes: Option<Vec<u8>>,
    activation_config_bytes: Option<Vec<u8>>,
    activation_skill: Option<workspace::CapturedActivationSkill>,
    source: SourceState,
    baseline: WorkspaceBaseline,
    maintenance_artifact_hashes: BTreeMap<String, String>,
    workspace_identity: String,
    seal: ReadinessStateSeal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ActivationSkillSeal {
    recorded_path: String,
    resolved_path: String,
    file: Option<FileSeal>,
    error: Option<String>,
}

type CapturedGoals = (
    Vec<Goal>,
    Vec<GoalLoadIssue>,
    BTreeMap<String, FileSeal>,
    Option<String>,
);

impl ReadinessCapture {
    pub fn capture(root: &Path) -> Result<Self> {
        let root = root
            .canonicalize()
            .with_context(|| format!("无法规范化 readiness workspace: {}", root.display()))?;

        let mut workspace_bytes = BTreeMap::new();
        let mut context_entries = Vec::new();
        let mut workspace_seal = BTreeMap::new();
        for path in workspace_files_checked(&root)? {
            context::ensure_source_file(&root, &path)?;
            let (bytes, identity) = read_handle_bound_file(&path, "readiness workspace file")?;
            context::ensure_source_file(&root, &path)?;
            let key = relative_key(&root, &path);
            if workspace_bytes.contains_key(&key) {
                bail!("readiness workspace capture 包含重复路径: {key}");
            }
            let mtime_ns = identity
                .modified
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map(|duration| duration.as_nanos())
                .unwrap_or(0);
            let entry = context::build_entry_from_captured_bytes(
                &root,
                &path,
                identity.len,
                mtime_ns,
                &bytes,
            )?;
            workspace_seal.insert(key.clone(), FileSeal::new(&bytes, identity));
            workspace_bytes.insert(key, bytes);
            context_entries.push(entry);
        }
        context_entries.sort_by(|left, right| left.path.cmp(&right.path));

        let baseline_files = workspace_bytes
            .iter()
            .map(|(path, bytes)| (path.clone(), sha256_bytes(bytes)))
            .collect::<BTreeMap<_, _>>();
        let baseline = WorkspaceBaseline {
            recorded_at: now_iso(),
            workspace_fingerprint: goal::fingerprint_for_files(&baseline_files),
            files: baseline_files,
        };

        let state_present = state_paths::managed_state_root(&root, false)?.is_some();
        let activation_path =
            state_paths::managed_state_file(&root, Path::new(ACTIVATION_STATE_PATH), false)?;
        let activation_capture =
            read_optional_handle_bound_file(&activation_path, "readiness workspace_skill.yaml")?;
        let (activation_config_bytes, activation_config_seal) = split_optional(activation_capture);
        let captured_activation_skill = activation_config_bytes
            .as_deref()
            .map(|bytes| workspace::capture_activation_skill(&root, bytes))
            .transpose()?
            .flatten();
        let (activation_skill, activation_skill_seal) = match captured_activation_skill {
            Some((skill, identity)) => {
                let file = match (skill.bytes.as_deref(), identity) {
                    (Some(bytes), Some(identity)) => {
                        if let Ok(relative) = skill.resolved_path.strip_prefix(&root) {
                            let key = relative.to_string_lossy().replace('\\', "/");
                            if let Some(workspace_file) = workspace_seal.get(&key)
                                && *workspace_file != FileSeal::new(bytes, identity.clone())
                            {
                                bail!("activation skill 与同轮 workspace capture 不一致: {key}");
                            }
                        }
                        Some(FileSeal::new(bytes, identity))
                    }
                    (None, None) => None,
                    _ => bail!("activation skill capture identity/bytes 结构不一致"),
                };
                let seal = ActivationSkillSeal {
                    recorded_path: skill.recorded_path.clone(),
                    resolved_path: skill.resolved_path.to_string_lossy().into_owned(),
                    file,
                    error: skill.error.clone(),
                };
                (Some(skill), Some(seal))
            }
            None => (None, None),
        };

        let (goals, goal_load_issues, goals_seal, goals_error) = capture_goals(&root)?;
        let pending_path =
            state_paths::managed_state_file(&root, Path::new(PENDING_STATE_PATH), false)?;
        let (pending_bytes, pending_seal) = split_optional(read_optional_handle_bound_file(
            &pending_path,
            "readiness pending.json",
        )?);
        let context_index_path =
            state_paths::managed_state_file(&root, Path::new(CONTEXT_INDEX_STATE_PATH), false)?;
        let (context_index_bytes, context_index_seal) = split_optional(
            read_optional_handle_bound_file(&context_index_path, "readiness context index")?,
        );

        let (maintenance_artifact_hashes, maintenance_artifact_seals) =
            capture_maintenance_artifacts(&root, &goals)?;
        let source = source_state::inspect(&root);
        let seal = ReadinessStateSeal {
            workspace: workspace_seal,
            state_present,
            activation_config: activation_config_seal,
            activation_skill: activation_skill_seal,
            goals: goals_seal,
            goals_error,
            pending: pending_seal,
            context_index: context_index_seal,
            source: source.clone(),
            maintenance_artifacts: maintenance_artifact_seals,
        };
        let workspace_identity = crate::context::workspace_identity_from_canonical_root(&root);
        Ok(Self {
            root,
            workspace_bytes,
            context_entries,
            state_present,
            goals,
            goal_load_issues,
            pending_bytes,
            context_index_bytes,
            activation_config_bytes,
            activation_skill,
            source,
            baseline,
            maintenance_artifact_hashes,
            workspace_identity,
            seal,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn workspace_bytes(&self) -> &BTreeMap<String, Vec<u8>> {
        &self.workspace_bytes
    }

    pub fn context_entries(&self) -> &[FileEntry] {
        &self.context_entries
    }

    pub fn state_present(&self) -> bool {
        self.state_present
    }

    pub fn captured_files(&self) -> impl Iterator<Item = (&FileEntry, &[u8])> {
        self.context_entries.iter().map(|entry| {
            let bytes = self
                .workspace_bytes
                .get(&entry.path)
                .expect("capture entries and bytes are constructed together");
            (entry, bytes.as_slice())
        })
    }

    pub fn captured_context_index(&self) -> Result<Option<ContextIndex>> {
        self.context_index_bytes
            .as_deref()
            .map(|bytes| {
                serde_json::from_slice(bytes).context("无法解析 captured context/index.json")
            })
            .transpose()
    }

    /// Verify the captured context index against the exact complete workspace
    /// entries owned by this decision capture. No state or workspace path is
    /// reopened by this operation.
    pub fn verify_context(&self) -> (context::FreshnessReport, Option<ContextIndex>) {
        context::verify_index_from_capture(
            &self.root,
            self.captured_context_index(),
            &self.context_entries,
        )
    }

    pub fn goals(&self) -> &[Goal] {
        &self.goals
    }

    pub fn goal_load_issues(&self) -> &[GoalLoadIssue] {
        &self.goal_load_issues
    }

    pub fn pending_readiness(&self) -> Result<PendingReadiness> {
        goal::pending_readiness_from_captured_bytes(self.pending_bytes.as_deref(), &self.goals)
    }

    pub fn activation_config_bytes(&self) -> Option<&[u8]> {
        self.activation_config_bytes.as_deref()
    }

    pub fn activation_skill(&self) -> Option<&workspace::CapturedActivationSkill> {
        self.activation_skill.as_ref()
    }

    pub fn source(&self) -> &SourceState {
        &self.source
    }

    pub fn baseline(&self) -> &WorkspaceBaseline {
        &self.baseline
    }

    pub fn goal_decision_context(&self) -> goal::GoalDecisionContext<'_> {
        goal::GoalDecisionContext::captured_with_readiness_state(
            &self.root,
            Some(&self.baseline),
            &self.workspace_bytes,
            &self.source,
            &self.maintenance_artifact_hashes,
            &self.workspace_identity,
        )
    }

    pub fn maintenance_artifact_hashes(&self) -> &BTreeMap<String, String> {
        &self.maintenance_artifact_hashes
    }

    pub fn seal(&self) -> &ReadinessStateSeal {
        &self.seal
    }

    /// Publish an index derived from this capture, then bind the newly written
    /// state file back into the same decision seal without another workspace
    /// walk. A malformed captured cache still refuses the refresh.
    pub fn refresh_context(&mut self) -> Result<context::RefreshReport> {
        let captured_cached = self.captured_context_index();
        let (expected, report) = context::refresh_from_capture(
            &self.root,
            self.context_entries.clone(),
            captured_cached,
        )?;
        let path = state_paths::managed_state_file(
            &self.root,
            Path::new(CONTEXT_INDEX_STATE_PATH),
            false,
        )?;
        let (bytes, identity) = read_handle_bound_file(&path, "refreshed readiness context index")?;
        let observed: ContextIndex = serde_json::from_slice(&bytes)
            .context("无法解析刚发布的 captured context/index.json")?;
        if observed != expected {
            bail!("context index 在 captured refresh 发布后发生变化");
        }
        self.context_index_bytes = Some(bytes.clone());
        // `refresh_from_capture` may create `.RaymanCodingSkill` for a
        // previously stateless workspace. The rest of this same decision must
        // observe that new state-root presence, and the terminal capture must
        // not report a synthetic activation-domain drift caused by stale
        // in-memory presence.
        self.state_present = true;
        self.seal.state_present = true;
        self.seal.context_index = Some(FileSeal::new(&bytes, identity));
        Ok(report)
    }
}

fn split_optional(capture: Option<(Vec<u8>, FileIdentity)>) -> (Option<Vec<u8>>, Option<FileSeal>) {
    match capture {
        Some((bytes, identity)) => {
            let seal = FileSeal::new(&bytes, identity);
            (Some(bytes), Some(seal))
        }
        None => (None, None),
    }
}

fn valid_goal_file_name(name: &str) -> bool {
    let Some(stem) = name.strip_suffix(".json") else {
        return false;
    };
    !stem.is_empty()
        && stem
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn valid_goal_lock_name(name: &str) -> bool {
    if name == "..store.rayman.lock" {
        return true;
    }
    name.strip_prefix('.')
        .and_then(|name| name.strip_suffix(".rayman.lock"))
        .is_some_and(valid_goal_file_name)
}

fn listed_goal_members(dir: &Path) -> Result<Vec<String>> {
    let mut names = Vec::new();
    for entry in std::fs::read_dir(dir)
        .with_context(|| format!("无法枚举 readiness goals 目录: {}", dir.display()))?
    {
        let entry = entry
            .with_context(|| format!("readiness goals 目录成员枚举失败: {}", dir.display()))?;
        let name = entry.file_name();
        let name = name
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("readiness goals 目录含非 UTF-8 成员: {:?}", name))?;
        if !valid_goal_file_name(name) && !valid_goal_lock_name(name) {
            bail!("readiness goals 目录含未知成员: {name}");
        }
        let metadata = std::fs::symlink_metadata(entry.path())
            .with_context(|| format!("无法读取 readiness goals 成员元数据: {name}"))?;
        if is_link_or_reparse(&metadata) || !metadata.file_type().is_file() {
            bail!("readiness goals 成员必须是普通非链接文件: {name}");
        }
        names.push(name.to_string());
    }
    names.sort();
    Ok(names)
}

fn capture_goals(root: &Path) -> Result<CapturedGoals> {
    let dir = match state_paths::managed_state_dir(root, Path::new(GOALS_STATE_PATH), false) {
        Ok(Some(dir)) => dir,
        Ok(None) => return Ok((Vec::new(), Vec::new(), BTreeMap::new(), None)),
        Err(error) => {
            let path = root.join(".RaymanCodingSkill").join(GOALS_STATE_PATH);
            let rendered = format!("{error:#}");
            let issue = GoalLoadIssue {
                path: path.display().to_string(),
                error: rendered.clone(),
            };
            let mut seals = BTreeMap::new();
            // If the directory slot is occupied by an ordinary file, bind its
            // exact bytes/identity too. Symlink/reparse and other unreadable
            // invalid states remain represented by `goals_error`, so a
            // valid<->invalid transition is always visible to changed_sections.
            if let Ok((bytes, identity)) =
                read_handle_bound_file(&path, "invalid readiness goals store path")
            {
                seals.insert(
                    "<invalid-goals-store>".to_string(),
                    FileSeal::new(&bytes, identity),
                );
            }
            return Ok((Vec::new(), vec![issue], seals, Some(rendered)));
        }
    };
    let first_members = listed_goal_members(&dir)?;
    let mut goals = Vec::new();
    let mut issues = Vec::new();
    let mut seals = BTreeMap::new();
    let mut ids = BTreeSet::new();
    for name in &first_members {
        let path = dir.join(name);
        let (bytes, identity) = read_handle_bound_file(&path, "readiness goal directory member")?;
        seals.insert(name.clone(), FileSeal::new(&bytes, identity));
        if !valid_goal_file_name(name) {
            continue;
        }
        match goal::goal_from_captured_bytes(&bytes) {
            Ok(goal) => {
                let expected = format!("{}.json", goal.id);
                if expected != *name {
                    bail!(
                        "readiness goal 文件名与内部 id 不匹配: file={name} id={}",
                        goal.id
                    );
                }
                if !ids.insert(goal.id.clone()) {
                    bail!("readiness goals 含重复 goal id: {}", goal.id);
                }
                goals.push(goal);
            }
            Err(error) => issues.push(GoalLoadIssue {
                path: path.display().to_string(),
                error: format!("{error:#}"),
            }),
        }
    }
    let second_members = listed_goal_members(&dir)?;
    if first_members != second_members {
        bail!("readiness goals 目录成员在捕获期间发生变化");
    }
    goals.sort_by(|left, right| left.created_at.cmp(&right.created_at));
    Ok((goals, issues, seals, None))
}

fn capture_maintenance_artifacts(
    root: &Path,
    goals: &[Goal],
) -> Result<(BTreeMap<String, String>, BTreeMap<String, FileSeal>)> {
    let mut hashes = BTreeMap::new();
    let mut seals = BTreeMap::new();
    let mut comparison_keys = BTreeMap::<String, String>::new();
    for rebind in goals
        .iter()
        .filter_map(|goal| goal.replacement_authority.as_ref())
        .filter_map(|proof| proof.live_authority.command_rebind.as_ref())
    {
        let path = goal::verified_maintenance_cycle_rebind_path(root, rebind)?;
        let key = rebind.current_value.clone();
        let comparison_key = if cfg!(windows) {
            key.to_ascii_lowercase()
        } else {
            key.clone()
        };
        if let Some(previous) = comparison_keys.insert(comparison_key, key.clone())
            && previous != key
        {
            bail!("maintenance artifact 路径别名冲突: {previous} vs {key}");
        }
        if hashes.contains_key(&key) {
            continue;
        }
        let (bytes, identity) =
            read_handle_bound_file(&path, "readiness maintenance cycle artifact")?;
        let current_sha256 = sha256_bytes(&bytes);
        if current_sha256 != rebind.current_sha256 {
            bail!("maintenance cycle rebind 文件 hash 已漂移: {key}");
        }
        hashes.insert(key.clone(), current_sha256);
        seals.insert(key, FileSeal::new(&bytes, identity));
    }
    Ok((hashes, seals))
}

pub fn changed_sections(
    before: &ReadinessStateSeal,
    after: &ReadinessStateSeal,
) -> Vec<&'static str> {
    let mut changed = Vec::new();
    if before.workspace != after.workspace {
        changed.push("workspace");
    }
    if before.state_present != after.state_present
        || before.activation_config != after.activation_config
        || before.activation_skill != after.activation_skill
    {
        changed.push("activation");
    }
    if before.goals != after.goals || before.goals_error != after.goals_error {
        changed.push("goals");
    }
    if before.pending != after.pending {
        changed.push("pending");
    }
    if before.context_index != after.context_index {
        changed.push("context");
    }
    if before.source != after.source {
        changed.push("source");
    }
    if before.maintenance_artifacts != after.maintenance_artifacts {
        changed.push("maintenance_artifacts");
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn goal_member_names_are_strict() {
        assert!(valid_goal_file_name("goal_123-a.json"));
        assert!(valid_goal_lock_name("..store.rayman.lock"));
        assert!(valid_goal_lock_name(".goal_123-a.json.rayman.lock"));
        assert!(!valid_goal_file_name("goal.json.tmp"));
        assert!(!valid_goal_file_name("../goal.json"));
        assert!(!valid_goal_lock_name(".unknown.rayman.lock"));
    }

    #[test]
    fn changed_sections_names_raw_state_domains() {
        let empty_source = SourceState {
            kind: "not_repository".into(),
            available: false,
            clean: None,
            head: None,
            tracked_dirty: 0,
            untracked: 0,
            path_encoding_lossy: false,
            changed_paths: Vec::new(),
            porcelain_sha256: None,
            error: None,
        };
        let before = ReadinessStateSeal {
            workspace: BTreeMap::new(),
            state_present: false,
            activation_config: None,
            activation_skill: None,
            goals: BTreeMap::new(),
            goals_error: None,
            pending: None,
            context_index: None,
            source: empty_source.clone(),
            maintenance_artifacts: BTreeMap::new(),
        };
        let mut after = before.clone();
        after.source.kind = "git".into();
        assert_eq!(changed_sections(&before, &after), ["source"]);
    }
}
