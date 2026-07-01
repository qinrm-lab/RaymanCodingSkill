use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

use crate::assets::{AssetRetirementManager, AssetRetirementReport};
use crate::{display_path, now_iso, read_text, yaml};

pub const FEATURE_COVERAGE_MANIFEST: &str = "config/feature_coverage.yaml";
pub const FEATURE_COVERAGE_MARKDOWN: &str = "docs/FEATURE_COVERAGE.md";

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct FeatureCoverageManifest {
    #[serde(default)]
    pub features: Vec<FeatureCoverageItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FeatureCoverageItem {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub strict_validation: bool,
    #[serde(default)]
    pub doc_anchors: Vec<CoverageAnchor>,
    #[serde(default)]
    pub implementation_anchors: Vec<CoverageAnchor>,
    #[serde(default)]
    pub test_anchors: Vec<CoverageAnchor>,
    #[serde(default)]
    pub validation_commands: Vec<String>,
    #[serde(default)]
    pub validation_records: Vec<ValidationRecord>,
    #[serde(default)]
    pub ui_surfaces: Vec<String>,
    #[serde(default)]
    pub public_commands: Vec<String>,
    #[serde(default)]
    pub api_endpoints: Vec<String>,
    #[serde(default)]
    pub claim_checks: Vec<ClaimCheck>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CoverageAnchor {
    pub path: String,
    pub contains: String,
    #[serde(default)]
    pub proves: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClaimCheck {
    pub id: String,
    pub claim: String,
    #[serde(default)]
    pub strict_validation: bool,
    #[serde(default)]
    pub doc_anchors: Vec<CoverageAnchor>,
    #[serde(default)]
    pub implementation_anchors: Vec<CoverageAnchor>,
    #[serde(default)]
    pub test_anchors: Vec<CoverageAnchor>,
    #[serde(default)]
    pub validation_commands: Vec<String>,
    #[serde(default)]
    pub validation_records: Vec<ValidationRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ValidationRecord {
    pub command: String,
    pub status: String,
    #[serde(default)]
    pub evidence_path: Option<String>,
    #[serde(default)]
    pub evidence_contains: Vec<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FeatureCoverageReport {
    pub workspace_path: PathBuf,
    pub generated_at: String,
    pub manifest_path: PathBuf,
    pub status: String,
    pub strict: bool,
    pub feature_count: usize,
    pub finding_count: usize,
    pub findings: Vec<FeatureCoverageFinding>,
    pub covered_document_paths: Vec<String>,
    pub expected_document_paths: Vec<String>,
    pub documented_public_commands: Vec<String>,
    pub implemented_public_commands: Vec<String>,
    pub registered_public_commands: Vec<String>,
    pub documented_api_endpoints: Vec<String>,
    pub implemented_api_endpoints: Vec<String>,
    pub registered_api_endpoints: Vec<String>,
    pub required_actions: Vec<String>,
    pub features: Vec<FeatureCoverageItem>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FeatureCoverageOptions {
    pub strict: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FeatureCoverageFinding {
    pub feature_id: Option<String>,
    pub path: Option<PathBuf>,
    pub line: usize,
    pub kind: String,
    pub message: String,
}

struct ValidationRecordCheck<'a> {
    scope: &'a str,
    scope_id: &'a str,
    strict_enabled: bool,
    validation_commands: &'a [String],
    validation_records: &'a [ValidationRecord],
    freshness_anchors: &'a [&'a [CoverageAnchor]],
    asset_retirement: Option<&'a AssetRetirementReport>,
}

pub fn load_manifest(root: &Path) -> Result<FeatureCoverageManifest> {
    let manifest_path = root.join(FEATURE_COVERAGE_MANIFEST);
    let text = read_text(&manifest_path).with_context(|| {
        format!("missing feature coverage manifest: {FEATURE_COVERAGE_MANIFEST}")
    })?;
    yaml::from_str(&text)
        .with_context(|| format!("invalid feature coverage manifest: {FEATURE_COVERAGE_MANIFEST}"))
}

pub fn check_feature_coverage(root: &Path) -> Result<FeatureCoverageReport> {
    check_feature_coverage_with_options(root, FeatureCoverageOptions::default())
}

pub fn check_feature_coverage_with_options(
    root: &Path,
    options: FeatureCoverageOptions,
) -> Result<FeatureCoverageReport> {
    let root = canonical_or_current(root)?;
    let manifest_path = root.join(FEATURE_COVERAGE_MANIFEST);
    let generated_at = now_iso();
    let manifest = match load_manifest(&root) {
        Ok(manifest) => manifest,
        Err(error) => {
            let finding = FeatureCoverageFinding {
                feature_id: None,
                path: Some(manifest_path.clone()),
                line: 1,
                kind: "feature_coverage_manifest".into(),
                message: error.to_string(),
            };
            return Ok(report_from_parts(
                &root,
                manifest_path,
                generated_at,
                FeatureCoverageManifest::default(),
                vec![finding],
                options,
            ));
        }
    };

    let mut findings = Vec::new();
    let asset_retirement = current_behavior_asset_report(&root, &mut findings);
    validate_manifest_shape(
        &root,
        &manifest,
        options,
        asset_retirement.as_ref(),
        &mut findings,
    );
    validate_document_coverage(&root, &manifest, asset_retirement.as_ref(), &mut findings)?;
    validate_public_commands(&root, &manifest, asset_retirement.as_ref(), &mut findings)?;
    validate_api_endpoints(&root, &manifest, asset_retirement.as_ref(), &mut findings)?;

    Ok(report_from_parts(
        &root,
        manifest_path,
        generated_at,
        manifest,
        findings,
        options,
    ))
}

pub fn assert_feature_coverage(root: &Path) -> Result<FeatureCoverageReport> {
    let report =
        check_feature_coverage_with_options(root, FeatureCoverageOptions { strict: true })?;
    if report.status == "passed" {
        return Ok(report);
    }
    bail!(
        "feature coverage gate failed:\n{}",
        format_feature_coverage_findings(&report)
    );
}

pub fn format_feature_coverage_findings(report: &FeatureCoverageReport) -> String {
    report
        .findings
        .iter()
        .map(|finding| {
            let path = finding
                .path
                .as_ref()
                .map(|path| display_path(path))
                .unwrap_or_else(|| "<manifest>".into());
            let id = finding.feature_id.as_deref().unwrap_or("feature_coverage");
            format!(
                "{}:{} {} [{}] - {}",
                path, finding.line, finding.kind, id, finding.message
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn render_feature_coverage_markdown(report: &FeatureCoverageReport) -> String {
    let mut text = String::new();
    text.push_str("# Feature Coverage\n\n");
    text.push_str(
        "This document is generated from `config/feature_coverage.yaml`. It maps public documentation claims to implementation anchors, test anchors, validation commands, and UI surfaces.\n\n",
    );
    text.push_str("## Summary\n\n");
    text.push_str(&format!("- Status: `{}`\n", report.status));
    text.push_str(&format!("- Strict validation: `{}`\n", report.strict));
    text.push_str(&format!("- Features: `{}`\n", report.feature_count));
    text.push_str(&format!("- Findings: `{}`\n", report.finding_count));
    text.push_str(&format!(
        "- Documented CLI commands: `{}`\n",
        report.documented_public_commands.len()
    ));
    text.push_str(&format!(
        "- Implemented CLI commands: `{}`\n",
        report.implemented_public_commands.len()
    ));
    text.push_str(&format!(
        "- Documented API endpoints: `{}`\n\n",
        report.documented_api_endpoints.len()
    ));
    text.push_str("## Matrix\n\n");
    text.push_str("| ID | Docs | Implementation | Tests | Validation | Claims | UI |\n");
    text.push_str("| --- | --- | --- | --- | --- | --- | --- |\n");
    for feature in &report.features {
        text.push_str(&format!(
            "| `{}` | {} | {} | {} | {} | {} | {} |\n",
            escape_markdown_table(&feature.id),
            anchor_summary(&feature.doc_anchors),
            anchor_summary(&feature.implementation_anchors),
            anchor_summary(&feature.test_anchors),
            list_summary(&feature.validation_commands),
            claim_summary(&feature.claim_checks),
            list_summary(&feature.ui_surfaces)
        ));
    }
    if !report.findings.is_empty() {
        text.push_str("\n## Findings\n\n");
        for finding in &report.findings {
            text.push_str(&format!(
                "- `{}`: {}\n",
                finding.kind,
                escape_markdown_table(&finding.message)
            ));
        }
    }
    text
}

pub fn documented_public_commands(root: &Path) -> Result<Vec<String>> {
    let asset_retirement = AssetRetirementManager::new(root)?.status()?;
    documented_public_commands_with_retirement(root, Some(&asset_retirement))
}

fn documented_public_commands_with_retirement(
    root: &Path,
    asset_retirement: Option<&AssetRetirementReport>,
) -> Result<Vec<String>> {
    if !is_current_behavior_path(asset_retirement, "docs/CLI.md") {
        return Ok(Vec::new());
    }
    let text = read_text(&root.join("docs").join("CLI.md"))?;
    let mut commands = BTreeSet::new();
    for line in text.lines() {
        if let Some(command) = extract_rayman_command(line) {
            commands.insert(command);
        }
    }
    Ok(commands.into_iter().collect())
}

pub fn documented_api_endpoints(root: &Path) -> Result<Vec<String>> {
    let asset_retirement = AssetRetirementManager::new(root)?.status()?;
    documented_api_endpoints_with_retirement(root, Some(&asset_retirement))
}

fn documented_api_endpoints_with_retirement(
    root: &Path,
    asset_retirement: Option<&AssetRetirementReport>,
) -> Result<Vec<String>> {
    if !is_current_behavior_path(asset_retirement, "docs/API.md") {
        return Ok(Vec::new());
    }
    let text = read_text(&root.join("docs").join("API.md"))?;
    let mut endpoints = BTreeSet::new();
    for line in text.lines() {
        for segment in line.split('`').skip(1).step_by(2) {
            if is_endpoint_spec(segment) {
                endpoints.insert(segment.trim().to_string());
            }
        }
    }
    Ok(endpoints.into_iter().collect())
}

pub fn implemented_public_commands(root: &Path) -> Result<Vec<String>> {
    let asset_retirement = AssetRetirementManager::new(root)?.status()?;
    implemented_public_commands_with_retirement(root, Some(&asset_retirement))
}

pub fn implemented_api_endpoints(root: &Path) -> Result<Vec<String>> {
    let asset_retirement = AssetRetirementManager::new(root)?.status()?;
    implemented_api_endpoints_with_retirement(root, Some(&asset_retirement))
}

fn implemented_public_commands_with_retirement(
    root: &Path,
    asset_retirement: Option<&AssetRetirementReport>,
) -> Result<Vec<String>> {
    let text = read_rust_sources(
        root,
        &root.join("crates").join("rayman-cli").join("src"),
        asset_retirement,
    )?;
    Ok(extract_cli_command_paths(&text).into_iter().collect())
}

fn implemented_api_endpoints_with_retirement(
    root: &Path,
    asset_retirement: Option<&AssetRetirementReport>,
) -> Result<Vec<String>> {
    let text = read_rust_sources(
        root,
        &root.join("crates").join("rayman-api").join("src"),
        asset_retirement,
    )?;
    let mut endpoints = BTreeSet::new();
    let mut in_route = false;
    let mut route_balance = 0isize;
    let mut route_block = String::new();
    for line in text.lines() {
        if !in_route && line.contains(".route(") {
            in_route = true;
            route_balance = 0;
            route_block.clear();
        }
        if in_route {
            route_block.push_str(line);
            route_block.push('\n');
            route_balance += paren_delta(line);
            if route_balance <= 0 {
                if let Some(path) = quoted_string(&route_block) {
                    for method in route_methods(&route_block) {
                        endpoints.insert(format!("{method} {path}"));
                    }
                }
                in_route = false;
                route_balance = 0;
                route_block.clear();
            }
        }
    }
    Ok(endpoints.into_iter().collect())
}

fn read_rust_sources(
    root: &Path,
    dir: &Path,
    asset_retirement: Option<&AssetRetirementReport>,
) -> Result<String> {
    let mut paths = Vec::new();
    if !dir.exists() {
        return Ok(String::new());
    }
    for entry in WalkDir::new(dir).into_iter().filter_map(|entry| entry.ok()) {
        if entry.file_type().is_file()
            && entry
                .path()
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext == "rs")
        {
            let relative = relative_slash(root, entry.path());
            if is_current_behavior_path(asset_retirement, &relative) {
                paths.push(entry.path().to_path_buf());
            }
        }
    }
    paths.sort();
    let mut text = String::new();
    for path in paths {
        text.push_str(&read_text(&path)?);
        text.push('\n');
    }
    Ok(text)
}

fn validate_manifest_shape(
    root: &Path,
    manifest: &FeatureCoverageManifest,
    options: FeatureCoverageOptions,
    asset_retirement: Option<&AssetRetirementReport>,
    findings: &mut Vec<FeatureCoverageFinding>,
) {
    if manifest.features.is_empty() {
        findings.push(FeatureCoverageFinding {
            feature_id: None,
            path: Some(root.join(FEATURE_COVERAGE_MANIFEST)),
            line: 1,
            kind: "feature_coverage_empty".into(),
            message: "feature coverage manifest must contain at least one feature".into(),
        });
    }
    let mut ids = BTreeSet::new();
    for feature in &manifest.features {
        if feature.id.trim().is_empty() {
            push_feature_finding(
                root,
                findings,
                feature,
                "feature_id_empty",
                "feature id must not be empty",
            );
        } else if !ids.insert(feature.id.clone()) {
            push_feature_finding(
                root,
                findings,
                feature,
                "feature_id_duplicate",
                "feature id must be unique",
            );
        }
        if feature.title.trim().is_empty() {
            push_feature_finding(
                root,
                findings,
                feature,
                "feature_title_empty",
                "feature title must not be empty",
            );
        }
        validate_anchor_group(
            root,
            findings,
            feature,
            "doc_anchor",
            &feature.doc_anchors,
            asset_retirement,
        );
        validate_anchor_group(
            root,
            findings,
            feature,
            "implementation_anchor",
            &feature.implementation_anchors,
            asset_retirement,
        );
        validate_anchor_group(
            root,
            findings,
            feature,
            "test_anchor",
            &feature.test_anchors,
            asset_retirement,
        );
        if feature.validation_commands.is_empty() {
            push_feature_finding(
                root,
                findings,
                feature,
                "validation_commands_missing",
                "feature must list at least one validation command",
            );
        }
        validate_validation_records(
            root,
            findings,
            feature,
            ValidationRecordCheck {
                scope: "feature",
                scope_id: &feature.id,
                strict_enabled: options.strict && feature.strict_validation,
                validation_commands: &feature.validation_commands,
                validation_records: &feature.validation_records,
                freshness_anchors: &[
                    &feature.doc_anchors,
                    &feature.implementation_anchors,
                    &feature.test_anchors,
                ],
                asset_retirement,
            },
        );
        validate_feature_proofs(root, findings, feature);
        validate_claim_checks(root, options, findings, feature, asset_retirement);
        validate_ui_surfaces(root, findings, feature, asset_retirement);
    }
}

fn validate_feature_proofs(
    root: &Path,
    findings: &mut Vec<FeatureCoverageFinding>,
    feature: &FeatureCoverageItem,
) {
    let proofs = feature_proofs(feature);
    for command in &feature.public_commands {
        if !proofs.contains(command) {
            push_feature_finding(
                root,
                findings,
                feature,
                "test_anchor_unproven_public_command",
                &format!(
                    "public command `{command}` must be named in test_anchors.proves by a semantic test"
                ),
            );
        }
    }
    for endpoint in &feature.api_endpoints {
        if !proofs.contains(endpoint) {
            push_feature_finding(
                root,
                findings,
                feature,
                "test_anchor_unproven_api_endpoint",
                &format!(
                    "API endpoint `{endpoint}` must be named in test_anchors.proves by a semantic test"
                ),
            );
        }
    }
    for claim in &feature.claim_checks {
        if !proofs.contains(&claim.id) && !proofs.contains(&claim.claim) {
            push_feature_finding(
                root,
                findings,
                feature,
                "test_anchor_unproven_claim",
                &format!(
                    "claim check `{}` must be named in test_anchors.proves by a semantic test",
                    claim.id
                ),
            );
        }
    }
}

fn validate_claim_checks(
    root: &Path,
    options: FeatureCoverageOptions,
    findings: &mut Vec<FeatureCoverageFinding>,
    feature: &FeatureCoverageItem,
    asset_retirement: Option<&AssetRetirementReport>,
) {
    let mut ids = BTreeSet::new();
    for claim in &feature.claim_checks {
        if claim.id.trim().is_empty() {
            push_feature_finding(
                root,
                findings,
                feature,
                "claim_check_id_empty",
                "claim check id must not be empty",
            );
        } else if !ids.insert(claim.id.clone()) {
            push_feature_finding(
                root,
                findings,
                feature,
                "claim_check_id_duplicate",
                &format!(
                    "claim check id must be unique within the feature: {}",
                    claim.id
                ),
            );
        }
        if claim.claim.trim().is_empty() {
            push_feature_finding(
                root,
                findings,
                feature,
                "claim_check_empty",
                &format!("claim check `{}` must describe the public claim", claim.id),
            );
        }
        validate_optional_anchor_group(
            root,
            findings,
            feature,
            "claim_doc_anchor",
            &claim.doc_anchors,
            asset_retirement,
        );
        validate_anchor_group(
            root,
            findings,
            feature,
            "claim_implementation_anchor",
            &claim.implementation_anchors,
            asset_retirement,
        );
        validate_anchor_group(
            root,
            findings,
            feature,
            "claim_test_anchor",
            &claim.test_anchors,
            asset_retirement,
        );
        if claim.validation_commands.is_empty() {
            push_feature_finding(
                root,
                findings,
                feature,
                "claim_validation_commands_missing",
                &format!("claim check `{}` must list validation commands", claim.id),
            );
        }
        validate_validation_records(
            root,
            findings,
            feature,
            ValidationRecordCheck {
                scope: "claim",
                scope_id: &claim.id,
                strict_enabled: options.strict && claim.strict_validation,
                validation_commands: &claim.validation_commands,
                validation_records: &claim.validation_records,
                freshness_anchors: &[
                    &claim.doc_anchors,
                    &claim.implementation_anchors,
                    &claim.test_anchors,
                ],
                asset_retirement,
            },
        );
        let claim_proofs = claim
            .test_anchors
            .iter()
            .flat_map(|anchor| anchor.proves.iter())
            .map(|proof| proof.trim())
            .collect::<BTreeSet<_>>();
        if !claim_proofs.contains(claim.id.as_str()) && !claim_proofs.contains(claim.claim.as_str())
        {
            push_feature_finding(
                root,
                findings,
                feature,
                "claim_test_anchor_proves_missing",
                &format!(
                    "claim check `{}` must have a test anchor whose proves includes the claim id",
                    claim.id
                ),
            );
        }
    }
}

fn validate_validation_records(
    root: &Path,
    findings: &mut Vec<FeatureCoverageFinding>,
    feature: &FeatureCoverageItem,
    check: ValidationRecordCheck<'_>,
) {
    let ValidationRecordCheck {
        scope,
        scope_id,
        strict_enabled,
        validation_commands,
        validation_records,
        freshness_anchors,
        asset_retirement,
    } = check;
    if !strict_enabled {
        return;
    }
    if validation_records.is_empty() {
        push_feature_finding(
            root,
            findings,
            feature,
            &format!("{scope}_validation_records_missing"),
            &format!("strict {scope} `{scope_id}` must list passed validation records"),
        );
        return;
    }

    let commands = validation_commands
        .iter()
        .map(|command| command.trim().to_string())
        .filter(|command| !command.is_empty())
        .collect::<BTreeSet<_>>();
    for command in &commands {
        if !validation_records
            .iter()
            .any(|record| record.command.trim() == command)
        {
            push_feature_finding(
                root,
                findings,
                feature,
                &format!("{scope}_validation_record_missing"),
                &format!(
                    "strict {scope} `{scope_id}` lacks a passed validation record for `{command}`"
                ),
            );
        }
    }

    for record in validation_records {
        let command = record.command.trim();
        if command.is_empty() {
            push_feature_finding(
                root,
                findings,
                feature,
                &format!("{scope}_validation_record_command_empty"),
                &format!("strict {scope} `{scope_id}` has an empty validation record command"),
            );
            continue;
        }
        if !commands.contains(command) {
            push_feature_finding(
                root,
                findings,
                feature,
                &format!("{scope}_validation_record_unknown_command"),
                &format!(
                    "strict {scope} `{scope_id}` validation record is not listed in validation_commands: `{command}`"
                ),
            );
        }
        if record.status.trim() != "passed" {
            push_feature_finding(
                root,
                findings,
                feature,
                &format!("{scope}_validation_record_not_passed"),
                &format!(
                    "strict {scope} `{scope_id}` validation record for `{command}` must have status `passed`"
                ),
            );
        }
        let updated_at = match record
            .updated_at
            .as_deref()
            .map(str::trim)
            .filter(|updated_at| !updated_at.is_empty())
        {
            Some(updated_at) => match DateTime::parse_from_rfc3339(updated_at) {
                Ok(parsed) => Some(parsed.with_timezone(&Utc)),
                Err(error) => {
                    push_feature_finding(
                        root,
                        findings,
                        feature,
                        &format!("{scope}_validation_record_updated_at_invalid"),
                        &format!(
                            "strict {scope} `{scope_id}` validation record for `{command}` has invalid updated_at: {error}"
                        ),
                    );
                    None
                }
            },
            None => {
                push_feature_finding(
                    root,
                    findings,
                    feature,
                    &format!("{scope}_validation_record_updated_at_missing"),
                    &format!(
                        "strict {scope} `{scope_id}` validation record for `{command}` must include updated_at"
                    ),
                );
                None
            }
        };
        let Some(evidence_path) = record
            .evidence_path
            .as_deref()
            .map(str::trim)
            .filter(|path| !path.is_empty())
        else {
            push_feature_finding(
                root,
                findings,
                feature,
                &format!("{scope}_validation_record_evidence_missing"),
                &format!(
                    "strict {scope} `{scope_id}` validation record for `{command}` must include evidence_path"
                ),
            );
            continue;
        };
        let path = match resolve_workspace_relative(root, evidence_path) {
            Ok(path) => path,
            Err(error) => {
                push_feature_finding(
                    root,
                    findings,
                    feature,
                    &format!("{scope}_validation_record_evidence_invalid_path"),
                    &format!(
                        "strict {scope} `{scope_id}` validation record for `{command}` has invalid evidence_path: {error}"
                    ),
                );
                continue;
            }
        };
        if !is_current_behavior_path(asset_retirement, evidence_path) {
            push_feature_finding(
                root,
                findings,
                feature,
                &format!("{scope}_validation_record_evidence_non_current_asset"),
                &format!(
                    "strict {scope} `{scope_id}` validation evidence for `{command}` points to non-current asset `{evidence_path}`"
                ),
            );
            continue;
        }
        let text = match read_text(&path) {
            Ok(text) => text,
            Err(error) => {
                push_feature_finding(
                    root,
                    findings,
                    feature,
                    &format!("{scope}_validation_record_evidence_missing_file"),
                    &format!(
                        "strict {scope} `{scope_id}` validation evidence for `{command}` is unreadable: {error}"
                    ),
                );
                continue;
            }
        };
        if !text.contains(command) {
            push_feature_finding(
                root,
                findings,
                feature,
                &format!("{scope}_validation_record_evidence_missing_command"),
                &format!(
                    "strict {scope} `{scope_id}` validation evidence for `{command}` must include the exact command"
                ),
            );
        }
        if !evidence_text_has_passed_status(&text) {
            push_feature_finding(
                root,
                findings,
                feature,
                &format!("{scope}_validation_record_evidence_missing_passed_status"),
                &format!(
                    "strict {scope} `{scope_id}` validation evidence for `{command}` must include a passed execution status"
                ),
            );
        }
        if let Some(updated_at) = updated_at
            && let Some(path) =
                validate_record_freshness(root, updated_at, freshness_anchors, asset_retirement)
        {
            push_feature_finding(
                root,
                findings,
                feature,
                &format!("{scope}_validation_record_stale"),
                &format!(
                    "strict {scope} `{scope_id}` validation record for `{command}` is older than anchor `{path}`"
                ),
            );
        }
        let required_needles = if record.evidence_contains.is_empty() {
            vec![command.to_string()]
        } else {
            record
                .evidence_contains
                .iter()
                .map(|needle| needle.trim().to_string())
                .filter(|needle| !needle.is_empty())
                .collect::<Vec<_>>()
        };
        if required_needles.is_empty() {
            push_feature_finding(
                root,
                findings,
                feature,
                &format!("{scope}_validation_record_evidence_empty_check"),
                &format!(
                    "strict {scope} `{scope_id}` validation record for `{command}` must include non-empty evidence text"
                ),
            );
            continue;
        }
        for needle in required_needles {
            if !text.contains(&needle) {
                push_feature_finding(
                    root,
                    findings,
                    feature,
                    &format!("{scope}_validation_record_evidence_missing_text"),
                    &format!(
                        "strict {scope} `{scope_id}` validation evidence for `{command}` does not contain `{needle}`"
                    ),
                );
            }
        }
    }
}

fn evidence_text_has_passed_status(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    let compact = lower.split_whitespace().collect::<String>();
    lower.contains("status: passed")
        || lower.contains("status: `passed`")
        || lower.contains("status=passed")
        || lower.contains("result: passed")
        || compact.contains("\"status\":\"passed\"")
}

fn validate_record_freshness(
    root: &Path,
    updated_at: DateTime<Utc>,
    freshness_anchors: &[&[CoverageAnchor]],
    asset_retirement: Option<&AssetRetirementReport>,
) -> Option<String> {
    let newest_anchor = freshness_anchors
        .iter()
        .flat_map(|anchors| anchors.iter())
        .filter_map(|anchor| {
            if !is_current_behavior_path(asset_retirement, &anchor.path) {
                return None;
            }
            let path = resolve_workspace_relative(root, &anchor.path).ok()?;
            let modified = fs::metadata(&path).ok()?.modified().ok()?;
            Some((anchor.path.as_str(), DateTime::<Utc>::from(modified)))
        })
        .max_by_key(|(_, modified)| *modified);

    newest_anchor
        .filter(|(_, modified)| updated_at < *modified)
        .map(|(path, _)| path.to_string())
}

fn validate_optional_anchor_group(
    root: &Path,
    findings: &mut Vec<FeatureCoverageFinding>,
    feature: &FeatureCoverageItem,
    kind: &str,
    anchors: &[CoverageAnchor],
    asset_retirement: Option<&AssetRetirementReport>,
) {
    if !anchors.is_empty() {
        validate_anchor_group(root, findings, feature, kind, anchors, asset_retirement);
    }
}

fn validate_anchor_group(
    root: &Path,
    findings: &mut Vec<FeatureCoverageFinding>,
    feature: &FeatureCoverageItem,
    kind: &str,
    anchors: &[CoverageAnchor],
    asset_retirement: Option<&AssetRetirementReport>,
) {
    if anchors.is_empty() {
        push_feature_finding(
            root,
            findings,
            feature,
            &format!("{kind}_missing"),
            &format!("feature must list at least one {kind}"),
        );
        return;
    }
    for anchor in anchors {
        if !is_current_behavior_path(asset_retirement, &anchor.path) {
            findings.push(FeatureCoverageFinding {
                feature_id: Some(feature.id.clone()),
                path: Some(root.join(&anchor.path)),
                line: 1,
                kind: format!("{kind}_non_current_asset"),
                message: format!(
                    "anchor points to non-current obsolete asset and cannot prove current behavior: {}",
                    anchor.path
                ),
            });
            continue;
        }
        match resolve_workspace_relative(root, &anchor.path) {
            Ok(path) => match read_text(&path) {
                Ok(text) => {
                    if first_line_containing(&text, &anchor.contains).is_none() {
                        findings.push(FeatureCoverageFinding {
                            feature_id: Some(feature.id.clone()),
                            path: Some(path),
                            line: 1,
                            kind: format!("{kind}_missing_text"),
                            message: format!(
                                "anchor text not found in {}: {}",
                                anchor.path, anchor.contains
                            ),
                        });
                    }
                }
                Err(error) => findings.push(FeatureCoverageFinding {
                    feature_id: Some(feature.id.clone()),
                    path: Some(path),
                    line: 1,
                    kind: format!("{kind}_missing_file"),
                    message: error.to_string(),
                }),
            },
            Err(error) => findings.push(FeatureCoverageFinding {
                feature_id: Some(feature.id.clone()),
                path: Some(root.join(FEATURE_COVERAGE_MANIFEST)),
                line: 1,
                kind: format!("{kind}_invalid_path"),
                message: error.to_string(),
            }),
        }
    }
}

fn validate_ui_surfaces(
    root: &Path,
    findings: &mut Vec<FeatureCoverageFinding>,
    feature: &FeatureCoverageItem,
    asset_retirement: Option<&AssetRetirementReport>,
) {
    for surface in &feature.ui_surfaces {
        if !matches!(surface.as_str(), "cli" | "api_json" | "html_docs") {
            push_feature_finding(
                root,
                findings,
                feature,
                "ui_surface_unknown",
                &format!("unsupported UI surface: {surface}"),
            );
            continue;
        }
        let marker = format!("@ui:{surface}");
        let has_marker = feature.test_anchors.iter().any(|anchor| {
            if !is_current_behavior_path(asset_retirement, &anchor.path) {
                return false;
            }
            resolve_workspace_relative(root, &anchor.path)
                .ok()
                .and_then(|path| read_text(&path).ok())
                .is_some_and(|text| text.contains(&marker))
        });
        if !has_marker {
            push_feature_finding(
                root,
                findings,
                feature,
                "ui_contract_missing",
                &format!("UI surface `{surface}` requires a test anchor containing `{marker}`"),
            );
        }
    }
}

fn validate_document_coverage(
    root: &Path,
    manifest: &FeatureCoverageManifest,
    asset_retirement: Option<&AssetRetirementReport>,
    findings: &mut Vec<FeatureCoverageFinding>,
) -> Result<()> {
    let covered = manifest
        .features
        .iter()
        .flat_map(|feature| feature.doc_anchors.iter())
        .map(|anchor| normalize_slash(&anchor.path))
        .filter(|path| is_current_behavior_path(asset_retirement, path))
        .collect::<BTreeSet<_>>();
    for expected in expected_document_paths_with_retirement(root, asset_retirement)? {
        if !covered.contains(&expected) {
            findings.push(FeatureCoverageFinding {
                feature_id: None,
                path: Some(root.join(&expected)),
                line: 1,
                kind: "document_unmapped".into(),
                message: format!(
                    "governance document is not mapped by any feature coverage doc anchor: {expected}"
                ),
            });
        }
    }
    Ok(())
}

fn validate_public_commands(
    root: &Path,
    manifest: &FeatureCoverageManifest,
    asset_retirement: Option<&AssetRetirementReport>,
    findings: &mut Vec<FeatureCoverageFinding>,
) -> Result<()> {
    let registered = registered_public_commands(manifest);
    for command in &registered {
        if !command.starts_with("rayman ") {
            findings.push(FeatureCoverageFinding {
                feature_id: None,
                path: Some(root.join(FEATURE_COVERAGE_MANIFEST)),
                line: 1,
                kind: "public_command_invalid".into(),
                message: format!("public command must start with `rayman `: {command}"),
            });
        }
    }
    for command in documented_public_commands_with_retirement(root, asset_retirement)? {
        if !command_documented_by_registered_surface(&command, &registered) {
            findings.push(FeatureCoverageFinding {
                feature_id: None,
                path: Some(root.join("docs").join("CLI.md")),
                line: 1,
                kind: "public_command_unmapped".into(),
                message: format!(
                    "documented CLI command is not registered in feature coverage: {command}"
                ),
            });
        }
    }
    for command in implemented_public_commands_with_retirement(root, asset_retirement)? {
        if !command_surface_registered(&command, &registered) {
            findings.push(FeatureCoverageFinding {
                feature_id: None,
                path: Some(
                    root.join("crates")
                        .join("rayman-cli")
                        .join("src")
                        .join("main.rs"),
                ),
                line: 1,
                kind: "public_command_source_unmapped".into(),
                message: format!(
                    "implemented CLI command is not registered in feature coverage: {command}"
                ),
            });
        }
    }
    Ok(())
}

fn validate_api_endpoints(
    root: &Path,
    manifest: &FeatureCoverageManifest,
    asset_retirement: Option<&AssetRetirementReport>,
    findings: &mut Vec<FeatureCoverageFinding>,
) -> Result<()> {
    let registered = registered_api_endpoints(manifest);
    for endpoint in documented_api_endpoints_with_retirement(root, asset_retirement)? {
        if !registered.contains(&endpoint) {
            findings.push(FeatureCoverageFinding {
                feature_id: None,
                path: Some(root.join("docs").join("API.md")),
                line: 1,
                kind: "api_endpoint_unmapped".into(),
                message: format!(
                    "documented API endpoint is not registered in feature coverage: {endpoint}"
                ),
            });
        }
    }
    for endpoint in implemented_api_endpoints_with_retirement(root, asset_retirement)? {
        if !registered.contains(&endpoint) {
            findings.push(FeatureCoverageFinding {
                feature_id: None,
                path: Some(
                    root.join("crates")
                        .join("rayman-api")
                        .join("src")
                        .join("lib.rs"),
                ),
                line: 1,
                kind: "api_route_unmapped".into(),
                message: format!(
                    "implemented API route is not registered in feature coverage: {endpoint}"
                ),
            });
        }
    }
    Ok(())
}

fn report_from_parts(
    root: &Path,
    manifest_path: PathBuf,
    generated_at: String,
    manifest: FeatureCoverageManifest,
    findings: Vec<FeatureCoverageFinding>,
    options: FeatureCoverageOptions,
) -> FeatureCoverageReport {
    let asset_retirement = AssetRetirementManager::new(root)
        .and_then(|manager| manager.status())
        .ok();
    let asset_retirement = asset_retirement.as_ref();
    let status = if findings.is_empty() {
        "passed"
    } else {
        "failed"
    };
    let documented_public_commands =
        documented_public_commands_with_retirement(root, asset_retirement).unwrap_or_default();
    let implemented_public_commands =
        implemented_public_commands_with_retirement(root, asset_retirement).unwrap_or_default();
    let registered_public_commands = registered_public_commands(&manifest).into_iter().collect();
    let documented_api_endpoints =
        documented_api_endpoints_with_retirement(root, asset_retirement).unwrap_or_default();
    let implemented_api_endpoints =
        implemented_api_endpoints_with_retirement(root, asset_retirement).unwrap_or_default();
    let registered_api_endpoints = registered_api_endpoints(&manifest).into_iter().collect();
    let covered_document_paths = manifest
        .features
        .iter()
        .flat_map(|feature| feature.doc_anchors.iter())
        .map(|anchor| normalize_slash(&anchor.path))
        .filter(|path| is_current_behavior_path(asset_retirement, path))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let expected_document_paths =
        expected_document_paths_with_retirement(root, asset_retirement).unwrap_or_default();
    let required_actions = if findings.is_empty() {
        vec!["Feature coverage matrix is current.".into()]
    } else {
        vec![
            "Update config/feature_coverage.yaml so every current public documentation claim maps to implementation and tests.".into(),
            "For strict validation records, attach passed command records with evidence_path and evidence_contains text.".into(),
            "Add deterministic UI contract tests for cli, api_json, or html_docs surfaces before marking coverage current.".into(),
        ]
    };
    let feature_count = manifest.features.len();
    FeatureCoverageReport {
        workspace_path: root.to_path_buf(),
        generated_at,
        manifest_path,
        status: status.into(),
        strict: options.strict,
        feature_count,
        finding_count: findings.len(),
        findings,
        covered_document_paths,
        expected_document_paths,
        documented_public_commands,
        implemented_public_commands,
        registered_public_commands,
        documented_api_endpoints,
        implemented_api_endpoints,
        registered_api_endpoints,
        required_actions,
        features: manifest.features,
    }
}

fn expected_document_paths_with_retirement(
    root: &Path,
    asset_retirement: Option<&AssetRetirementReport>,
) -> Result<Vec<String>> {
    let mut paths = BTreeSet::new();
    for relative in ["README.md", "QUICKSTART.md", "SKILL.md"] {
        if root.join(relative).exists() && is_current_behavior_path(asset_retirement, relative) {
            paths.insert(relative.to_string());
        }
    }
    for directory in ["docs", "references"] {
        let dir = root.join(directory);
        if !dir.exists() {
            continue;
        }
        for entry in WalkDir::new(dir).into_iter().filter_map(|entry| entry.ok()) {
            if !entry.file_type().is_file() {
                continue;
            }
            if entry.path().extension().and_then(|ext| ext.to_str()) != Some("md") {
                continue;
            }
            let relative = relative_slash(root, entry.path());
            if is_current_behavior_path(asset_retirement, &relative) {
                paths.insert(relative);
            }
        }
    }
    let agents_dir = root.join("agents");
    if agents_dir.exists() {
        for entry in WalkDir::new(agents_dir)
            .into_iter()
            .filter_map(|entry| entry.ok())
        {
            if !entry.file_type().is_file() {
                continue;
            }
            if matches!(
                entry.path().extension().and_then(|ext| ext.to_str()),
                Some("yaml" | "yml")
            ) {
                let relative = relative_slash(root, entry.path());
                if is_current_behavior_path(asset_retirement, &relative) {
                    paths.insert(relative);
                }
            }
        }
    }
    Ok(paths.into_iter().collect())
}

fn current_behavior_asset_report(
    root: &Path,
    findings: &mut Vec<FeatureCoverageFinding>,
) -> Option<AssetRetirementReport> {
    match AssetRetirementManager::new(root).and_then(|manager| manager.status()) {
        Ok(report) => Some(report),
        Err(error) => {
            findings.push(FeatureCoverageFinding {
                feature_id: None,
                path: Some(
                    root.join(".RaymanCodingSkill")
                        .join("assets")
                        .join("retirement.json"),
                ),
                line: 1,
                kind: "asset_retirement_state_unreadable".into(),
                message: format!(
                    "unable to load asset retirement state for current-behavior filtering: {error}"
                ),
            });
            None
        }
    }
}

fn is_current_behavior_path(
    asset_retirement: Option<&AssetRetirementReport>,
    relative_path: &str,
) -> bool {
    asset_retirement.is_some_and(|report| report.is_current_behavior_path(relative_path))
}

fn registered_public_commands(manifest: &FeatureCoverageManifest) -> BTreeSet<String> {
    manifest
        .features
        .iter()
        .flat_map(|feature| feature.public_commands.iter())
        .map(|command| command.trim().to_string())
        .filter(|command| !command.is_empty())
        .collect()
}

fn registered_api_endpoints(manifest: &FeatureCoverageManifest) -> BTreeSet<String> {
    manifest
        .features
        .iter()
        .flat_map(|feature| feature.api_endpoints.iter())
        .map(|endpoint| endpoint.trim().to_string())
        .filter(|endpoint| !endpoint.is_empty())
        .collect()
}

fn feature_proofs(feature: &FeatureCoverageItem) -> BTreeSet<String> {
    let mut proofs = BTreeSet::new();
    for anchor in &feature.test_anchors {
        proofs.extend(
            anchor
                .proves
                .iter()
                .map(|proof| proof.trim().to_string())
                .filter(|proof| !proof.is_empty()),
        );
    }
    for claim in &feature.claim_checks {
        for anchor in &claim.test_anchors {
            proofs.extend(
                anchor
                    .proves
                    .iter()
                    .map(|proof| proof.trim().to_string())
                    .filter(|proof| !proof.is_empty()),
            );
        }
    }
    proofs
}

fn command_registered_by_prefix(command: &str, registered: &BTreeSet<String>) -> bool {
    registered
        .iter()
        .any(|entry| command == entry || command.starts_with(&format!("{entry} ")))
}

fn command_documented_by_registered_surface(command: &str, registered: &BTreeSet<String>) -> bool {
    command_registered_by_prefix(command, registered)
}

fn command_surface_registered(command: &str, registered: &BTreeSet<String>) -> bool {
    registered
        .iter()
        .any(|entry| command == entry || entry.starts_with(&format!("{command} ")))
}

#[derive(Debug, Clone)]
struct CliVariant {
    command_name: String,
    aliases: Vec<String>,
    payload_type: Option<String>,
    subcommand: bool,
    hidden: bool,
}

fn extract_cli_command_paths(text: &str) -> BTreeSet<String> {
    let mut commands = BTreeSet::new();
    for variant in enum_variants(text, "Command") {
        if variant.hidden {
            continue;
        }
        for command_name in variant_command_names(&variant) {
            let base = format!("rayman {command_name}");
            commands.insert(base.clone());
            if variant.subcommand
                && let Some(payload) = variant.payload_type.as_deref()
            {
                collect_nested_command_paths(text, payload, &base, &mut commands);
            } else if let Some(payload) = variant.payload_type.as_deref()
                && let Some(subcommand_enum) = args_subcommand_enum(text, payload)
            {
                collect_nested_command_paths(text, &subcommand_enum, &base, &mut commands);
            }
        }
    }
    commands
}

fn collect_nested_command_paths(
    text: &str,
    enum_name: &str,
    prefix: &str,
    commands: &mut BTreeSet<String>,
) {
    for variant in enum_variants(text, enum_name) {
        if variant.hidden {
            continue;
        }
        for command_name in variant_command_names(&variant) {
            let path = format!("{prefix} {command_name}");
            commands.insert(path.clone());
            if variant.subcommand
                && let Some(payload) = variant.payload_type.as_deref()
            {
                collect_nested_command_paths(text, payload, &path, commands);
            } else if let Some(payload) = variant.payload_type.as_deref()
                && let Some(subcommand_enum) = args_subcommand_enum(text, payload)
            {
                collect_nested_command_paths(text, &subcommand_enum, &path, commands);
            }
        }
    }
}

fn args_subcommand_enum(text: &str, struct_name: &str) -> Option<String> {
    let body = type_body(text, "struct", struct_name)?;
    let mut saw_subcommand_attr = false;
    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("#[") {
            if trimmed.contains("command(subcommand)") {
                saw_subcommand_attr = true;
            }
            continue;
        }
        if saw_subcommand_attr && let Some((_, ty)) = trimmed.split_once(':') {
            return Some(
                ty.trim()
                    .trim_end_matches(',')
                    .split_whitespace()
                    .next()
                    .unwrap_or_default()
                    .trim()
                    .to_string(),
            )
            .filter(|value| !value.is_empty());
        }
    }
    None
}

fn enum_variants(text: &str, enum_name: &str) -> Vec<CliVariant> {
    let Some(body) = type_body(text, "enum", enum_name) else {
        return Vec::new();
    };
    let mut variants = Vec::new();
    let mut attrs = Vec::new();
    let mut item_brace_depth = 0isize;
    for line in body.lines() {
        let trimmed = line.trim();
        if item_brace_depth > 0 {
            item_brace_depth += brace_delta(trimmed);
            continue;
        }
        if trimmed.is_empty() || trimmed.starts_with("//") {
            continue;
        }
        if trimmed.starts_with("#[") {
            attrs.push(trimmed.to_string());
            continue;
        }
        if let Some(name) = leading_ident(trimmed) {
            let command_name = command_name_for_variant(&name, &attrs);
            let aliases = command_aliases_for_variant(&attrs);
            let payload_type = tuple_payload_type(trimmed);
            let subcommand = attrs
                .iter()
                .any(|attr| attr.contains("command(subcommand)"));
            let hidden = attrs.iter().any(|attr| attr.contains("hide = true"));
            variants.push(CliVariant {
                command_name,
                aliases,
                payload_type,
                subcommand,
                hidden,
            });
            attrs.clear();
            item_brace_depth += brace_delta(trimmed).max(0);
        } else {
            attrs.clear();
        }
    }
    variants
}

fn variant_command_names(variant: &CliVariant) -> Vec<String> {
    let mut names = vec![variant.command_name.clone()];
    names.extend(variant.aliases.iter().cloned());
    names
}

fn type_body(text: &str, kind: &str, name: &str) -> Option<String> {
    let needle = format!("{kind} {name}");
    let start = text.find(&needle)?;
    let after = &text[start..];
    let open = after.find('{')?;
    let mut depth = 0isize;
    let mut body_start = None;
    for (offset, ch) in after[open..].char_indices() {
        match ch {
            '{' => {
                depth += 1;
                if depth == 1 {
                    body_start = Some(open + offset + ch.len_utf8());
                }
            }
            '}' => {
                depth -= 1;
                if depth == 0 {
                    let start = body_start?;
                    return Some(after[start..open + offset].to_string());
                }
            }
            _ => {}
        }
    }
    None
}

fn leading_ident(line: &str) -> Option<String> {
    let ident = line
        .chars()
        .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '_')
        .collect::<String>();
    (!ident.is_empty()).then_some(ident)
}

fn tuple_payload_type(line: &str) -> Option<String> {
    let start = line.find('(')?;
    let rest = &line[start + 1..];
    let end = rest.find(')')?;
    let ty = rest[..end].trim();
    if let Some(inner) = ty
        .strip_prefix("Box<")
        .and_then(|value| value.strip_suffix('>'))
    {
        return Some(inner.trim().to_string());
    }
    Some(ty.to_string()).filter(|value| !value.is_empty())
}

fn command_name_for_variant(name: &str, attrs: &[String]) -> String {
    for attr in attrs {
        if let Some(name) = command_name_attr(attr) {
            return name;
        }
    }
    kebab_case(name.strip_suffix("Command").unwrap_or(name))
}

fn command_name_attr(attr: &str) -> Option<String> {
    let marker = "name = \"";
    let start = attr.find(marker)?;
    let rest = &attr[start + marker.len()..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

fn command_aliases_for_variant(attrs: &[String]) -> Vec<String> {
    attrs
        .iter()
        .flat_map(|attr| command_alias_attrs(attr))
        .collect()
}

fn command_alias_attrs(attr: &str) -> Vec<String> {
    let marker = "alias = \"";
    let mut aliases = Vec::new();
    let mut rest = attr;
    while let Some(start) = rest.find(marker) {
        let after = &rest[start + marker.len()..];
        let Some(end) = after.find('"') else {
            break;
        };
        aliases.push(after[..end].to_string());
        rest = &after[end + 1..];
    }
    aliases
}

fn kebab_case(value: &str) -> String {
    let mut out = String::new();
    for (index, ch) in value.chars().enumerate() {
        if ch.is_ascii_uppercase() {
            if index > 0 {
                out.push('-');
            }
            out.push(ch.to_ascii_lowercase());
        } else if ch == '_' {
            out.push('-');
        } else {
            out.push(ch);
        }
    }
    out
}

fn brace_delta(line: &str) -> isize {
    line.chars().filter(|ch| *ch == '{').count() as isize
        - line.chars().filter(|ch| *ch == '}').count() as isize
}

fn paren_delta(line: &str) -> isize {
    line.chars().filter(|ch| *ch == '(').count() as isize
        - line.chars().filter(|ch| *ch == ')').count() as isize
}

fn extract_rayman_command(line: &str) -> Option<String> {
    let trimmed = line.trim();
    if let Some(rest) = trimmed.strip_prefix("`rayman ") {
        let command = rest.split('`').next()?.trim();
        return Some(format!("rayman {command}"));
    }
    if let Some(rest) = trimmed.strip_prefix("rayman ") {
        return Some(format!("rayman {}", rest.trim()));
    }
    None
}

fn is_endpoint_spec(value: &str) -> bool {
    let trimmed = value.trim();
    ["GET /", "POST /", "PUT /", "DELETE /", "PATCH /"]
        .iter()
        .any(|prefix| trimmed.starts_with(prefix))
}

fn route_methods(text: &str) -> Vec<&'static str> {
    [
        ("get(", "GET"),
        ("post(", "POST"),
        ("put(", "PUT"),
        ("delete(", "DELETE"),
        ("patch(", "PATCH"),
    ]
    .into_iter()
    .filter_map(|(needle, method)| text.contains(needle).then_some(method))
    .collect()
}

fn quoted_string(line: &str) -> Option<String> {
    let start = line.find('"')?;
    let rest = &line[start + 1..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

fn resolve_workspace_relative(root: &Path, relative: &str) -> Result<PathBuf> {
    let path = Path::new(relative);
    if path.is_absolute() {
        bail!("coverage anchor path must be workspace-relative: {relative}");
    }
    for component in path.components() {
        if matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        ) {
            bail!("coverage anchor path must stay inside workspace: {relative}");
        }
    }
    Ok(root.join(path))
}

fn first_line_containing(text: &str, needle: &str) -> Option<usize> {
    text.lines()
        .enumerate()
        .find_map(|(index, line)| line.contains(needle).then_some(index + 1))
}

fn push_feature_finding(
    root: &Path,
    findings: &mut Vec<FeatureCoverageFinding>,
    feature: &FeatureCoverageItem,
    kind: &str,
    message: &str,
) {
    findings.push(FeatureCoverageFinding {
        feature_id: Some(feature.id.clone()),
        path: Some(root.join(FEATURE_COVERAGE_MANIFEST)),
        line: 1,
        kind: kind.into(),
        message: message.into(),
    });
}

fn canonical_or_current(root: &Path) -> Result<PathBuf> {
    root.canonicalize()
        .with_context(|| format!("cannot resolve workspace root: {}", root.display()))
}

fn anchor_summary(anchors: &[CoverageAnchor]) -> String {
    let mut by_path: BTreeMap<&str, usize> = BTreeMap::new();
    for anchor in anchors {
        *by_path.entry(anchor.path.as_str()).or_default() += 1;
    }
    by_path
        .into_iter()
        .map(|(path, count)| {
            if count == 1 {
                format!("`{}`", escape_markdown_table(path))
            } else {
                format!("`{}` ({count})", escape_markdown_table(path))
            }
        })
        .collect::<Vec<_>>()
        .join("<br>")
}

fn list_summary(values: &[String]) -> String {
    if values.is_empty() {
        return String::new();
    }
    values
        .iter()
        .map(|value| format!("`{}`", escape_markdown_table(value)))
        .collect::<Vec<_>>()
        .join("<br>")
}

fn claim_summary(claims: &[ClaimCheck]) -> String {
    if claims.is_empty() {
        return String::new();
    }
    claims
        .iter()
        .map(|claim| {
            if claim.strict_validation {
                format!("`{}` (strict)", escape_markdown_table(&claim.id))
            } else {
                format!("`{}`", escape_markdown_table(&claim.id))
            }
        })
        .collect::<Vec<_>>()
        .join("<br>")
}

fn escape_markdown_table(value: &str) -> String {
    value.replace('|', "\\|")
}

fn normalize_slash(path: &str) -> String {
    path.replace('\\', "/")
}

fn relative_slash(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn coverage_gate_rejects_unmapped_cli_command() {
        let temp = tempfile::tempdir().unwrap();
        write_minimal_repo(temp.path());
        fs::write(
            temp.path().join("docs").join("CLI.md"),
            "# CLI\n\n```text\nrayman missing command\n```\n",
        )
        .unwrap();

        let report = check_feature_coverage(temp.path()).unwrap();

        assert!(report.findings.iter().any(|finding| {
            finding.kind == "public_command_unmapped"
                && finding.message.contains("rayman missing command")
        }));
    }

    #[test]
    fn coverage_gate_rejects_source_public_command_missing_from_manifest() {
        let temp = tempfile::tempdir().unwrap();
        write_minimal_repo(temp.path());
        fs::write(
            temp.path()
                .join("crates")
                .join("rayman-cli")
                .join("src")
                .join("main.rs"),
            r#"
#[derive(Subcommand)]
enum Command {
    Session(SessionCommand),
    InstallTools(InstallToolsArgs),
}
#[derive(Subcommand)]
enum SessionCommand {
    Status,
}
"#,
        )
        .unwrap();

        let report = check_feature_coverage(temp.path()).unwrap();

        assert!(report.findings.iter().any(|finding| {
            finding.kind == "public_command_source_unmapped"
                && finding.message.contains("rayman install-tools")
        }));
    }

    #[test]
    fn implemented_public_commands_reads_split_cli_module() {
        let temp = tempfile::tempdir().unwrap();
        write_minimal_repo(temp.path());
        fs::write(
            temp.path()
                .join("crates")
                .join("rayman-cli")
                .join("src")
                .join("cli.rs"),
            r#"
#[derive(Subcommand)]
enum Command {
    InstallTools(InstallToolsArgs),
    #[command(subcommand)]
    Session(SessionCommand),
}
#[derive(Subcommand)]
enum SessionCommand {
    Status,
}
"#,
        )
        .unwrap();

        let commands = implemented_public_commands(temp.path()).unwrap();

        assert!(commands.contains(&"rayman install-tools".to_string()));
        assert!(commands.contains(&"rayman session status".to_string()));
    }

    #[test]
    fn coverage_gate_rejects_nested_source_command_registered_only_by_parent() {
        let temp = tempfile::tempdir().unwrap();
        write_minimal_repo(temp.path());
        fs::write(
            temp.path()
                .join("crates")
                .join("rayman-cli")
                .join("src")
                .join("main.rs"),
            r#"
#[derive(Subcommand)]
enum Command {
    #[command(subcommand)]
    Session(SessionCommand),
}
#[derive(Subcommand)]
enum SessionCommand {
    Status,
    Delete,
}
"#,
        )
        .unwrap();
        fs::write(
            temp.path().join(FEATURE_COVERAGE_MANIFEST),
            r##"
features:
  - id: cli
    title: CLI
    doc_anchors:
      - path: docs/CLI.md
        contains: "# CLI"
    implementation_anchors:
      - path: crates/rayman-cli/src/main.rs
        contains: enum Command
    test_anchors:
      - path: crates/rayman-cli/tests/ui_contract.rs
        contains: cli_help
        proves:
          - rayman session
          - rayman session status
    validation_commands:
      - cargo test -p rayman-cli
    ui_surfaces:
      - cli
    public_commands:
      - rayman session
      - rayman session status
"##,
        )
        .unwrap();

        let report = check_feature_coverage(temp.path()).unwrap();

        assert!(report.findings.iter().any(|finding| {
            finding.kind == "public_command_source_unmapped"
                && finding.message.contains("rayman session delete")
        }));
    }

    #[test]
    fn api_route_extractor_collects_supported_verbs() {
        assert_eq!(route_methods("get(handler)"), vec!["GET"]);
        assert_eq!(route_methods("post(handler)"), vec!["POST"]);
        assert_eq!(route_methods("put(handler)"), vec!["PUT"]);
        assert_eq!(route_methods("delete(handler)"), vec!["DELETE"]);
        assert_eq!(route_methods("patch(handler)"), vec!["PATCH"]);
        assert_eq!(route_methods("get(list).post(create)"), vec!["GET", "POST"]);
    }

    #[test]
    fn coverage_gate_rejects_unmapped_api_route() {
        let temp = tempfile::tempdir().unwrap();
        write_minimal_repo(temp.path());
        fs::write(
            temp.path()
                .join("crates")
                .join("rayman-api")
                .join("src")
                .join("lib.rs"),
            r#"
fn app() {
    Router::new()
        .route("/api/missing", put(update_handler));
}
"#,
        )
        .unwrap();

        let report = check_feature_coverage(temp.path()).unwrap();

        assert!(report.findings.iter().any(|finding| {
            finding.kind == "api_route_unmapped" && finding.message.contains("PUT /api/missing")
        }));
    }

    #[test]
    fn api_route_extractor_collects_chained_and_multiline_methods() {
        let temp = tempfile::tempdir().unwrap();
        write_minimal_repo(temp.path());
        fs::write(
            temp.path()
                .join("crates")
                .join("rayman-api")
                .join("src")
                .join("lib.rs"),
            r#"
pub fn app() {
    Router::new()
        .route("/api/items", get(list_items).post(create_item))
        .route(
            "/api/items/{id}",
            get(get_item)
                .put(update_item)
                .delete(delete_item),
        );
}
"#,
        )
        .unwrap();

        let endpoints = implemented_api_endpoints(temp.path()).unwrap();

        for endpoint in [
            "GET /api/items",
            "POST /api/items",
            "GET /api/items/{id}",
            "PUT /api/items/{id}",
            "DELETE /api/items/{id}",
        ] {
            assert!(
                endpoints.contains(&endpoint.to_string()),
                "missing {endpoint}"
            );
        }
    }

    #[test]
    fn coverage_gate_requires_test_anchor_proves_public_command() {
        let temp = tempfile::tempdir().unwrap();
        write_minimal_repo(temp.path());
        fs::write(
            temp.path().join(FEATURE_COVERAGE_MANIFEST),
            r##"
features:
  - id: cli
    title: CLI
    doc_anchors:
      - path: docs/CLI.md
        contains: "# CLI"
    implementation_anchors:
      - path: crates/rayman-cli/src/main.rs
        contains: enum Command
    test_anchors:
      - path: crates/rayman-cli/tests/ui_contract.rs
        contains: "@ui:cli"
    validation_commands:
      - cargo test -p rayman-cli
    ui_surfaces:
      - cli
    public_commands:
      - rayman session status
"##,
        )
        .unwrap();

        let report = check_feature_coverage(temp.path()).unwrap();

        assert!(report.findings.iter().any(|finding| {
            finding.kind == "test_anchor_unproven_public_command"
                && finding.message.contains("rayman session status")
        }));
    }

    #[test]
    fn coverage_gate_requires_test_anchor_proves_api_endpoint() {
        let temp = tempfile::tempdir().unwrap();
        write_minimal_repo(temp.path());
        fs::write(
            temp.path().join("docs").join("API.md"),
            "# API\n\n- `GET /api/items`\n",
        )
        .unwrap();
        fs::write(
            temp.path().join(FEATURE_COVERAGE_MANIFEST),
            r##"
features:
  - id: api
    title: API
    doc_anchors:
      - path: docs/API.md
        contains: "# API"
    implementation_anchors:
      - path: crates/rayman-api/src/lib.rs
        contains: pub fn app
    test_anchors:
      - path: crates/rayman-cli/tests/ui_contract.rs
        contains: "@ui:cli"
    validation_commands:
      - cargo test -p rayman-api
    api_endpoints:
      - GET /api/items
"##,
        )
        .unwrap();

        let report = check_feature_coverage(temp.path()).unwrap();

        assert!(report.findings.iter().any(|finding| {
            finding.kind == "test_anchor_unproven_api_endpoint"
                && finding.message.contains("GET /api/items")
        }));
    }

    #[test]
    fn coverage_gate_validates_claim_doc_anchors() {
        let temp = tempfile::tempdir().unwrap();
        write_minimal_repo(temp.path());
        fs::write(
            temp.path().join(FEATURE_COVERAGE_MANIFEST),
            r##"
features:
  - id: cli
    title: CLI
    doc_anchors:
      - path: docs/CLI.md
        contains: "# CLI"
    implementation_anchors:
      - path: crates/rayman-cli/src/main.rs
        contains: enum Command
    test_anchors:
      - path: crates/rayman-cli/tests/ui_contract.rs
        contains: "@ui:cli"
        proves:
          - rayman session status
          - documented_claim
    validation_commands:
      - cargo test -p rayman-cli
    ui_surfaces:
      - cli
    public_commands:
      - rayman session status
    claim_checks:
      - id: documented_claim
        claim: A nested claim has a current documentation anchor.
        doc_anchors:
          - path: docs/CLI.md
            contains: "missing nested claim text"
        implementation_anchors:
          - path: crates/rayman-cli/src/main.rs
            contains: enum Command
        test_anchors:
          - path: crates/rayman-cli/tests/ui_contract.rs
            contains: "@ui:cli"
            proves:
              - documented_claim
        validation_commands:
          - cargo test -p rayman-cli
"##,
        )
        .unwrap();

        let report = check_feature_coverage(temp.path()).unwrap();

        assert!(report.findings.iter().any(|finding| {
            finding.kind == "claim_doc_anchor_missing_text"
                && finding.message.contains("missing nested claim text")
        }));
    }

    #[test]
    fn coverage_gate_requires_claim_check_proof_triplet() {
        let temp = tempfile::tempdir().unwrap();
        write_minimal_repo(temp.path());
        fs::write(
            temp.path().join(FEATURE_COVERAGE_MANIFEST),
            r##"
features:
  - id: cli
    title: CLI
    doc_anchors:
      - path: docs/CLI.md
        contains: "# CLI"
    implementation_anchors:
      - path: crates/rayman-cli/src/main.rs
        contains: enum Command
    test_anchors:
      - path: crates/rayman-cli/tests/ui_contract.rs
        contains: "@ui:cli"
        proves:
          - rayman session status
    validation_commands:
      - cargo test -p rayman-cli
    ui_surfaces:
      - cli
    public_commands:
      - rayman session status
    claim_checks:
      - id: paper_claim_gate
        claim: Paper claims need executable proof.
"##,
        )
        .unwrap();

        let report = check_feature_coverage(temp.path()).unwrap();

        assert!(
            report
                .findings
                .iter()
                .any(|finding| { finding.kind == "claim_implementation_anchor_missing" })
        );
        assert!(
            report
                .findings
                .iter()
                .any(|finding| finding.kind == "claim_test_anchor_missing")
        );
        assert!(
            report
                .findings
                .iter()
                .any(|finding| { finding.kind == "claim_validation_commands_missing" })
        );
    }

    #[test]
    fn markdown_matrix_lists_claim_check_ids() {
        let report = FeatureCoverageReport {
            workspace_path: PathBuf::from("."),
            generated_at: "2026-07-01T00:00:00Z".into(),
            manifest_path: PathBuf::from(FEATURE_COVERAGE_MANIFEST),
            status: "passed".into(),
            strict: true,
            feature_count: 1,
            finding_count: 0,
            findings: Vec::new(),
            covered_document_paths: Vec::new(),
            expected_document_paths: Vec::new(),
            documented_public_commands: Vec::new(),
            implemented_public_commands: Vec::new(),
            registered_public_commands: Vec::new(),
            documented_api_endpoints: Vec::new(),
            implemented_api_endpoints: Vec::new(),
            registered_api_endpoints: Vec::new(),
            required_actions: Vec::new(),
            features: vec![FeatureCoverageItem {
                id: "quality".into(),
                title: "Quality".into(),
                strict_validation: false,
                doc_anchors: Vec::new(),
                implementation_anchors: Vec::new(),
                test_anchors: Vec::new(),
                validation_commands: Vec::new(),
                validation_records: Vec::new(),
                ui_surfaces: Vec::new(),
                public_commands: Vec::new(),
                api_endpoints: Vec::new(),
                claim_checks: vec![ClaimCheck {
                    id: "execution_context_quality_patterns".into(),
                    claim: "Execution context patterns are visible in generated docs.".into(),
                    strict_validation: true,
                    doc_anchors: Vec::new(),
                    implementation_anchors: Vec::new(),
                    test_anchors: Vec::new(),
                    validation_commands: Vec::new(),
                    validation_records: Vec::new(),
                }],
            }],
        };

        let markdown = render_feature_coverage_markdown(&report);

        assert!(
            markdown.contains("| ID | Docs | Implementation | Tests | Validation | Claims | UI |")
        );
        assert!(markdown.contains("`execution_context_quality_patterns` (strict)"));
    }

    #[test]
    fn strict_claim_requires_validation_records() {
        let temp = tempfile::tempdir().unwrap();
        write_strict_claim_repo(temp.path(), "");

        let report = check_feature_coverage_with_options(
            temp.path(),
            FeatureCoverageOptions { strict: true },
        )
        .unwrap();

        assert!(report.findings.iter().any(|finding| {
            finding.kind == "claim_validation_records_missing"
                && finding.message.contains("strict_claim")
        }));
    }

    #[test]
    fn non_strict_mode_accepts_legacy_claim_without_validation_records() {
        let temp = tempfile::tempdir().unwrap();
        write_strict_claim_repo(temp.path(), "");

        let report = check_feature_coverage(temp.path()).unwrap();

        assert_eq!(report.status, "passed");
        assert!(
            !report
                .findings
                .iter()
                .any(|finding| finding.kind == "claim_validation_records_missing")
        );
    }

    #[test]
    fn strict_claim_rejects_stale_validation_record() {
        let temp = tempfile::tempdir().unwrap();
        write_strict_claim_repo(
            temp.path(),
            r#"
        validation_records:
          - command: cargo test -p rayman-core feature_coverage::strict_claim_accepts_current_passed_validation_record
            status: passed
            evidence_path: .RaymanCodingSkill/feature_coverage/validation.txt
            evidence_contains:
              - strict validation stale fixture
            updated_at: "2000-01-01T00:00:00Z"
"#,
        );
        fs::create_dir_all(
            temp.path()
                .join(".RaymanCodingSkill")
                .join("feature_coverage"),
        )
        .unwrap();
        fs::write(
            temp.path()
                .join(".RaymanCodingSkill")
                .join("feature_coverage")
                .join("validation.txt"),
            "command: cargo test -p rayman-core feature_coverage::strict_claim_accepts_current_passed_validation_record\nstatus: passed\nstrict validation stale fixture\n",
        )
        .unwrap();

        let report = check_feature_coverage_with_options(
            temp.path(),
            FeatureCoverageOptions { strict: true },
        )
        .unwrap();

        assert!(report.findings.iter().any(|finding| {
            finding.kind == "claim_validation_record_stale"
                && finding.message.contains("strict_claim")
        }));
    }

    #[test]
    fn strict_claim_accepts_current_passed_validation_record() {
        let temp = tempfile::tempdir().unwrap();
        let command = "cargo test -p rayman-core feature_coverage::strict_claim_accepts_current_passed_validation_record";
        write_strict_claim_repo(
            temp.path(),
            &format!(
                r#"
        validation_records:
          - command: {command}
            status: passed
            evidence_path: .RaymanCodingSkill/feature_coverage/validation.txt
            evidence_contains:
              - strict validation current fixture
            updated_at: "2999-01-01T00:00:00Z"
"#
            ),
        );
        fs::create_dir_all(
            temp.path()
                .join(".RaymanCodingSkill")
                .join("feature_coverage"),
        )
        .unwrap();
        fs::write(
            temp.path()
                .join(".RaymanCodingSkill")
                .join("feature_coverage")
                .join("validation.txt"),
            format!("command: {command}\nstatus: passed\nstrict validation current fixture\n"),
        )
        .unwrap();

        let report = check_feature_coverage_with_options(
            temp.path(),
            FeatureCoverageOptions { strict: true },
        )
        .unwrap();

        assert_eq!(report.status, "passed");
    }

    #[test]
    fn cli_command_extractor_collects_nested_commands_and_ignores_hidden() {
        let commands = extract_cli_command_paths(
            r#"
#[derive(Subcommand)]
enum Command {
    Generate(GenerateArgs),
    #[command(alias = "skill")]
    #[command(subcommand)]
    AgentSkill(AgentSkillCommand),
    #[command(subcommand)]
    Quality(QualityCommand),
    #[command(subcommand)]
    Docs(DocsCommand),
    #[command(name = "self")]
    #[command(subcommand)]
    SelfCommand(SelfCommand),
}
#[derive(Subcommand)]
enum AgentSkillCommand {
    #[command(alias = "install", alias = "update")]
    Sync,
    Status,
}
#[derive(Subcommand)]
enum QualityCommand {
    #[command(subcommand)]
    Incident(QualityIncidentCommand),
    Patterns,
}
#[derive(Subcommand)]
enum QualityIncidentCommand {
    Add(QualityIncidentAddArgs),
}
#[derive(Subcommand)]
enum DocsCommand {
    Maintain(DocsMaintainArgs),
    #[command(hide = true)]
    Compress { file: PathBuf },
}
#[derive(Subcommand)]
enum SelfCommand {
    Status,
}
"#,
        );

        assert!(commands.contains("rayman generate"));
        assert!(commands.contains("rayman agent-skill sync"));
        assert!(commands.contains("rayman agent-skill install"));
        assert!(commands.contains("rayman agent-skill update"));
        assert!(commands.contains("rayman skill sync"));
        assert!(commands.contains("rayman skill install"));
        assert!(commands.contains("rayman skill update"));
        assert!(commands.contains("rayman skill status"));
        assert!(commands.contains("rayman quality"));
        assert!(commands.contains("rayman quality incident add"));
        assert!(commands.contains("rayman docs maintain"));
        assert!(commands.contains("rayman self status"));
        assert!(!commands.contains("rayman docs compress"));
    }

    #[test]
    fn coverage_gate_requires_ui_marker_for_surface() {
        let temp = tempfile::tempdir().unwrap();
        write_minimal_repo(temp.path());
        fs::write(
            temp.path()
                .join("crates")
                .join("rayman-cli")
                .join("tests")
                .join("ui_contract.rs"),
            "fn cli_help() {}\n",
        )
        .unwrap();
        fs::write(
            temp.path().join(FEATURE_COVERAGE_MANIFEST),
            r##"
features:
  - id: cli
    title: CLI
    doc_anchors:
      - path: docs/CLI.md
        contains: "# CLI"
    implementation_anchors:
      - path: crates/rayman-cli/src/main.rs
        contains: enum Command
    test_anchors:
      - path: crates/rayman-cli/tests/ui_contract.rs
        contains: cli_help
        proves:
          - rayman session status
    validation_commands:
      - cargo test -p rayman-cli
    ui_surfaces:
      - cli
    public_commands:
      - rayman session status
"##,
        )
        .unwrap();

        let report = check_feature_coverage(temp.path()).unwrap();

        assert!(
            report
                .findings
                .iter()
                .any(|finding| finding.kind == "ui_contract_missing")
        );
    }

    #[test]
    fn coverage_gate_requires_agent_yaml_surface_mapping() {
        let temp = tempfile::tempdir().unwrap();
        write_minimal_repo(temp.path());
        fs::create_dir_all(temp.path().join("agents")).unwrap();
        fs::write(
            temp.path().join("agents").join("openai.yaml"),
            "interface:\n  display_name: Test Agent\n",
        )
        .unwrap();

        let report = check_feature_coverage(temp.path()).unwrap();

        assert!(report.findings.iter().any(|finding| {
            finding.kind == "document_unmapped" && finding.message.contains("agents/openai.yaml")
        }));
    }

    #[test]
    fn coverage_rejects_non_current_doc_anchor_as_current_proof() {
        let temp = tempfile::tempdir().unwrap();
        write_minimal_repo(temp.path());
        let old_doc = temp.path().join("docs").join("old-cli.md");
        fs::write(&old_doc, "# CLI\nstale coverage proof\n").unwrap();
        AssetRetirementManager::new(temp.path())
            .unwrap()
            .exempt(crate::assets::AssetExemptRequest {
                path: old_doc,
                retention_reason: "temporary audit retention".into(),
                expires_at: "2999-01-01".into(),
            })
            .unwrap();
        fs::write(
            temp.path().join(FEATURE_COVERAGE_MANIFEST),
            r##"
features:
  - id: cli
    title: CLI
    doc_anchors:
      - path: docs/old-cli.md
        contains: stale coverage proof
    implementation_anchors:
      - path: crates/rayman-cli/src/main.rs
        contains: enum Command
    test_anchors:
      - path: crates/rayman-cli/tests/ui_contract.rs
        contains: "@ui:cli"
        proves:
          - rayman session status
    validation_commands:
      - cargo test -p rayman-cli
    ui_surfaces:
      - cli
    public_commands:
      - rayman session status
"##,
        )
        .unwrap();

        let report = check_feature_coverage(temp.path()).unwrap();

        assert!(report.findings.iter().any(|finding| {
            finding.kind == "doc_anchor_non_current_asset"
                && finding.message.contains("docs/old-cli.md")
        }));
        assert!(
            !report
                .covered_document_paths
                .contains(&"docs/old-cli.md".to_string())
        );
        assert!(
            !report
                .expected_document_paths
                .contains(&"docs/old-cli.md".to_string())
        );
    }

    #[test]
    fn command_extractors_ignore_non_current_sources_and_docs() {
        let temp = tempfile::tempdir().unwrap();
        write_minimal_repo(temp.path());
        let old_doc = temp.path().join("docs").join("CLI.md");
        fs::write(&old_doc, "# CLI\nrayman stale doc command\n").unwrap();
        let old_source = temp
            .path()
            .join("crates")
            .join("rayman-cli")
            .join("src")
            .join("old.rs");
        fs::write(&old_source, "enum Command { StaleSourceCommand }\n").unwrap();
        let manager = AssetRetirementManager::new(temp.path()).unwrap();
        manager
            .exempt(crate::assets::AssetExemptRequest {
                path: old_doc,
                retention_reason: "temporary audit retention".into(),
                expires_at: "2999-01-01".into(),
            })
            .unwrap();
        manager
            .exempt(crate::assets::AssetExemptRequest {
                path: old_source,
                retention_reason: "temporary audit retention".into(),
                expires_at: "2999-01-01".into(),
            })
            .unwrap();

        let documented = documented_public_commands(temp.path()).unwrap();
        let implemented = implemented_public_commands(temp.path()).unwrap();

        assert!(!documented.contains(&"rayman stale doc command".to_string()));
        assert!(!implemented.contains(&"rayman stale-source-command".to_string()));
    }

    fn write_minimal_repo(root: &Path) {
        fs::create_dir_all(root.join("config")).unwrap();
        fs::create_dir_all(root.join("docs")).unwrap();
        fs::create_dir_all(root.join("crates").join("rayman-cli").join("src")).unwrap();
        fs::create_dir_all(root.join("crates").join("rayman-cli").join("tests")).unwrap();
        fs::create_dir_all(root.join("crates").join("rayman-api").join("src")).unwrap();
        fs::write(
            root.join("docs").join("CLI.md"),
            "# CLI\nrayman session status\n",
        )
        .unwrap();
        fs::write(root.join("docs").join("API.md"), "# API\n").unwrap();
        fs::write(
            root.join("crates")
                .join("rayman-cli")
                .join("src")
                .join("main.rs"),
            "enum Command {}\n",
        )
        .unwrap();
        fs::write(
            root.join("crates")
                .join("rayman-cli")
                .join("tests")
                .join("ui_contract.rs"),
            "// @ui:cli\nfn cli_help() {}\n",
        )
        .unwrap();
        fs::write(
            root.join("crates")
                .join("rayman-api")
                .join("src")
                .join("lib.rs"),
            "pub fn app() {}\n",
        )
        .unwrap();
        fs::write(
            root.join(FEATURE_COVERAGE_MANIFEST),
            r##"
features:
  - id: cli
    title: CLI
    doc_anchors:
      - path: docs/CLI.md
        contains: "# CLI"
    implementation_anchors:
      - path: crates/rayman-cli/src/main.rs
        contains: enum Command
    test_anchors:
      - path: crates/rayman-cli/tests/ui_contract.rs
        contains: "@ui:cli"
        proves:
          - rayman session status
    validation_commands:
      - cargo test -p rayman-cli
    ui_surfaces:
      - cli
    public_commands:
      - rayman session status
  - id: api
    title: API
    doc_anchors:
      - path: docs/API.md
        contains: "# API"
    implementation_anchors:
      - path: crates/rayman-api/src/lib.rs
        contains: pub fn app
    test_anchors:
      - path: crates/rayman-cli/tests/ui_contract.rs
        contains: "@ui:cli"
    validation_commands:
      - cargo test -p rayman-api
"##,
        )
        .unwrap();
    }

    fn write_strict_claim_repo(root: &Path, validation_record_yaml: &str) {
        write_minimal_repo(root);
        fs::write(
            root.join(FEATURE_COVERAGE_MANIFEST),
            format!(
                r##"
features:
  - id: cli
    title: CLI
    doc_anchors:
      - path: docs/CLI.md
        contains: "# CLI"
    implementation_anchors:
      - path: crates/rayman-cli/src/main.rs
        contains: enum Command
    test_anchors:
      - path: crates/rayman-cli/tests/ui_contract.rs
        contains: fn cli_help
        proves:
          - rayman session status
          - strict_claim
    validation_commands:
      - cargo test -p rayman-cli
    ui_surfaces:
      - cli
    public_commands:
      - rayman session status
    claim_checks:
      - id: strict_claim
        claim: Strict claims require current passed validation records.
        strict_validation: true
        doc_anchors:
          - path: docs/CLI.md
            contains: "# CLI"
        implementation_anchors:
          - path: crates/rayman-cli/src/main.rs
            contains: enum Command
        test_anchors:
          - path: crates/rayman-cli/tests/ui_contract.rs
            contains: fn cli_help
            proves:
              - strict_claim
        validation_commands:
          - cargo test -p rayman-core feature_coverage::strict_claim_accepts_current_passed_validation_record
{validation_record_yaml}
  - id: api
    title: API
    doc_anchors:
      - path: docs/API.md
        contains: "# API"
    implementation_anchors:
      - path: crates/rayman-api/src/lib.rs
        contains: pub fn app
    test_anchors:
      - path: crates/rayman-cli/tests/ui_contract.rs
        contains: "@ui:cli"
    validation_commands:
      - cargo test -p rayman-api
"##
            ),
        )
        .unwrap();
    }
}
