use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use chrono::{NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

use crate::feature_coverage::{FEATURE_COVERAGE_MANIFEST, retired_test_anchor_proofs};
use crate::{display_path, ensure_within, now_iso, sha256_file, write_text};

const STATE_VERSION: u32 = 1;
const STATE_RELATIVE_PATH: &str = ".RaymanCodingSkill/assets/retirement.json";
const SOURCE_POLICY: &str = "Obsolete assets are not current-behavior evidence. Current files are authoritative, but recorded obsolete assets must be retired or explicitly exempted before success.";
const FEATURE_COVERAGE_PROOF_SOURCE_PREFIX: &str = "feature_coverage_proofs:";
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
    pub line_end: Option<usize>,
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
    pub cascade_source: Option<String>,
    #[serde(default)]
    pub cascade_test_name: Option<String>,
    #[serde(default)]
    pub cascade_anchor_contains: Option<String>,
    #[serde(default)]
    pub cascade_manifest_path: Option<String>,
    #[serde(default)]
    pub cascade_has_current_proofs: bool,
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
            line_end: None,
            sha256: None,
            kind: String::new(),
            stale_behavior: String::new(),
            replacement_behavior: String::new(),
            deletion_reason: String::new(),
            risk: String::new(),
            validation_command: String::new(),
            references: Vec::new(),
            cascade_source: None,
            cascade_test_name: None,
            cascade_anchor_contains: None,
            cascade_manifest_path: None,
            cascade_has_current_proofs: false,
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
    #[serde(default)]
    pub cascade_candidates: Vec<ObsoleteAssetRecord>,
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
            .filter(|record| record_marks_whole_path_non_current(record))
            .map(|record| record.path.clone())
            .collect()
    }

    pub fn is_non_current_path(&self, path: &str) -> bool {
        let path = normalize_asset_record_path(path);
        self.records.iter().any(|record| {
            record_marks_whole_path_non_current(record)
                && non_current_record_covers_path(&record.path, &path)
        })
    }

    pub fn is_current_behavior_path(&self, path: &str) -> bool {
        !self.is_non_current_path(path)
    }
}

fn normalize_asset_record_path(path: &str) -> String {
    path.replace('\\', "/").trim_matches('/').to_string()
}

fn non_current_record_covers_path(record_path: &str, path: &str) -> bool {
    let record_path = normalize_asset_record_path(record_path);
    if record_path.is_empty() {
        return false;
    }
    path == record_path || path.starts_with(&format!("{record_path}/"))
}

fn record_marks_whole_path_non_current(record: &ObsoleteAssetRecord) -> bool {
    matches!(
        record.status,
        AssetStatus::RetirementCandidate | AssetStatus::Retired | AssetStatus::CompatibilityExempt
    ) && !(record.kind == "test" && record.cascade_source.is_some())
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
    #[serde(default)]
    pub line: Option<usize>,
    #[serde(default)]
    pub line_end: Option<usize>,
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
            if let Some(scanned) = report
                .records
                .iter()
                .find(|item| same_asset_record(item, record))
            {
                record.references = scanned.references.clone();
                record.updated_at = now.clone();
            }
        }
        for record in self.cascade_records_for_report(&report, &now)? {
            upsert_record(&mut state.records, record);
        }
        self.write_state(&state)?;
        self.report_for_records(state.records)
    }

    pub fn cleanup(&self, request: AssetCleanupRequest) -> Result<AssetRetirementReport> {
        let mut state = self.read_state()?;
        let now = now_iso();
        let mut deleted = BTreeSet::new();
        let initial = self.report_for_records(state.records.clone())?;
        if !request.apply {
            return Ok(initial);
        }
        loop {
            let report = self.report_for_records(state.records.clone())?;
            let mut changed = false;
            for item in &report.cleanup_plan {
                if !matches!(
                    item.action.as_str(),
                    "delete_file"
                        | "delete_test_case"
                        | "delete_feature_coverage_anchor"
                        | "mark_retired"
                ) {
                    continue;
                }
                let Some(record) = state
                    .records
                    .iter_mut()
                    .find(|record| cleanup_plan_matches_record(item, record))
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
                if item.action == "delete_feature_coverage_anchor" {
                    remove_feature_coverage_test_anchor(&self.workspace, record)?;
                    record.status = AssetStatus::Retired;
                    record.updated_at = now.clone();
                    record.retired_at = Some(now.clone());
                    record.references.clear();
                    deleted.insert(asset_record_key(record));
                    changed = true;
                    break;
                }
                if !target.exists() || item.action == "mark_retired" {
                    record.status = AssetStatus::Retired;
                    record.updated_at = now.clone();
                    record.retired_at = Some(now.clone());
                    record.references.clear();
                    deleted.insert(asset_record_key(record));
                    changed = true;
                    break;
                }
                if item.action == "delete_test_case" {
                    let removed = remove_cascade_test_case_range(&target, record)?;
                    remove_feature_coverage_test_anchor(&self.workspace, record)?;
                    record.line = Some(removed.line_start);
                    record.line_end = Some(removed.line_end);
                    record.cascade_test_name = Some(removed.name);
                    record.status = AssetStatus::Retired;
                    record.updated_at = now.clone();
                    record.retired_at = Some(now.clone());
                    record.references.clear();
                    deleted.insert(asset_record_key(record));
                    changed = true;
                    break;
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
                deleted.insert(asset_record_key(record));
                changed = true;
                break;
            }
            if !changed {
                break;
            }
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
                line_end: None,
                sha256,
                kind: asset_kind(&path).into(),
                stale_behavior: "asset identified as obsolete by retirement request".into(),
                replacement_behavior: request.replacement_behavior,
                deletion_reason: request.deletion_reason,
                risk: "whole-file retirement can break callers if stale references remain".into(),
                validation_command: request.validation_command,
                references,
                cascade_source: None,
                cascade_test_name: None,
                cascade_anchor_contains: None,
                cascade_manifest_path: None,
                cascade_has_current_proofs: false,
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
                let _ = self.scan();
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
        self.scan()
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
                line_end: None,
                sha256,
                kind: asset_kind(&path).into(),
                stale_behavior: "asset retained only for compatibility or audit".into(),
                replacement_behavior: "retained asset is excluded from current-behavior context".into(),
                deletion_reason: "temporary compatibility exemption".into(),
                risk: "retained obsolete assets can pollute project understanding if treated as current".into(),
                validation_command: "rayman assets status".into(),
                references,
                cascade_source: None,
                cascade_test_name: None,
                cascade_anchor_contains: None,
                cascade_manifest_path: None,
                cascade_has_current_proofs: false,
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
        let mut cascade_candidates = Vec::new();
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
                        line: record.line,
                        line_end: record.line_end,
                        status: "blocked".into(),
                        action: "invalid_path".into(),
                        reason: error.to_string(),
                        reference_count: 0,
                        manifest_required: false,
                    });
                    continue;
                }
            };
            record.references = self.references_for_record(record, Some(&target))?;
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
                    if record.cascade_source.is_some() {
                        cascade_candidates.push(record.clone());
                    }
                }
                AssetStatus::Retired => {
                    if self.retired_record_still_present(record, &target)? {
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
            if record.line.is_none()
                && record.cascade_source.is_none()
                && let Some(stored_hash) = &record.sha256
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
            cascade_candidates,
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
                    line: record.line,
                    line_end: record.line_end,
                    status: "blocked".into(),
                    action: "invalid_path".into(),
                    reason: error.to_string(),
                    reference_count,
                    manifest_required: false,
                });
            }
        };
        if record.cascade_source.is_some() && record.kind == "test" {
            return self.cascade_test_cleanup_plan_item(record, &target, reference_count);
        }
        if !matches!(record.status, AssetStatus::RetirementCandidate) {
            return Ok(AssetCleanupPlanItem {
                path: record.path.clone(),
                line: record.line,
                line_end: record.line_end,
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
                line: record.line,
                line_end: record.line_end,
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
                line: record.line,
                line_end: record.line_end,
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
                line: record.line,
                line_end: record.line_end,
                status: "ready".into(),
                action: "delete_file".into(),
                reason: "registered retirement candidate has no current references and resolves inside workspace".into(),
                reference_count,
                manifest_required: false,
            });
        }
        Ok(AssetCleanupPlanItem {
            path: record.path.clone(),
            line: record.line,
            line_end: record.line_end,
            status: "blocked".into(),
            action: "unsupported_asset_type".into(),
            reason: "cleanup can delete only whole files; use an explicit manifest for other asset types".into(),
            reference_count,
            manifest_required: false,
        })
    }

    fn cascade_test_cleanup_plan_item(
        &self,
        record: &ObsoleteAssetRecord,
        target: &Path,
        reference_count: usize,
    ) -> Result<AssetCleanupPlanItem> {
        if !matches!(record.status, AssetStatus::RetirementCandidate) {
            return Ok(AssetCleanupPlanItem {
                path: record.path.clone(),
                line: record.line,
                line_end: record.line_end,
                status: asset_status_name(&record.status).into(),
                action: "none".into(),
                reason: "cleanup only deletes registered retirement candidates".into(),
                reference_count,
                manifest_required: false,
            });
        }
        if feature_coverage_anchor_cascade(record) && record.cascade_has_current_proofs {
            return Ok(AssetCleanupPlanItem {
                path: record.path.clone(),
                line: record.line,
                line_end: record.line_end,
                status: "blocked".into(),
                action: "manual_test_case_prune_required".into(),
                reason: "feature coverage test anchor mixes current and retired proofs".into(),
                reference_count,
                manifest_required: false,
            });
        }
        if !target.exists() {
            if feature_coverage_anchor_cascade(record) {
                return Ok(self.delete_feature_coverage_anchor_plan_item(
                    record,
                    reference_count,
                    "feature coverage test anchor points to a test target that is already absent",
                ));
            }
            return Ok(AssetCleanupPlanItem {
                path: record.path.clone(),
                line: record.line,
                line_end: record.line_end,
                status: "ready".into(),
                action: "mark_retired".into(),
                reason: "cascaded obsolete test target is already absent".into(),
                reference_count,
                manifest_required: false,
            });
        }
        if !target.is_file() {
            if feature_coverage_anchor_cascade(record) {
                return Ok(self.delete_feature_coverage_anchor_plan_item(
                    record,
                    reference_count,
                    "feature coverage test anchor points to a non-file target and has no current proofs",
                ));
            }
            return Ok(AssetCleanupPlanItem {
                path: record.path.clone(),
                line: record.line,
                line_end: record.line_end,
                status: "blocked".into(),
                action: "manual_test_case_prune_required".into(),
                reason: "cascaded obsolete test target is not a file".into(),
                reference_count,
                manifest_required: false,
            });
        }
        if feature_coverage_anchor_cascade(record)
            && record
                .cascade_anchor_contains
                .as_deref()
                .is_some_and(|needle| {
                    !fs::read_to_string(target)
                        .unwrap_or_default()
                        .contains(needle)
                })
        {
            return Ok(self.delete_feature_coverage_anchor_plan_item(
                record,
                reference_count,
                "feature coverage test anchor points to a test case that is already absent",
            ));
        }
        if let Some(range) = self.current_cascade_test_range(record, target)? {
            return Ok(AssetCleanupPlanItem {
                path: record.path.clone(),
                line: record.line.or(Some(range.line_start)),
                line_end: record.line_end.or(Some(range.line_end)),
                status: "ready".into(),
                action: "delete_test_case".into(),
                reason: "registered cascaded obsolete test case can be deleted by line range"
                    .into(),
                reference_count,
                manifest_required: false,
            });
        }
        if record.line.is_none() && record.cascade_test_name.is_none() {
            return Ok(AssetCleanupPlanItem {
                path: record.path.clone(),
                line: record.line,
                line_end: record.line_end,
                status: "blocked".into(),
                action: "manual_test_case_prune_required".into(),
                reason: "obsolete test reference could not be isolated to one test case".into(),
                reference_count,
                manifest_required: false,
            });
        }
        Ok(AssetCleanupPlanItem {
            path: record.path.clone(),
            line: record.line,
            line_end: record.line_end,
            status: "ready".into(),
            action: "mark_retired".into(),
            reason: "registered cascaded obsolete test case is no longer present".into(),
            reference_count,
            manifest_required: false,
        })
    }

    fn delete_feature_coverage_anchor_plan_item(
        &self,
        record: &ObsoleteAssetRecord,
        reference_count: usize,
        reason: &str,
    ) -> AssetCleanupPlanItem {
        AssetCleanupPlanItem {
            path: record.path.clone(),
            line: record.line,
            line_end: record.line_end,
            status: "ready".into(),
            action: "delete_feature_coverage_anchor".into(),
            reason: reason.into(),
            reference_count,
            manifest_required: false,
        }
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
                if let Some(match_kind) = obsolete_reference_match(line, &needle, &file_name) {
                    references.push(AssetReference {
                        path: relative_path(&self.workspace, path),
                        line: index + 1,
                        sha256: sha256_file(path).ok(),
                        reason: format!(
                            "{} {} reference to obsolete asset {obsolete_path}",
                            reference_surface(path),
                            match_kind.reason_label()
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

    fn references_for_record(
        &self,
        record: &ObsoleteAssetRecord,
        target: Option<&Path>,
    ) -> Result<Vec<AssetReference>> {
        if let Some(source) = &record.cascade_source {
            return self.references_for_cascade_test(record, source, target);
        }
        self.references_for_path(&record.path, target)
    }

    fn references_for_cascade_test(
        &self,
        record: &ObsoleteAssetRecord,
        source: &str,
        target: Option<&Path>,
    ) -> Result<Vec<AssetReference>> {
        let Some(target) = target else {
            return Ok(Vec::new());
        };
        if !target.exists() || !target.is_file() {
            return Ok(Vec::new());
        }
        let text = fs::read_to_string(target).unwrap_or_default();
        let match_text = cascade_match_text(record, source);
        if let Some(range) = cascade_test_range(record, &text)
            && range_contains_exact_obsolete_source(&text, &range, match_text)
        {
            return Ok(vec![AssetReference {
                path: relative_path(&self.workspace, target),
                line: range.line_start,
                sha256: sha256_file(target).ok(),
                reason: cascade_reference_reason(record, source, &range.name),
            }]);
        }
        if record.line.is_none() {
            for (index, line) in text.lines().enumerate() {
                if line_contains_possible_obsolete_source(line, match_text) {
                    return Ok(vec![AssetReference {
                        path: relative_path(&self.workspace, target),
                        line: index + 1,
                        sha256: sha256_file(target).ok(),
                        reason: cascade_unisolated_reference_reason(record, source),
                    }]);
                }
            }
        }
        Ok(Vec::new())
    }

    fn cascade_test_range_present(
        &self,
        record: &ObsoleteAssetRecord,
        target: &Path,
    ) -> Result<bool> {
        if !target.exists() || !target.is_file() {
            return Ok(false);
        }
        let Some(source) = &record.cascade_source else {
            return Ok(false);
        };
        let text = fs::read_to_string(target).unwrap_or_default();
        let match_text = cascade_match_text(record, source);
        Ok(cascade_test_range(record, &text)
            .as_ref()
            .is_some_and(|range| range_contains_exact_obsolete_source(&text, range, match_text)))
    }

    fn current_cascade_test_range(
        &self,
        record: &ObsoleteAssetRecord,
        target: &Path,
    ) -> Result<Option<TestCaseRange>> {
        if !target.exists() || !target.is_file() {
            return Ok(None);
        }
        let Some(source) = &record.cascade_source else {
            return Ok(None);
        };
        let text = fs::read_to_string(target).unwrap_or_default();
        let match_text = cascade_match_text(record, source);
        Ok(cascade_test_range(record, &text)
            .filter(|range| range_contains_exact_obsolete_source(&text, range, match_text)))
    }

    fn retired_record_still_present(
        &self,
        record: &ObsoleteAssetRecord,
        target: &Path,
    ) -> Result<bool> {
        if record.cascade_source.is_some() && record.kind == "test" {
            return self.cascade_test_range_present(record, target);
        }
        Ok(target.exists())
    }

    fn cascade_records_for_report(
        &self,
        report: &AssetRetirementReport,
        now: &str,
    ) -> Result<Vec<ObsoleteAssetRecord>> {
        let mut records = Vec::new();
        let mut seen = BTreeSet::new();
        for record in &report.records {
            if record.cascade_source.is_some()
                || matches!(record.status, AssetStatus::CompatibilityExempt)
            {
                continue;
            }
            for reference in &record.references {
                if !is_test_reference(reference) {
                    continue;
                }
                if let Some(candidate) =
                    self.cascade_record_for_reference(record, reference, now)?
                    && seen.insert(asset_record_key(&candidate))
                {
                    records.push(candidate);
                }
            }
        }
        for candidate in self.feature_coverage_cascade_records(now)? {
            if seen.insert(asset_record_key(&candidate)) {
                records.push(candidate);
            }
        }
        Ok(records)
    }

    fn feature_coverage_cascade_records(&self, now: &str) -> Result<Vec<ObsoleteAssetRecord>> {
        let mut records = Vec::new();
        for retired_anchor in retired_test_anchor_proofs(&self.workspace)? {
            if let Some(record) =
                self.cascade_record_for_retired_test_anchor(retired_anchor, now)?
            {
                records.push(record);
            }
        }
        Ok(records)
    }

    fn cascade_record_for_retired_test_anchor(
        &self,
        anchor: crate::feature_coverage::RetiredTestAnchorProof,
        now: &str,
    ) -> Result<Option<ObsoleteAssetRecord>> {
        let target = match ensure_within(
            Path::new(&anchor.anchor_path),
            &self.workspace,
            "feature coverage test anchor path escaped workspace",
        ) {
            Ok(target) => target,
            Err(_) => return Ok(None),
        };
        if !target.exists() || !target.is_file() {
            return Ok(None);
        }
        let text = fs::read_to_string(&target).unwrap_or_default();
        let anchor_line = first_line_containing(&text, &anchor.anchor_contains);
        let range = if anchor.has_current_proofs {
            None
        } else {
            anchor_line.and_then(|line| rust_test_case_range_for_line(&text, line))
        };
        let (line, line_end, test_name) = range
            .as_ref()
            .map(|range| {
                (
                    Some(range.line_start),
                    Some(range.line_end),
                    Some(range.name.clone()),
                )
            })
            .unwrap_or((None, None, None));
        let proof_summary = anchor.orphan_proofs.join(", ");
        let source = format!(
            "{FEATURE_COVERAGE_PROOF_SOURCE_PREFIX}{}",
            serde_json::to_string(&anchor.orphan_proofs)?
        );
        let path = relative_path(&self.workspace, &target);
        Ok(Some(ObsoleteAssetRecord {
            path: path.clone(),
            line,
            line_end,
            sha256: None,
            kind: "test".into(),
            stale_behavior: test_name
                .as_deref()
                .map(|name| {
                    format!(
                        "test case `{name}` only proves retired feature coverage surface(s): {proof_summary}"
                    )
                })
                .unwrap_or_else(|| {
                    if anchor.has_current_proofs {
                        format!(
                            "feature coverage test anchor mixes current proof with retired surface(s): {proof_summary}"
                        )
                    } else {
                        format!(
                            "feature coverage test anchor proves retired surface(s) but could not be isolated to one test case: {proof_summary}"
                        )
                    }
                }),
            replacement_behavior: "delete the obsolete test case or rewrite it to prove current behavior"
                .into(),
            deletion_reason: format!(
                "cascaded from retired feature coverage surface(s): {proof_summary}"
            ),
            risk: "semantic test retirement can affect mixed test files if the anchor no longer isolates one test"
                .into(),
            validation_command: "rayman coverage status --check".into(),
            references: vec![AssetReference {
                path,
                line: anchor_line.unwrap_or(1),
                sha256: sha256_file(&target).ok(),
                reason: format!(
                    "test anchor proves retired feature coverage surface(s): {proof_summary}"
                ),
            }],
            cascade_source: Some(source),
            cascade_test_name: test_name,
            cascade_anchor_contains: Some(anchor.anchor_contains),
            cascade_manifest_path: Some(FEATURE_COVERAGE_MANIFEST.into()),
            cascade_has_current_proofs: anchor.has_current_proofs,
            retention_reason: None,
            expires_at: None,
            status: AssetStatus::RetirementCandidate,
            created_at: now.to_string(),
            updated_at: now.to_string(),
            retired_at: None,
        }))
    }

    fn cascade_record_for_reference(
        &self,
        source: &ObsoleteAssetRecord,
        reference: &AssetReference,
        now: &str,
    ) -> Result<Option<ObsoleteAssetRecord>> {
        let target = ensure_within(
            Path::new(&reference.path),
            &self.workspace,
            "cascade test path escaped workspace",
        )?;
        if !target.exists() || !target.is_file() {
            return Ok(None);
        }
        let text = fs::read_to_string(&target).unwrap_or_default();
        let exact_reference = text
            .lines()
            .nth(reference.line.saturating_sub(1))
            .is_some_and(|line| line_contains_exact_obsolete_source(line, &source.path));
        let range = exact_reference
            .then(|| rust_test_case_range_for_line(&text, reference.line))
            .flatten();
        let (line, line_end, test_name) = range
            .as_ref()
            .map(|range| {
                (
                    Some(range.line_start),
                    Some(range.line_end),
                    Some(range.name.clone()),
                )
            })
            .unwrap_or((None, None, None));
        Ok(Some(ObsoleteAssetRecord {
            path: reference.path.clone(),
            line,
            line_end,
            sha256: None,
            kind: "test".into(),
            stale_behavior: test_name
                .as_deref()
                .map(|name| {
                    format!("test case `{name}` only proves obsolete asset {}", source.path)
                })
                .unwrap_or_else(|| {
                    format!(
                        "test reference to obsolete asset {} could not be isolated to one test case",
                        source.path
                    )
                }),
            replacement_behavior: source.replacement_behavior.clone(),
            deletion_reason: format!("cascaded from obsolete asset {}", source.path),
            risk: "line-level test retirement can affect mixed test files if range detection is wrong"
                .into(),
            validation_command: if source.validation_command.trim().is_empty() {
                "cargo test --all".into()
            } else {
                source.validation_command.clone()
            },
            references: vec![reference.clone()],
            cascade_source: Some(source.path.clone()),
            cascade_test_name: test_name,
            cascade_anchor_contains: None,
            cascade_manifest_path: None,
            cascade_has_current_proofs: false,
            retention_reason: None,
            expires_at: None,
            status: AssetStatus::RetirementCandidate,
            created_at: now.to_string(),
            updated_at: now.to_string(),
            retired_at: None,
        }))
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
    if let Some(existing) = records
        .iter_mut()
        .find(|record| same_asset_record(record, &next))
    {
        if existing.created_at.is_empty() {
            existing.created_at = next.created_at.clone();
        }
        next.created_at = existing.created_at.clone();
        *existing = next;
    } else {
        records.push(next);
    }
}

fn same_asset_record(left: &ObsoleteAssetRecord, right: &ObsoleteAssetRecord) -> bool {
    left.path == right.path
        && left.line == right.line
        && left.line_end == right.line_end
        && left.cascade_source == right.cascade_source
        && left.cascade_test_name == right.cascade_test_name
        && left.cascade_anchor_contains == right.cascade_anchor_contains
        && left.cascade_manifest_path == right.cascade_manifest_path
        && left.cascade_has_current_proofs == right.cascade_has_current_proofs
}

fn cleanup_plan_matches_record(item: &AssetCleanupPlanItem, record: &ObsoleteAssetRecord) -> bool {
    item.path == record.path && item.line == record.line && item.line_end == record.line_end
}

fn asset_record_key(record: &ObsoleteAssetRecord) -> String {
    format!(
        "{}:{}:{}:{}:{}:{}:{}:{}",
        record.path,
        record.line.unwrap_or(0),
        record.line_end.unwrap_or(0),
        record.cascade_source.as_deref().unwrap_or(""),
        record.cascade_test_name.as_deref().unwrap_or(""),
        record.cascade_anchor_contains.as_deref().unwrap_or(""),
        record.cascade_manifest_path.as_deref().unwrap_or(""),
        record.cascade_has_current_proofs
    )
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

fn is_test_reference(reference: &AssetReference) -> bool {
    reference.reason.starts_with("test ")
        || reference
            .path
            .to_ascii_lowercase()
            .replace('\\', "/")
            .contains("/tests/")
        || reference.path.to_ascii_lowercase().contains("test")
        || reference.path.to_ascii_lowercase().contains("spec")
}

fn cascade_match_text<'a>(record: &'a ObsoleteAssetRecord, source: &'a str) -> &'a str {
    record.cascade_anchor_contains.as_deref().unwrap_or(source)
}

fn feature_coverage_anchor_cascade(record: &ObsoleteAssetRecord) -> bool {
    record.cascade_manifest_path.as_deref() == Some(FEATURE_COVERAGE_MANIFEST)
}

fn cascade_reference_reason(record: &ObsoleteAssetRecord, source: &str, test_name: &str) -> String {
    if record.cascade_manifest_path.is_some() {
        return format!(
            "test case `{test_name}` proves retired feature coverage surface(s): {}",
            feature_coverage_proof_summary(source)
        );
    }
    format!("test case `{test_name}` depends on obsolete asset {source}")
}

fn cascade_unisolated_reference_reason(record: &ObsoleteAssetRecord, source: &str) -> String {
    if record.cascade_manifest_path.is_some() {
        return format!(
            "test reference could not be isolated but still proves retired feature coverage surface(s): {}",
            feature_coverage_proof_summary(source)
        );
    }
    format!("test reference could not be isolated but still mentions obsolete asset {source}")
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TestCaseRange {
    name: String,
    line_start: usize,
    line_end: usize,
}

fn cascade_test_range(record: &ObsoleteAssetRecord, text: &str) -> Option<TestCaseRange> {
    if let Some(name) = &record.cascade_test_name {
        rust_test_case_ranges(text)
            .into_iter()
            .find(|range| &range.name == name)
    } else {
        record
            .line
            .and_then(|line| rust_test_case_range_for_line(text, line))
    }
}

fn rust_test_case_range_for_line(text: &str, line: usize) -> Option<TestCaseRange> {
    rust_test_case_ranges(text)
        .into_iter()
        .find(|range| line >= range.line_start && line <= range.line_end)
}

fn rust_test_case_ranges(text: &str) -> Vec<TestCaseRange> {
    let lines = text.lines().collect::<Vec<_>>();
    let mut ranges = Vec::new();
    let mut index = 0usize;
    while index < lines.len() {
        let trimmed = lines[index].trim();
        let attr_start = if is_rust_test_attr(trimmed) {
            Some(index)
        } else {
            None
        };
        let Some(fn_index) = find_test_fn_index(&lines, index, attr_start.is_some()) else {
            index += 1;
            continue;
        };
        let Some(name) = rust_fn_name(lines[fn_index]) else {
            index += 1;
            continue;
        };
        if attr_start.is_none() && !name.starts_with("test_") {
            index += 1;
            continue;
        }
        let line_start = attr_start.unwrap_or(fn_index) + 1;
        let line_end = rust_fn_end_line(&lines, fn_index).unwrap_or(fn_index + 1);
        ranges.push(TestCaseRange {
            name,
            line_start,
            line_end,
        });
        index = line_end.max(index + 1);
    }
    ranges
}

fn is_rust_test_attr(trimmed: &str) -> bool {
    trimmed == "#[test]"
        || trimmed.starts_with("#[tokio::test")
        || trimmed.starts_with("#[async_std::test")
}

fn find_test_fn_index(lines: &[&str], start: usize, from_attr: bool) -> Option<usize> {
    let limit = if from_attr {
        (start + 6).min(lines.len())
    } else {
        (start + 1).min(lines.len())
    };
    (start..limit).find(|index| rust_fn_name(lines[*index]).is_some())
}

fn rust_fn_name(line: &str) -> Option<String> {
    let rest = line.split("fn ").nth(1)?;
    let name = rest
        .chars()
        .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '_')
        .collect::<String>();
    (!name.is_empty()).then_some(name)
}

fn rust_fn_end_line(lines: &[&str], fn_index: usize) -> Option<usize> {
    let mut balance = 0isize;
    let mut saw_body = false;
    let mut scan = RustBraceScanState::default();
    for (index, line) in lines.iter().enumerate().skip(fn_index) {
        let (opens, closes) = rust_code_braces(line, &mut scan);
        if opens > 0 {
            saw_body = true;
        }
        balance += opens as isize;
        balance -= closes as isize;
        if saw_body && balance <= 0 {
            return Some(index + 1);
        }
    }
    saw_body.then_some(lines.len())
}

#[derive(Debug, Default)]
struct RustBraceScanState {
    block_comment_depth: usize,
    in_string: bool,
    raw_string_hashes: Option<usize>,
}

fn rust_code_braces(line: &str, state: &mut RustBraceScanState) -> (usize, usize) {
    let chars = line.chars().collect::<Vec<_>>();
    let mut index = 0usize;
    let mut opens = 0usize;
    let mut closes = 0usize;
    while index < chars.len() {
        if let Some(hashes) = state.raw_string_hashes {
            if chars[index] == '"' && raw_string_terminates(&chars, index, hashes) {
                state.raw_string_hashes = None;
                index += hashes + 1;
            } else {
                index += 1;
            }
            continue;
        }
        if state.in_string {
            if chars[index] == '\\' {
                index += 2;
                continue;
            }
            if chars[index] == '"' {
                state.in_string = false;
            }
            index += 1;
            continue;
        }
        if state.block_comment_depth > 0 {
            if starts_with_chars(&chars, index, "/*") {
                state.block_comment_depth += 1;
                index += 2;
            } else if starts_with_chars(&chars, index, "*/") {
                state.block_comment_depth -= 1;
                index += 2;
            } else {
                index += 1;
            }
            continue;
        }
        if starts_with_chars(&chars, index, "//") {
            break;
        }
        if starts_with_chars(&chars, index, "/*") {
            state.block_comment_depth += 1;
            index += 2;
            continue;
        }
        if let Some(hashes) = raw_string_hashes_at(&chars, index) {
            state.raw_string_hashes = Some(hashes);
            index += hashes + 2;
            continue;
        }
        if chars[index] == '"' {
            state.in_string = true;
            index += 1;
            continue;
        }
        if let Some(end) = char_literal_end(&chars, index) {
            index = end + 1;
            continue;
        }
        match chars[index] {
            '{' => opens += 1,
            '}' => closes += 1,
            _ => {}
        }
        index += 1;
    }
    (opens, closes)
}

fn starts_with_chars(chars: &[char], index: usize, needle: &str) -> bool {
    needle
        .chars()
        .enumerate()
        .all(|(offset, expected)| chars.get(index + offset) == Some(&expected))
}

fn raw_string_hashes_at(chars: &[char], index: usize) -> Option<usize> {
    if chars.get(index) != Some(&'r') {
        return None;
    }
    let mut cursor = index + 1;
    let mut hashes = 0usize;
    while chars.get(cursor) == Some(&'#') {
        hashes += 1;
        cursor += 1;
    }
    (chars.get(cursor) == Some(&'"')).then_some(hashes)
}

fn raw_string_terminates(chars: &[char], quote_index: usize, hashes: usize) -> bool {
    (0..hashes).all(|offset| chars.get(quote_index + 1 + offset) == Some(&'#'))
}

fn char_literal_end(chars: &[char], index: usize) -> Option<usize> {
    if chars.get(index) != Some(&'\'') {
        return None;
    }
    if chars.get(index + 1) == Some(&'\\') {
        return (chars.get(index + 3) == Some(&'\'')).then_some(index + 3);
    }
    (chars.get(index + 2) == Some(&'\'')).then_some(index + 2)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ObsoleteReferenceMatch {
    ExactPath,
    FilenameOnly,
}

impl ObsoleteReferenceMatch {
    fn reason_label(self) -> &'static str {
        match self {
            ObsoleteReferenceMatch::ExactPath => "exact-path",
            ObsoleteReferenceMatch::FilenameOnly => "filename-only-possible",
        }
    }
}

fn obsolete_reference_match(
    line: &str,
    normalized_source: &str,
    file_name: &str,
) -> Option<ObsoleteReferenceMatch> {
    if !normalized_source.is_empty() && line.replace('\\', "/").contains(normalized_source) {
        return Some(ObsoleteReferenceMatch::ExactPath);
    }
    if !file_name.is_empty() && line.contains(file_name) {
        return Some(ObsoleteReferenceMatch::FilenameOnly);
    }
    None
}

fn first_line_containing(text: &str, needle: &str) -> Option<usize> {
    text.lines()
        .enumerate()
        .find_map(|(index, line)| line.contains(needle).then_some(index + 1))
}

fn feature_coverage_proofs(source: &str) -> Vec<String> {
    let Some(json) = source.strip_prefix(FEATURE_COVERAGE_PROOF_SOURCE_PREFIX) else {
        return Vec::new();
    };
    serde_json::from_str(json).unwrap_or_default()
}

fn feature_coverage_proof_summary(source: &str) -> String {
    let proofs = feature_coverage_proofs(source);
    if proofs.is_empty() {
        source.to_string()
    } else {
        proofs.join(", ")
    }
}

fn range_contains_exact_obsolete_source(text: &str, range: &TestCaseRange, source: &str) -> bool {
    text.lines()
        .enumerate()
        .filter(|(index, _)| {
            let line = index + 1;
            line >= range.line_start && line <= range.line_end
        })
        .any(|(_, line)| line_contains_exact_obsolete_source(line, source))
}

fn line_contains_exact_obsolete_source(line: &str, source: &str) -> bool {
    let source = source.replace('\\', "/");
    !source.is_empty() && line.replace('\\', "/").contains(&source)
}

fn line_contains_possible_obsolete_source(line: &str, source: &str) -> bool {
    if line_contains_exact_obsolete_source(line, source) {
        return true;
    }
    Path::new(&source)
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| !name.is_empty() && line.contains(name))
}

fn remove_line_range(path: &Path, start: usize, end: usize) -> Result<()> {
    if start == 0 || end < start {
        bail!("invalid obsolete test line range: {start}..{end}");
    }
    let text = fs::read_to_string(path)
        .with_context(|| format!("unable to read obsolete test file: {}", path.display()))?;
    let mut lines = text.lines().map(str::to_string).collect::<Vec<_>>();
    if end > lines.len() {
        bail!(
            "obsolete test line range exceeds file length: {} {}..{}",
            path.display(),
            start,
            end
        );
    }
    lines.drain((start - 1)..end);
    let mut next = lines.join("\n");
    if text.ends_with('\n') && !next.is_empty() {
        next.push('\n');
    }
    write_text(path, &next)
}

fn remove_feature_coverage_test_anchor(root: &Path, record: &ObsoleteAssetRecord) -> Result<()> {
    if record.cascade_manifest_path.as_deref() != Some(FEATURE_COVERAGE_MANIFEST) {
        return Ok(());
    }
    let Some(anchor_contains) = record.cascade_anchor_contains.as_deref() else {
        return Ok(());
    };
    let Some(source) = record.cascade_source.as_deref() else {
        return Ok(());
    };
    let retired_proofs = feature_coverage_proofs(source);
    if retired_proofs.is_empty() {
        return Ok(());
    }
    let manifest_path = root.join(FEATURE_COVERAGE_MANIFEST);
    if !manifest_path.exists() {
        return Ok(());
    }
    let text = fs::read_to_string(&manifest_path).with_context(|| {
        format!(
            "unable to read feature coverage manifest: {}",
            manifest_path.display()
        )
    })?;
    if let Some(next) =
        remove_feature_coverage_anchor_block(&text, &record.path, anchor_contains, &retired_proofs)?
    {
        write_text(&manifest_path, &next)?;
    }
    Ok(())
}

fn remove_feature_coverage_anchor_block(
    text: &str,
    anchor_path: &str,
    anchor_contains: &str,
    retired_proofs: &[String],
) -> Result<Option<String>> {
    let mut lines = text.lines().map(str::to_string).collect::<Vec<_>>();
    let mut index = 0usize;
    while index < lines.len() {
        if yaml_path_entry_value(&lines[index]).as_deref() != Some(anchor_path) {
            index += 1;
            continue;
        }
        let item_indent = leading_spaces(&lines[index]);
        let Some(parent_index) =
            parent_sequence_key_index(&lines, index, item_indent, "test_anchors")
        else {
            index += 1;
            continue;
        };
        let end = yaml_item_block_end(&lines, index, item_indent);
        let block = &lines[index..end];
        if !yaml_block_contains_value(block, "contains", anchor_contains) {
            index += 1;
            continue;
        }
        let block_proofs = yaml_block_proofs(block);
        if !retired_proofs
            .iter()
            .all(|proof| block_proofs.contains(proof))
        {
            index += 1;
            continue;
        }
        let current_proofs = block_proofs
            .iter()
            .filter(|proof| !retired_proofs.contains(*proof))
            .cloned()
            .collect::<Vec<_>>();
        if !current_proofs.is_empty() {
            bail!(
                "feature coverage test anchor still proves current surfaces and must be rewritten manually: {}",
                current_proofs.join(", ")
            );
        }
        lines.drain(index..end);
        normalize_empty_yaml_sequence(&mut lines, parent_index, "test_anchors");
        let mut next = lines.join("\n");
        if text.ends_with('\n') && !next.is_empty() {
            next.push('\n');
        }
        return Ok(Some(next));
    }
    Ok(None)
}

fn parent_sequence_key_index(
    lines: &[String],
    start: usize,
    item_indent: usize,
    key: &str,
) -> Option<usize> {
    let expected = format!("{key}:");
    (0..start).rev().find(|index| {
        leading_spaces(&lines[*index]) < item_indent && lines[*index].trim() == expected
    })
}

fn normalize_empty_yaml_sequence(lines: &mut [String], key_index: usize, key: &str) {
    let key_indent = leading_spaces(&lines[key_index]);
    let mut cursor = key_index + 1;
    while cursor < lines.len() {
        let trimmed = lines[cursor].trim();
        if trimmed.is_empty() {
            cursor += 1;
            continue;
        }
        let indent = leading_spaces(&lines[cursor]);
        if indent <= key_indent {
            break;
        }
        if lines[cursor].trim_start().starts_with("- ") {
            return;
        }
        cursor += 1;
    }
    lines[key_index] = format!("{}{key}: []", " ".repeat(key_indent));
}

fn yaml_item_block_end(lines: &[String], start: usize, item_indent: usize) -> usize {
    let mut cursor = start + 1;
    while cursor < lines.len() {
        if !lines[cursor].trim().is_empty() && leading_spaces(&lines[cursor]) <= item_indent {
            break;
        }
        cursor += 1;
    }
    cursor
}

fn yaml_path_entry_value(line: &str) -> Option<String> {
    let value = line.trim_start().strip_prefix("- path:")?.trim();
    yaml_scalar_string(value)
}

fn yaml_block_contains_value(block: &[String], key: &str, expected: &str) -> bool {
    block
        .iter()
        .filter_map(|line| yaml_key_value(line, key))
        .any(|value| value == expected)
}

fn yaml_key_value(line: &str, key: &str) -> Option<String> {
    let prefix = format!("{key}:");
    let value = line.trim_start().strip_prefix(&prefix)?.trim();
    yaml_scalar_string(value)
}

fn yaml_block_proofs(block: &[String]) -> Vec<String> {
    let mut proofs = Vec::new();
    let mut proves_indent = None;
    for line in block {
        let trimmed = line.trim_start();
        if trimmed == "proves:" {
            proves_indent = Some(leading_spaces(line));
            continue;
        }
        let Some(indent) = proves_indent else {
            continue;
        };
        if trimmed.is_empty() {
            continue;
        }
        if leading_spaces(line) <= indent {
            proves_indent = None;
            continue;
        }
        if let Some(value) = trimmed.strip_prefix("- ").and_then(yaml_scalar_string) {
            proofs.push(value);
        }
    }
    proofs
}

fn yaml_scalar_string(value: &str) -> Option<String> {
    if value.is_empty() {
        return Some(String::new());
    }
    serde_yaml::from_str::<String>(value)
        .ok()
        .or_else(|| Some(value.trim_matches(['"', '\'']).to_string()))
}

fn leading_spaces(line: &str) -> usize {
    line.chars().take_while(|ch| *ch == ' ').count()
}

fn remove_cascade_test_case_range(
    path: &Path,
    record: &ObsoleteAssetRecord,
) -> Result<TestCaseRange> {
    let Some(source) = &record.cascade_source else {
        bail!(
            "registered obsolete test case lacks cascade source: {}",
            record.path
        );
    };
    let text = fs::read_to_string(path)
        .with_context(|| format!("unable to read obsolete test file: {}", path.display()))?;
    let range = cascade_test_range(record, &text).with_context(|| {
        format!(
            "registered obsolete test case could not be isolated in current file: {}",
            record.path
        )
    })?;
    let match_text = cascade_match_text(record, source);
    if !range_contains_exact_obsolete_source(&text, &range, match_text) {
        bail!(
            "registered obsolete test case no longer mentions cascade source {}: {}",
            match_text,
            record.path
        );
    }
    remove_line_range(path, range.line_start, range.line_end)?;
    Ok(range)
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

    fn write_feature_coverage_orphan_repo(root: &Path, mixed_current_proof: bool) {
        fs::create_dir_all(root.join("config")).unwrap();
        fs::create_dir_all(root.join("tests")).unwrap();
        fs::write(
            root.join("tests").join("contract.rs"),
            r#"#[test]
fn retired_feature_contract() {
    assert!(true);
}

#[test]
fn current_feature_contract() {
    assert!(true);
}
"#,
        )
        .unwrap();
        let current_proof = if mixed_current_proof {
            "          - current_claim\n"
        } else {
            ""
        };
        let claim_check = if mixed_current_proof {
            r#"
    claim_checks:
      - id: current_claim
        claim: Current claim remains active.
        implementation_anchors:
          - path: tests/contract.rs
            contains: "fn current_feature_contract"
        test_anchors:
          - path: tests/contract.rs
            contains: "fn current_feature_contract"
            proves:
              - current_claim
        validation_commands:
          - cargo test
"#
        } else {
            ""
        };
        fs::write(
            root.join(FEATURE_COVERAGE_MANIFEST),
            format!(
                r#"
features:
  - id: semantic_retirement
    title: Semantic retirement
    doc_anchors:
      - path: tests/contract.rs
        contains: "fn current_feature_contract"
    implementation_anchors:
      - path: tests/contract.rs
        contains: "fn current_feature_contract"
    test_anchors:
      - path: tests/contract.rs
        contains: "fn retired_feature_contract"
        proves:
{current_proof}          - retired_claim
    validation_commands:
      - cargo test
{claim_check}"#
            ),
        )
        .unwrap();
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
    fn directory_non_current_records_cover_child_paths() {
        let report = AssetRetirementReport {
            records: vec![ObsoleteAssetRecord {
                path: "references/legacy".into(),
                status: AssetStatus::CompatibilityExempt,
                ..ObsoleteAssetRecord::default()
            }],
            ..AssetRetirementReport::default()
        };

        assert!(report.is_non_current_path("references/legacy"));
        assert!(report.is_non_current_path("references/legacy/rule.md"));
        assert!(report.is_non_current_path("references\\legacy\\nested\\rule.md"));
        assert!(!report.is_non_current_path("references/legacy2/rule.md"));
        assert!(report.is_current_behavior_path("references/current/rule.md"));
    }

    #[test]
    fn line_level_cascade_records_do_not_mark_whole_test_file_non_current() {
        let report = AssetRetirementReport {
            records: vec![ObsoleteAssetRecord {
                path: "tests/contract.rs".into(),
                line: Some(3),
                line_end: Some(5),
                kind: "test".into(),
                cascade_source: Some("old.md".into()),
                status: AssetStatus::RetirementCandidate,
                ..ObsoleteAssetRecord::default()
            }],
            ..AssetRetirementReport::default()
        };

        assert!(report.non_current_paths().is_empty());
        assert!(report.is_current_behavior_path("tests/contract.rs"));
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
    fn scan_registers_test_case_cascade_candidate() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join("tests")).unwrap();
        fs::write(temp.path().join("old.md"), "old behavior\n").unwrap();
        fs::write(
            temp.path().join("tests").join("contract.rs"),
            r#"#[test]
fn old_behavior_contract() {
    assert!(include_str!("../old.md").contains("old"));
}

#[test]
fn current_behavior_contract() {
    assert!(true);
}
"#,
        )
        .unwrap();

        let report = manager(temp.path())
            .retire(AssetRetireRequest {
                path: PathBuf::from("old.md"),
                replacement_behavior: "current.md".into(),
                deletion_reason: "replaced".into(),
                validation_command: "cargo test".into(),
                apply_delete: false,
            })
            .unwrap();

        let candidate = report
            .cascade_candidates
            .iter()
            .find(|record| record.path == "tests/contract.rs")
            .expect("cascade test candidate");
        assert_eq!(
            candidate.cascade_test_name.as_deref(),
            Some("old_behavior_contract")
        );
        assert_eq!(candidate.cascade_source.as_deref(), Some("old.md"));
        assert!(candidate.line.is_some());
        assert!(
            report
                .cleanup_plan
                .iter()
                .any(|item| item.action == "delete_test_case")
        );
    }

    #[test]
    fn scan_registers_feature_coverage_orphan_test_anchor_cascade_candidate() {
        let temp = tempfile::tempdir().unwrap();
        write_feature_coverage_orphan_repo(temp.path(), false);

        let report = manager(temp.path()).scan().unwrap();

        let candidate = report
            .cascade_candidates
            .iter()
            .find(|record| record.path == "tests/contract.rs")
            .expect("semantic cascade test candidate");
        assert_eq!(
            candidate.cascade_anchor_contains.as_deref(),
            Some("fn retired_feature_contract")
        );
        assert_eq!(
            candidate.cascade_manifest_path.as_deref(),
            Some(FEATURE_COVERAGE_MANIFEST)
        );
        assert!(
            candidate
                .stale_behavior
                .contains("retired feature coverage surface")
        );
        assert!(candidate.line.is_some());
        assert!(
            report
                .cleanup_plan
                .iter()
                .any(|item| item.path == "tests/contract.rs" && item.action == "delete_test_case")
        );
    }

    #[test]
    fn cleanup_apply_deletes_cascaded_test_case_then_obsolete_file() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join("tests")).unwrap();
        fs::write(temp.path().join("old.md"), "old behavior\n").unwrap();
        let test_path = temp.path().join("tests").join("contract.rs");
        fs::write(
            &test_path,
            r#"#[test]
fn old_behavior_contract() {
    assert!(include_str!("../old.md").contains("old"));
}

#[test]
fn current_behavior_contract() {
    assert!(true);
}
"#,
        )
        .unwrap();
        let manager = manager(temp.path());
        manager
            .retire(AssetRetireRequest {
                path: PathBuf::from("old.md"),
                replacement_behavior: "current.md".into(),
                deletion_reason: "replaced".into(),
                validation_command: "cargo test".into(),
                apply_delete: false,
            })
            .unwrap();

        let report = manager
            .cleanup(AssetCleanupRequest { apply: true })
            .unwrap();

        let text = fs::read_to_string(&test_path).unwrap();
        assert!(!text.contains("old_behavior_contract"));
        assert!(text.contains("current_behavior_contract"));
        assert!(!temp.path().join("old.md").exists());
        assert!(
            report
                .records
                .iter()
                .any(|record| record.path == "old.md" && record.status == AssetStatus::Retired)
        );
        assert!(report.blockers.is_empty());
    }

    #[test]
    fn cleanup_apply_deletes_feature_coverage_orphan_test_and_anchor() {
        let temp = tempfile::tempdir().unwrap();
        write_feature_coverage_orphan_repo(temp.path(), false);
        let manager = manager(temp.path());
        manager.scan().unwrap();

        let report = manager
            .cleanup(AssetCleanupRequest { apply: true })
            .unwrap();

        let test_text = fs::read_to_string(temp.path().join("tests").join("contract.rs")).unwrap();
        assert!(!test_text.contains("retired_feature_contract"));
        assert!(test_text.contains("current_feature_contract"));
        let manifest_text =
            fs::read_to_string(temp.path().join(FEATURE_COVERAGE_MANIFEST)).unwrap();
        assert!(!manifest_text.contains("retired_claim"));
        assert!(!manifest_text.contains("fn retired_feature_contract"));
        assert!(manifest_text.contains("test_anchors: []"));
        assert!(report.blockers.is_empty());
    }

    #[test]
    fn cleanup_apply_deletes_feature_coverage_anchor_when_test_already_gone() {
        let temp = tempfile::tempdir().unwrap();
        write_feature_coverage_orphan_repo(temp.path(), false);
        fs::write(
            temp.path().join("tests").join("contract.rs"),
            r#"#[test]
fn current_feature_contract() {
    assert!(true);
}
"#,
        )
        .unwrap();
        let manager = manager(temp.path());

        let scanned = manager.scan().unwrap();

        assert!(scanned.cleanup_plan.iter().any(|item| {
            item.path == "tests/contract.rs" && item.action == "delete_feature_coverage_anchor"
        }));

        let report = manager
            .cleanup(AssetCleanupRequest { apply: true })
            .unwrap();

        let test_text = fs::read_to_string(temp.path().join("tests").join("contract.rs")).unwrap();
        assert!(test_text.contains("current_feature_contract"));
        let manifest_text =
            fs::read_to_string(temp.path().join(FEATURE_COVERAGE_MANIFEST)).unwrap();
        assert!(!manifest_text.contains("retired_claim"));
        assert!(!manifest_text.contains("fn retired_feature_contract"));
        assert!(manifest_text.contains("test_anchors: []"));
        assert!(report.blockers.is_empty());
    }

    #[test]
    fn mixed_feature_coverage_anchor_stays_blocked_when_test_already_gone() {
        let temp = tempfile::tempdir().unwrap();
        write_feature_coverage_orphan_repo(temp.path(), true);
        fs::write(
            temp.path().join("tests").join("contract.rs"),
            r#"#[test]
fn current_feature_contract() {
    assert!(true);
}
"#,
        )
        .unwrap();
        let manager = manager(temp.path());

        let scanned = manager.scan().unwrap();

        let candidate = scanned
            .cascade_candidates
            .iter()
            .find(|record| record.path == "tests/contract.rs")
            .expect("mixed semantic cascade candidate");
        assert!(candidate.cascade_has_current_proofs);
        assert!(scanned.cleanup_plan.iter().any(|item| {
            item.path == "tests/contract.rs" && item.action == "manual_test_case_prune_required"
        }));

        let cleanup = manager
            .cleanup(AssetCleanupRequest { apply: true })
            .unwrap();

        let manifest_text =
            fs::read_to_string(temp.path().join(FEATURE_COVERAGE_MANIFEST)).unwrap();
        assert!(manifest_text.contains("retired_claim"));
        assert!(manifest_text.contains("current_claim"));
        assert!(cleanup.blockers.iter().any(|item| {
            item.contains("retirement candidate") && item.contains("tests/contract.rs")
        }));
    }

    #[test]
    fn cleanup_apply_deletes_multiple_cascaded_test_cases_in_one_file() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join("tests")).unwrap();
        fs::write(temp.path().join("old.md"), "old behavior\n").unwrap();
        let test_path = temp.path().join("tests").join("contract.rs");
        fs::write(
            &test_path,
            r#"#[test]
fn old_behavior_one() {
    assert!(include_str!("../old.md").contains("old"));
}

#[test]
fn old_behavior_two() {
    assert!(include_str!("../old.md").contains("old"));
}

#[test]
fn current_behavior_contract() {
    assert!(true);
}
"#,
        )
        .unwrap();
        let manager = manager(temp.path());
        manager
            .retire(AssetRetireRequest {
                path: PathBuf::from("old.md"),
                replacement_behavior: "current.md".into(),
                deletion_reason: "replaced".into(),
                validation_command: "cargo test".into(),
                apply_delete: false,
            })
            .unwrap();

        let report = manager
            .cleanup(AssetCleanupRequest { apply: true })
            .unwrap();

        let text = fs::read_to_string(&test_path).unwrap();
        assert!(!text.contains("old_behavior_one"));
        assert!(!text.contains("old_behavior_two"));
        assert!(text.contains("current_behavior_contract"));
        assert_eq!(text.matches("#[test]").count(), 1);
        assert!(!temp.path().join("old.md").exists());
        assert!(report.blockers.is_empty());
    }

    #[test]
    fn cascade_range_ignores_braces_inside_strings_and_comments() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join("tests")).unwrap();
        fs::write(temp.path().join("old.md"), "old behavior\n").unwrap();
        let test_path = temp.path().join("tests").join("contract.rs");
        fs::write(
            &test_path,
            r##"#[test]
fn old_behavior_contract() {
    let _literal = "}";
    let _raw = r#"/* { not a block } */"#;
    // } not a block
    /* { not a block either } */
    assert!(include_str!("../old.md").contains("old"));
}

#[test]
fn current_behavior_contract() {
    assert!(true);
}
"##,
        )
        .unwrap();
        let manager = manager(temp.path());
        let scanned = manager
            .retire(AssetRetireRequest {
                path: PathBuf::from("old.md"),
                replacement_behavior: "current.md".into(),
                deletion_reason: "replaced".into(),
                validation_command: "cargo test".into(),
                apply_delete: false,
            })
            .unwrap();

        assert!(scanned.cascade_candidates.iter().any(|record| {
            record.cascade_test_name.as_deref() == Some("old_behavior_contract")
        }));

        let report = manager
            .cleanup(AssetCleanupRequest { apply: true })
            .unwrap();

        let text = fs::read_to_string(&test_path).unwrap();
        assert!(!text.contains("old_behavior_contract"));
        assert!(text.contains("current_behavior_contract"));
        assert!(!temp.path().join("old.md").exists());
        assert!(report.blockers.is_empty());
    }

    #[test]
    fn unisolated_test_reference_blocks_manual_prune() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join("tests")).unwrap();
        fs::write(temp.path().join("old.md"), "old behavior\n").unwrap();
        fs::write(
            temp.path().join("tests").join("contract.txt"),
            "legacy test fixture mentions old.md\n",
        )
        .unwrap();

        let report = manager(temp.path())
            .retire(AssetRetireRequest {
                path: PathBuf::from("old.md"),
                replacement_behavior: "current.md".into(),
                deletion_reason: "replaced".into(),
                validation_command: "cargo test".into(),
                apply_delete: false,
            })
            .unwrap();

        assert!(
            report
                .cleanup_plan
                .iter()
                .any(|item| item.action == "manual_test_case_prune_required")
        );
        assert!(report.blockers.iter().any(|item| {
            item.contains("retirement candidate") && item.contains("tests/contract.txt")
        }));
    }

    #[test]
    fn mixed_feature_coverage_orphan_proof_requires_manual_rewrite() {
        let temp = tempfile::tempdir().unwrap();
        write_feature_coverage_orphan_repo(temp.path(), true);
        let manager = manager(temp.path());

        let report = manager.scan().unwrap();

        let candidate = report
            .cascade_candidates
            .iter()
            .find(|record| record.path == "tests/contract.rs")
            .expect("mixed semantic cascade candidate");
        assert_eq!(candidate.line, None);
        assert!(
            candidate
                .stale_behavior
                .contains("mixes current proof with retired surface")
        );
        assert!(report.cleanup_plan.iter().any(|item| {
            item.path == "tests/contract.rs" && item.action == "manual_test_case_prune_required"
        }));

        let cleanup = manager
            .cleanup(AssetCleanupRequest { apply: true })
            .unwrap();
        let test_text = fs::read_to_string(temp.path().join("tests").join("contract.rs")).unwrap();
        assert!(test_text.contains("retired_feature_contract"));
        assert!(cleanup.blockers.iter().any(|item| {
            item.contains("retirement candidate") && item.contains("tests/contract.rs")
        }));
    }

    #[test]
    fn basename_only_test_reference_blocks_manual_prune_without_auto_delete() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join("docs")).unwrap();
        fs::create_dir_all(temp.path().join("tests")).unwrap();
        fs::write(temp.path().join("docs").join("README.md"), "old behavior\n").unwrap();
        let test_path = temp.path().join("tests").join("contract.rs");
        fs::write(
            &test_path,
            r#"#[test]
fn current_behavior_mentions_readme_name_only() {
    let label = "README.md";
    assert!(label.ends_with(".md"));
}
"#,
        )
        .unwrap();

        let manager = manager(temp.path());
        let report = manager
            .retire(AssetRetireRequest {
                path: PathBuf::from("docs/README.md"),
                replacement_behavior: "docs/current.md".into(),
                deletion_reason: "replaced".into(),
                validation_command: "cargo test".into(),
                apply_delete: false,
            })
            .unwrap();

        let candidate = report
            .cascade_candidates
            .iter()
            .find(|record| record.path == "tests/contract.rs")
            .expect("filename-only cascade candidate");
        assert_eq!(candidate.line, None);
        assert_eq!(candidate.cascade_test_name, None);
        assert!(
            report.cleanup_plan.iter().any(|item| {
                item.path == "tests/contract.rs" && item.action == "manual_test_case_prune_required"
            }),
            "filename-only references must not become delete_test_case actions"
        );

        let cleanup = manager
            .cleanup(AssetCleanupRequest { apply: true })
            .unwrap();

        let text = fs::read_to_string(&test_path).unwrap();
        assert!(text.contains("current_behavior_mentions_readme_name_only"));
        assert!(temp.path().join("docs").join("README.md").exists());
        assert!(cleanup.blockers.iter().any(|item| {
            item.contains("retirement candidate") && item.contains("tests/contract.rs")
        }));
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
