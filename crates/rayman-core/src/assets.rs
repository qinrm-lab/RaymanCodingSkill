use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use chrono::{NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

use crate::{display_path, ensure_within, now_iso, sha256_file, write_text};

const STATE_VERSION: u32 = 1;
const STATE_RELATIVE_PATH: &str = ".RaymanCodingSkill/assets/retirement.json";
const SOURCE_POLICY: &str = "Obsolete assets are not current-behavior evidence. Current files are authoritative, but recorded obsolete assets must be retired or explicitly exempted before success.";
const IGNORED_ROOTS: &[&str] = &[
    ".git",
    "target",
    "node_modules",
    "dist",
    "build",
    "__pycache__",
    ".tmp",
    "logs",
    ".RaymanCodingSkill/context",
    ".RaymanCodingSkill/tmp",
    ".RaymanCodingSkill/regression",
    ".RaymanCodingSkill/release",
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum AssetStatus {
    #[default]
    RetirementCandidate,
    Retired,
    CompatibilityExempt,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AssetReference {
    pub path: String,
    pub line: usize,
    pub sha256: Option<String>,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ObsoleteAssetRecord {
    pub path: String,
    #[serde(default)]
    pub line: Option<usize>,
    #[serde(default)]
    pub sha256: Option<String>,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub stale_behavior: String,
    #[serde(default)]
    pub replacement_behavior: String,
    #[serde(default)]
    pub deletion_reason: String,
    #[serde(default)]
    pub risk: String,
    #[serde(default)]
    pub validation_command: String,
    #[serde(default)]
    pub references: Vec<AssetReference>,
    #[serde(default)]
    pub retention_reason: Option<String>,
    #[serde(default)]
    pub expires_at: Option<String>,
    #[serde(default)]
    pub status: AssetStatus,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
    #[serde(default)]
    pub retired_at: Option<String>,
}

impl Default for ObsoleteAssetRecord {
    fn default() -> Self {
        Self {
            path: String::new(),
            line: None,
            sha256: None,
            kind: String::new(),
            stale_behavior: String::new(),
            replacement_behavior: String::new(),
            deletion_reason: String::new(),
            risk: String::new(),
            validation_command: String::new(),
            references: Vec::new(),
            retention_reason: None,
            expires_at: None,
            status: AssetStatus::RetirementCandidate,
            created_at: String::new(),
            updated_at: String::new(),
            retired_at: None,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AssetRetirementReport {
    pub workspace_path: String,
    pub generated_at: String,
    pub state_path: String,
    pub controller_scope: String,
    pub ignored_roots: Vec<String>,
    pub blockers: Vec<String>,
    pub candidates: Vec<ObsoleteAssetRecord>,
    pub retired_present: Vec<ObsoleteAssetRecord>,
    pub exemptions: Vec<ObsoleteAssetRecord>,
    pub cleanup_plan: Vec<AssetCleanupPlanItem>,
    pub detected_references: Vec<AssetReference>,
    pub records: Vec<ObsoleteAssetRecord>,
    pub required_actions: Vec<String>,
    pub source_policy: String,
}

impl AssetRetirementReport {
    pub fn has_blockers(&self) -> bool {
        !self.blockers.is_empty()
    }

    pub fn non_current_paths(&self) -> BTreeSet<String> {
        self.records
            .iter()
            .filter(|record| {
                matches!(
                    record.status,
                    AssetStatus::RetirementCandidate
                        | AssetStatus::Retired
                        | AssetStatus::CompatibilityExempt
                )
            })
            .map(|record| record.path.clone())
            .collect()
    }
}

#[derive(Debug, Clone)]
pub struct AssetRetireRequest {
    pub path: PathBuf,
    pub replacement_behavior: String,
    pub deletion_reason: String,
    pub validation_command: String,
    pub apply_delete: bool,
}

#[derive(Debug, Clone)]
pub struct AssetExemptRequest {
    pub path: PathBuf,
    pub retention_reason: String,
    pub expires_at: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AssetCleanupPlanItem {
    pub path: String,
    pub status: String,
    pub action: String,
    pub reason: String,
    pub reference_count: usize,
    pub manifest_required: bool,
}

#[derive(Debug, Clone, Default)]
pub struct AssetCleanupRequest {
    pub apply: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AssetState {
    version: u32,
    #[serde(default)]
    records: Vec<ObsoleteAssetRecord>,
}

impl Default for AssetState {
    fn default() -> Self {
        Self {
            version: STATE_VERSION,
            records: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct AssetRetirementManager {
    workspace: PathBuf,
    state_path: PathBuf,
}

impl AssetRetirementManager {
    pub fn new(workspace: impl Into<PathBuf>) -> Result<Self> {
        let workspace = workspace
            .into()
            .canonicalize()
            .context("unable to resolve workspace path")?;
        let state_path = workspace.join(STATE_RELATIVE_PATH);
        Ok(Self {
            workspace,
            state_path,
        })
    }

    pub fn state_path(&self) -> &Path {
        &self.state_path
    }

    pub fn status(&self) -> Result<AssetRetirementReport> {
        let state = self.read_state()?;
        self.report_for_records(state.records)
    }

    pub fn scan(&self) -> Result<AssetRetirementReport> {
        let mut state = self.read_state()?;
        let report = self.report_for_records(state.records.clone())?;
        let now = now_iso();
        for record in &mut state.records {
            if let Some(scanned) = report.records.iter().find(|item| item.path == record.path) {
                record.references = scanned.references.clone();
                record.updated_at = now.clone();
            }
        }
        self.write_state(&state)?;
        self.report_for_records(state.records)
    }

    pub fn cleanup(&self, request: AssetCleanupRequest) -> Result<AssetRetirementReport> {
        let mut state = self.read_state()?;
        let report = self.report_for_records(state.records.clone())?;
        if !request.apply {
            return Ok(report);
        }
        let now = now_iso();
        let mut deleted = BTreeSet::new();
        for item in &report.cleanup_plan {
            if item.action != "delete_file" {
                continue;
            }
            let Some(record) = state
                .records
                .iter_mut()
                .find(|record| record.path == item.path)
            else {
                continue;
            };
            if !matches!(record.status, AssetStatus::RetirementCandidate) {
                continue;
            }
            let target = ensure_within(
                Path::new(&record.path),
                &self.workspace,
                "asset cleanup path escaped workspace",
            )?;
            if !target.exists() {
                record.status = AssetStatus::Retired;
                record.updated_at = now.clone();
                record.retired_at = Some(now.clone());
                record.references.clear();
                deleted.insert(record.path.clone());
                continue;
            }
            if !target.is_file() {
                bail!(
                    "asset cleanup refuses recursive directory deletion: {}",
                    record.path
                );
            }
            fs::remove_file(&target).with_context(|| {
                format!("unable to delete obsolete asset: {}", target.display())
            })?;
            record.status = AssetStatus::Retired;
            record.updated_at = now.clone();
            record.retired_at = Some(now.clone());
            record.references.clear();
            deleted.insert(record.path.clone());
        }
        self.write_state(&state)?;
        let mut next = self.report_for_records(state.records)?;
        if deleted.is_empty() {
            next.required_actions.push(
                "No eligible registered obsolete asset files were deleted by cleanup --apply."
                    .into(),
            );
        }
        Ok(next)
    }

    pub fn retire(&self, request: AssetRetireRequest) -> Result<AssetRetirementReport> {
        if request.replacement_behavior.trim().is_empty() {
            bail!("asset retirement requires --replacement");
        }
        if request.deletion_reason.trim().is_empty() {
            bail!("asset retirement requires --reason");
        }
        if request.validation_command.trim().is_empty() {
            bail!("asset retirement requires --validation-command");
        }
        let target = ensure_within(
            &request.path,
            &self.workspace,
            "asset path escaped workspace",
        )?;
        let path = relative_path(&self.workspace, &target);
        let mut state = self.read_state()?;
        let now = now_iso();
        let sha256 = target.is_file().then(|| sha256_file(&target)).transpose()?;
        let references = self.references_for_path(&path, Some(&target))?;
        upsert_record(
            &mut state.records,
            ObsoleteAssetRecord {
                path: path.clone(),
                line: None,
                sha256,
                kind: asset_kind(&path).into(),
                stale_behavior: "asset identified as obsolete by retirement request".into(),
                replacement_behavior: request.replacement_behavior,
                deletion_reason: request.deletion_reason,
                risk: "whole-file retirement can break callers if stale references remain".into(),
                validation_command: request.validation_command,
                references,
                retention_reason: None,
                expires_at: None,
                status: AssetStatus::RetirementCandidate,
                created_at: now.clone(),
                updated_at: now.clone(),
                retired_at: None,
            },
        );

        if request.apply_delete {
            let references = self.references_for_path(&path, Some(&target))?;
            if !references.is_empty() {
                self.write_state(&state)?;
                bail!(
                    "asset retirement blocked: {} is still referenced by {} current files",
                    path,
                    references.len()
                );
            }
            if !target.exists() {
                bail!("asset retirement requested deletion but path is missing: {path}");
            }
            if !target.is_file() {
                bail!("asset retirement can delete only whole files: {path}");
            }
            fs::remove_file(&target).with_context(|| {
                format!("unable to delete obsolete asset: {}", target.display())
            })?;
            let retired_at = now_iso();
            if let Some(record) = state.records.iter_mut().find(|record| record.path == path) {
                record.status = AssetStatus::Retired;
                record.updated_at = retired_at.clone();
                record.retired_at = Some(retired_at);
                record.references.clear();
            }
        }

        self.write_state(&state)?;
        self.status()
    }

    pub fn exempt(&self, request: AssetExemptRequest) -> Result<AssetRetirementReport> {
        if request.retention_reason.trim().is_empty() {
            bail!("asset exemption requires --reason");
        }
        parse_expiry(&request.expires_at)?;
        let target = ensure_within(
            &request.path,
            &self.workspace,
            "asset path escaped workspace",
        )?;
        let path = relative_path(&self.workspace, &target);
        let mut state = self.read_state()?;
        let now = now_iso();
        let sha256 = target.is_file().then(|| sha256_file(&target)).transpose()?;
        let references = self.references_for_path(&path, Some(&target))?;
        upsert_record(
            &mut state.records,
            ObsoleteAssetRecord {
                path: path.clone(),
                line: None,
                sha256,
                kind: asset_kind(&path).into(),
                stale_behavior: "asset retained only for compatibility or audit".into(),
                replacement_behavior: "retained asset is excluded from current-behavior context".into(),
                deletion_reason: "temporary compatibility exemption".into(),
                risk: "retained obsolete assets can pollute project understanding if treated as current".into(),
                validation_command: "rayman assets status".into(),
                references,
                retention_reason: Some(request.retention_reason),
                expires_at: Some(request.expires_at),
                status: AssetStatus::CompatibilityExempt,
                created_at: now.clone(),
                updated_at: now,
                retired_at: None,
            },
        );
        self.write_state(&state)?;
        self.status()
    }

    pub fn assert_no_blockers(&self) -> Result<()> {
        let report = self.status()?;
        if report.has_blockers() {
            bail!(
                "obsolete asset retirement blockers: {}",
                report.blockers.join("; ")
            );
        }
        Ok(())
    }

    fn report_for_records(
        &self,
        mut records: Vec<ObsoleteAssetRecord>,
    ) -> Result<AssetRetirementReport> {
        let mut blockers = Vec::new();
        let mut candidates = Vec::new();
        let mut retired_present = Vec::new();
        let mut exemptions = Vec::new();
        let mut cleanup_plan = Vec::new();
        let mut detected_references = Vec::new();

        for record in &mut records {
            let target = match ensure_within(
                Path::new(&record.path),
                &self.workspace,
                "asset retirement record path escaped workspace",
            ) {
                Ok(target) => target,
                Err(error) => {
                    blockers.push(error.to_string());
                    cleanup_plan.push(AssetCleanupPlanItem {
                        path: record.path.clone(),
                        status: "blocked".into(),
                        action: "invalid_path".into(),
                        reason: error.to_string(),
                        reference_count: 0,
                        manifest_required: false,
                    });
                    continue;
                }
            };
            record.references = self.references_for_path(&record.path, Some(&target))?;
            detected_references.extend(record.references.clone());
            if !record.references.is_empty() {
                blockers.push(format!(
                    "obsolete asset still referenced by {} current files: {}",
                    record.references.len(),
                    record.path
                ));
            }
            match record.status {
                AssetStatus::RetirementCandidate => {
                    blockers.push(format!(
                        "retirement candidate must be deleted or exempted: {}",
                        record.path
                    ));
                    candidates.push(record.clone());
                }
                AssetStatus::Retired => {
                    if target.exists() {
                        blockers.push(format!("retired asset still exists: {}", record.path));
                        retired_present.push(record.clone());
                    }
                }
                AssetStatus::CompatibilityExempt => {
                    exemptions.push(record.clone());
                    if record
                        .retention_reason
                        .as_deref()
                        .unwrap_or_default()
                        .trim()
                        .is_empty()
                    {
                        blockers.push(format!(
                            "compatibility exemption lacks retention reason: {}",
                            record.path
                        ));
                    }
                    match expiry_state(record.expires_at.as_deref()) {
                        ExpiryState::Active => {}
                        ExpiryState::Missing => blockers.push(format!(
                            "compatibility exemption lacks expiry: {}",
                            record.path
                        )),
                        ExpiryState::Invalid => blockers.push(format!(
                            "compatibility exemption has invalid expiry: {}",
                            record.path
                        )),
                        ExpiryState::Expired => blockers
                            .push(format!("compatibility exemption expired: {}", record.path)),
                    }
                }
            }
            cleanup_plan.push(self.cleanup_plan_item(record)?);
            if let Some(stored_hash) = &record.sha256
                && target.exists()
                && target.is_file()
            {
                let current = sha256_file(&target)?;
                if current != *stored_hash {
                    blockers.push(format!(
                        "obsolete asset hash changed since retirement record: {}",
                        record.path
                    ));
                }
            }
        }

        blockers.sort();
        blockers.dedup();
        detected_references
            .sort_by(|left, right| left.path.cmp(&right.path).then(left.line.cmp(&right.line)));
        detected_references
            .dedup_by(|left, right| left.path == right.path && left.line == right.line);
        let required_actions = if blockers.is_empty() {
            vec!["No obsolete asset retirement blockers detected.".into()]
        } else {
            vec![
                "Delete retirement candidates or add explicit compatibility exemptions with expiry.".into(),
                "Remove stale docs/config/tests/entrypoint references before closing work.".into(),
                "Run rayman assets scan, rayman assets cleanup --apply when safe, and rayman audit after cleanup.".into(),
            ]
        };
        Ok(AssetRetirementReport {
            workspace_path: display_path(&self.workspace),
            generated_at: now_iso(),
            state_path: display_path(&self.state_path),
            controller_scope: self.controller_scope(),
            ignored_roots: IGNORED_ROOTS.iter().map(|value| (*value).into()).collect(),
            blockers,
            candidates,
            retired_present,
            exemptions,
            cleanup_plan,
            detected_references,
            records,
            required_actions,
            source_policy: SOURCE_POLICY.into(),
        })
    }

    fn cleanup_plan_item(&self, record: &ObsoleteAssetRecord) -> Result<AssetCleanupPlanItem> {
        let reference_count = record.references.len();
        let target = match ensure_within(
            Path::new(&record.path),
            &self.workspace,
            "asset cleanup path escaped workspace",
        ) {
            Ok(target) => target,
            Err(error) => {
                return Ok(AssetCleanupPlanItem {
                    path: record.path.clone(),
                    status: "blocked".into(),
                    action: "invalid_path".into(),
                    reason: error.to_string(),
                    reference_count,
                    manifest_required: false,
                });
            }
        };
        if !matches!(record.status, AssetStatus::RetirementCandidate) {
            return Ok(AssetCleanupPlanItem {
                path: record.path.clone(),
                status: asset_status_name(&record.status).into(),
                action: "none".into(),
                reason: "cleanup only deletes registered retirement candidates".into(),
                reference_count,
                manifest_required: false,
            });
        }
        if reference_count > 0 {
            return Ok(AssetCleanupPlanItem {
                path: record.path.clone(),
                status: "blocked".into(),
                action: "remove_references_first".into(),
                reason: "current docs/config/tests/entrypoints still reference this obsolete asset"
                    .into(),
                reference_count,
                manifest_required: false,
            });
        }
        if target.exists() && target.is_dir() {
            return Ok(AssetCleanupPlanItem {
                path: record.path.clone(),
                status: "blocked".into(),
                action: "manifest_required".into(),
                reason: "directory retirement requires an explicit per-file manifest; recursive deletion is refused".into(),
                reference_count,
                manifest_required: true,
            });
        }
        if !target.exists() || target.is_file() {
            return Ok(AssetCleanupPlanItem {
                path: record.path.clone(),
                status: "ready".into(),
                action: "delete_file".into(),
                reason: "registered retirement candidate has no current references and resolves inside workspace".into(),
                reference_count,
                manifest_required: false,
            });
        }
        Ok(AssetCleanupPlanItem {
            path: record.path.clone(),
            status: "blocked".into(),
            action: "unsupported_asset_type".into(),
            reason: "cleanup can delete only whole files; use an explicit manifest for other asset types".into(),
            reference_count,
            manifest_required: false,
        })
    }

    fn controller_scope(&self) -> String {
        let skill = self.workspace.join("SKILL.md");
        let cargo = self.workspace.join("Cargo.toml");
        let skill_text = if skill.exists() {
            fs::read_to_string(&skill).unwrap_or_default()
        } else {
            String::new()
        };
        let cargo_text = if cargo.exists() {
            fs::read_to_string(&cargo).unwrap_or_default()
        } else {
            String::new()
        };
        if skill_text.contains("RaymanCodingSkill") && cargo_text.contains("crates/rayman-core") {
            "raymancodingskill_controller".into()
        } else {
            "user_controller".into()
        }
    }

    fn references_for_path(
        &self,
        obsolete_path: &str,
        target: Option<&Path>,
    ) -> Result<Vec<AssetReference>> {
        let mut references = Vec::new();
        let needle = obsolete_path.replace('\\', "/");
        let Some(file_name) = Path::new(obsolete_path)
            .file_name()
            .and_then(|name| name.to_str())
            .map(str::to_string)
        else {
            return Ok(references);
        };
        for entry in WalkDir::new(&self.workspace)
            .into_iter()
            .filter_entry(|entry| !ignored(entry.path(), &self.workspace))
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_type().is_file())
        {
            let path = entry.path();
            if Some(path) == target || path == self.state_path {
                continue;
            }
            if !is_text_asset(path) {
                continue;
            }
            let text = fs::read_to_string(path).unwrap_or_default();
            for (index, line) in text.lines().enumerate() {
                if line.contains(&needle) || (!file_name.is_empty() && line.contains(&file_name)) {
                    references.push(AssetReference {
                        path: relative_path(&self.workspace, path),
                        line: index + 1,
                        sha256: sha256_file(path).ok(),
                        reason: format!(
                            "{} reference to obsolete asset {obsolete_path}",
                            reference_surface(path)
                        ),
                    });
                }
            }
        }
        references
            .sort_by(|left, right| left.path.cmp(&right.path).then(left.line.cmp(&right.line)));
        references.dedup_by(|left, right| left.path == right.path && left.line == right.line);
        Ok(references)
    }

    fn read_state(&self) -> Result<AssetState> {
        if !self.state_path.exists() {
            return Ok(AssetState::default());
        }
        let text = fs::read_to_string(&self.state_path).with_context(|| {
            format!(
                "unable to read asset retirement state: {}",
                self.state_path.display()
            )
        })?;
        serde_json::from_str(&text).with_context(|| {
            format!(
                "unable to parse asset retirement state: {}",
                self.state_path.display()
            )
        })
    }

    fn write_state(&self, state: &AssetState) -> Result<()> {
        let mut normalized = (*state).clone();
        normalized.version = STATE_VERSION;
        normalized
            .records
            .sort_by(|left, right| left.path.cmp(&right.path));
        let text = serde_json::to_string_pretty(&normalized)?;
        write_text(&self.state_path, &text)
    }
}

fn upsert_record(records: &mut Vec<ObsoleteAssetRecord>, mut next: ObsoleteAssetRecord) {
    if let Some(existing) = records.iter_mut().find(|record| record.path == next.path) {
        if existing.created_at.is_empty() {
            existing.created_at = next.created_at.clone();
        }
        next.created_at = existing.created_at.clone();
        *existing = next;
    } else {
        records.push(next);
    }
}

fn relative_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn ignored(path: &Path, root: &Path) -> bool {
    let relative = path.strip_prefix(root).unwrap_or(path);
    let normalized = relative.to_string_lossy().replace('\\', "/");
    IGNORED_ROOTS
        .iter()
        .any(|root| normalized == *root || normalized.starts_with(&format!("{root}/")))
}

fn is_text_asset(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or(""),
        "rs" | "md"
            | "toml"
            | "yaml"
            | "yml"
            | "json"
            | "txt"
            | "js"
            | "jsx"
            | "ts"
            | "tsx"
            | "py"
            | "cs"
            | "go"
            | "sh"
            | "ps1"
    )
}

fn asset_kind(path: &str) -> &'static str {
    let lower = path.to_ascii_lowercase();
    if lower.contains("/docs/") || lower.ends_with(".md") {
        "docs"
    } else if lower.ends_with(".toml")
        || lower.ends_with(".yaml")
        || lower.ends_with(".yml")
        || lower.ends_with(".json")
    {
        "config"
    } else if lower.contains("test") || lower.contains("spec") {
        "test"
    } else if lower.ends_with(".sh") || lower.ends_with(".ps1") {
        "script"
    } else if lower.ends_with(".rs")
        || lower.ends_with(".ts")
        || lower.ends_with(".tsx")
        || lower.ends_with(".js")
        || lower.ends_with(".jsx")
        || lower.ends_with(".py")
        || lower.ends_with(".cs")
        || lower.ends_with(".go")
    {
        "code"
    } else {
        "asset"
    }
}

fn reference_surface(path: &Path) -> &'static str {
    let lower = path
        .to_string_lossy()
        .replace('\\', "/")
        .to_ascii_lowercase();
    if lower.contains("/docs/") || lower.ends_with(".md") {
        "docs"
    } else if lower.contains("/tests/") || lower.contains("test") || lower.contains("spec") {
        "test"
    } else if lower.ends_with(".toml")
        || lower.ends_with(".yaml")
        || lower.ends_with(".yml")
        || lower.ends_with(".json")
    {
        "config"
    } else if lower.contains("crates/rayman-cli/src/") || lower.contains("crates/rayman-api/src/") {
        "entrypoint"
    } else {
        "current-file"
    }
}

fn asset_status_name(status: &AssetStatus) -> &'static str {
    match status {
        AssetStatus::RetirementCandidate => "retirement_candidate",
        AssetStatus::Retired => "retired",
        AssetStatus::CompatibilityExempt => "compatibility_exempt",
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExpiryState {
    Active,
    Expired,
    Missing,
    Invalid,
}

fn expiry_state(value: Option<&str>) -> ExpiryState {
    let Some(value) = value else {
        return ExpiryState::Missing;
    };
    let Ok(date) = parse_expiry(value) else {
        return ExpiryState::Invalid;
    };
    if date < Utc::now().date_naive() {
        ExpiryState::Expired
    } else {
        ExpiryState::Active
    }
}

fn parse_expiry(value: &str) -> Result<NaiveDate> {
    NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .with_context(|| format!("invalid expiry date, expected YYYY-MM-DD: {value}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manager(root: &Path) -> AssetRetirementManager {
        AssetRetirementManager::new(root).unwrap()
    }

    #[test]
    fn candidate_asset_blocks_success() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("old.md"), "old behavior\n").unwrap();
        let report = manager(temp.path())
            .retire(AssetRetireRequest {
                path: PathBuf::from("old.md"),
                replacement_behavior: "new.md".into(),
                deletion_reason: "replaced".into(),
                validation_command: "cargo test".into(),
                apply_delete: false,
            })
            .unwrap();

        assert!(report.has_blockers());
        assert!(
            report
                .blockers
                .iter()
                .any(|item| item.contains("retirement candidate"))
        );
        assert!(manager(temp.path()).assert_no_blockers().is_err());
    }

    #[test]
    fn retired_asset_still_present_blocks_success() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join(".RaymanCodingSkill/assets")).unwrap();
        fs::write(temp.path().join("old.md"), "old behavior\n").unwrap();
        let state = AssetState {
            version: STATE_VERSION,
            records: vec![ObsoleteAssetRecord {
                path: "old.md".into(),
                sha256: Some(sha256_file(&temp.path().join("old.md")).unwrap()),
                status: AssetStatus::Retired,
                ..ObsoleteAssetRecord::default()
            }],
        };
        fs::write(
            temp.path().join(STATE_RELATIVE_PATH),
            serde_json::to_string_pretty(&state).unwrap(),
        )
        .unwrap();

        let report = manager(temp.path()).status().unwrap();

        assert!(
            report
                .retired_present
                .iter()
                .any(|record| record.path == "old.md")
        );
        assert!(
            report
                .blockers
                .iter()
                .any(|item| item.contains("retired asset still exists"))
        );
    }

    #[test]
    fn expired_exemption_blocks_success() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("old.md"), "old behavior\n").unwrap();
        let report = manager(temp.path())
            .exempt(AssetExemptRequest {
                path: PathBuf::from("old.md"),
                retention_reason: "audit".into(),
                expires_at: "1970-01-01".into(),
            })
            .unwrap();

        assert!(report.blockers.iter().any(|item| item.contains("expired")));
    }

    #[test]
    fn active_exemption_is_non_current_but_not_blocking() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("old.md"), "old behavior\n").unwrap();
        let report = manager(temp.path())
            .exempt(AssetExemptRequest {
                path: PathBuf::from("old.md"),
                retention_reason: "compatibility window".into(),
                expires_at: "2999-01-01".into(),
            })
            .unwrap();

        assert!(report.blockers.is_empty());
        assert!(report.non_current_paths().contains("old.md"));
        assert!(
            report
                .exemptions
                .iter()
                .any(|record| record.path == "old.md")
        );
    }

    #[test]
    fn retired_path_references_are_detected() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join(".RaymanCodingSkill/assets")).unwrap();
        fs::create_dir_all(temp.path().join("docs")).unwrap();
        fs::write(temp.path().join("docs").join("usage.md"), "See old.md\n").unwrap();
        let state = AssetState {
            version: STATE_VERSION,
            records: vec![ObsoleteAssetRecord {
                path: "old.md".into(),
                status: AssetStatus::Retired,
                ..ObsoleteAssetRecord::default()
            }],
        };
        fs::write(
            temp.path().join(STATE_RELATIVE_PATH),
            serde_json::to_string_pretty(&state).unwrap(),
        )
        .unwrap();

        let report = manager(temp.path()).scan().unwrap();

        assert!(
            report
                .records
                .iter()
                .find(|record| record.path == "old.md")
                .unwrap()
                .references
                .iter()
                .any(|reference| reference.path == "docs/usage.md")
        );
        assert!(
            report
                .blockers
                .iter()
                .any(|item| item.contains("still referenced"))
        );
    }

    #[test]
    fn scan_ignores_managed_temp_references() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join(".RaymanCodingSkill/assets")).unwrap();
        fs::create_dir_all(temp.path().join(".RaymanCodingSkill/tmp/runs/run_1")).unwrap();
        fs::write(
            temp.path()
                .join(".RaymanCodingSkill/tmp/runs/run_1")
                .join("metadata.json"),
            "old.md",
        )
        .unwrap();
        let state = AssetState {
            version: STATE_VERSION,
            records: vec![ObsoleteAssetRecord {
                path: "old.md".into(),
                status: AssetStatus::Retired,
                ..ObsoleteAssetRecord::default()
            }],
        };
        fs::write(
            temp.path().join(STATE_RELATIVE_PATH),
            serde_json::to_string_pretty(&state).unwrap(),
        )
        .unwrap();

        let report = manager(temp.path()).scan().unwrap();

        assert!(report.detected_references.is_empty());
        assert!(report.blockers.is_empty());
    }

    #[test]
    fn scan_ignores_context_os_derived_state_references() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join(".RaymanCodingSkill/assets")).unwrap();
        fs::create_dir_all(temp.path().join(".RaymanCodingSkill/context")).unwrap();
        for name in ["index.json", "state.json", "events.jsonl"] {
            fs::write(
                temp.path().join(".RaymanCodingSkill/context").join(name),
                "old.md",
            )
            .unwrap();
        }
        let state = AssetState {
            version: STATE_VERSION,
            records: vec![ObsoleteAssetRecord {
                path: "old.md".into(),
                status: AssetStatus::Retired,
                ..ObsoleteAssetRecord::default()
            }],
        };
        fs::write(
            temp.path().join(STATE_RELATIVE_PATH),
            serde_json::to_string_pretty(&state).unwrap(),
        )
        .unwrap();

        let report = manager(temp.path()).scan().unwrap();

        assert!(report.detected_references.is_empty());
        assert!(report.blockers.is_empty());
    }

    #[test]
    fn cleanup_apply_deletes_only_registered_unreferenced_files() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("old.md"), "old behavior\n").unwrap();
        fs::write(temp.path().join("active.md"), "current behavior\n").unwrap();
        let manager = manager(temp.path());
        manager
            .retire(AssetRetireRequest {
                path: PathBuf::from("old.md"),
                replacement_behavior: "active.md".into(),
                deletion_reason: "replaced".into(),
                validation_command: "cargo test".into(),
                apply_delete: false,
            })
            .unwrap();

        let report = manager
            .cleanup(AssetCleanupRequest { apply: true })
            .unwrap();

        assert!(!temp.path().join("old.md").exists());
        assert!(temp.path().join("active.md").exists());
        assert!(
            report
                .records
                .iter()
                .any(|record| record.path == "old.md" && record.status == AssetStatus::Retired)
        );
    }

    #[test]
    fn cleanup_requires_manifest_for_directories() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join("old_dir")).unwrap();
        fs::write(temp.path().join("old_dir").join("old.md"), "old behavior\n").unwrap();
        let manager = manager(temp.path());
        manager
            .retire(AssetRetireRequest {
                path: PathBuf::from("old_dir"),
                replacement_behavior: "new_dir".into(),
                deletion_reason: "replaced".into(),
                validation_command: "cargo test".into(),
                apply_delete: false,
            })
            .unwrap();

        let report = manager
            .cleanup(AssetCleanupRequest { apply: true })
            .unwrap();

        assert!(temp.path().join("old_dir").exists());
        assert!(
            report
                .cleanup_plan
                .iter()
                .any(|item| item.path == "old_dir" && item.manifest_required)
        );
    }

    #[test]
    fn controller_scope_distinguishes_canonical_and_user_workspace() {
        let canonical = tempfile::tempdir().unwrap();
        fs::write(canonical.path().join("SKILL.md"), "RaymanCodingSkill").unwrap();
        fs::write(
            canonical.path().join("Cargo.toml"),
            "members = [\"crates/rayman-core\"]",
        )
        .unwrap();
        let user = tempfile::tempdir().unwrap();
        fs::write(user.path().join("README.md"), "# customer").unwrap();

        assert_eq!(
            manager(canonical.path()).status().unwrap().controller_scope,
            "raymancodingskill_controller"
        );
        assert_eq!(
            manager(user.path()).status().unwrap().controller_scope,
            "user_controller"
        );
    }

    #[test]
    fn cleanup_plan_blocks_escaped_state_paths() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join(".RaymanCodingSkill/assets")).unwrap();
        let state = AssetState {
            version: STATE_VERSION,
            records: vec![ObsoleteAssetRecord {
                path: "../outside.md".into(),
                status: AssetStatus::RetirementCandidate,
                ..ObsoleteAssetRecord::default()
            }],
        };
        fs::write(
            temp.path().join(STATE_RELATIVE_PATH),
            serde_json::to_string_pretty(&state).unwrap(),
        )
        .unwrap();

        let report = manager(temp.path())
            .cleanup(AssetCleanupRequest { apply: true })
            .unwrap();

        assert!(report.blockers.iter().any(|item| item.contains("escaped")));
        assert!(
            report
                .cleanup_plan
                .iter()
                .any(|item| item.path == "../outside.md" && item.action == "invalid_path")
        );
    }
}
