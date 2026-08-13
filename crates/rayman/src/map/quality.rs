use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::context::ContextIndex;
use crate::file_io::read_json;
use crate::state_paths;

use super::{Dependency, MapRisk, MapSymbol, ProjectMap, TestTarget};
use crate::file_io::is_link_or_reparse;

const QUALITY_CONFIG_RELATIVE_PATH: &str = ".RaymanCodingSkill/quality.json";
const QUALITY_CONFIG_STATE_RELATIVE: &str = "quality.json";
const LARGE_SOURCE_WARNING_LINES: usize = 2_000;
const LARGE_TEST_WARNING_LINES: usize = 3_000;
const BLOCKABLE_WARNING_KINDS: &[&str] = &[
    "large_file",
    "high_fan_in",
    "public_api_without_test_evidence",
];

#[derive(Debug, Clone, Serialize)]
pub struct QualityReport {
    pub ready: bool,
    pub profile: String,
    pub strict_default_block_warning_kinds: Vec<String>,
    pub configured_block_warning_kinds: Vec<String>,
    pub configured_exemptions: Vec<QualityExemption>,
    pub source_files: usize,
    pub test_files: usize,
    pub candidate_test_covered_source_files: usize,
    pub public_api_files_without_test_evidence: usize,
    pub error_count: usize,
    pub warning_count: usize,
    pub info_count: usize,
    pub findings: Vec<QualityFinding>,
    pub findings_by_role: BTreeMap<String, QualityRoleSummary>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct QualityRoleSummary {
    pub findings: usize,
    pub error_count: usize,
    pub warning_count: usize,
    pub info_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QualityConfig {
    #[serde(default = "default_quality_profile")]
    pub profile: String,
    #[serde(default = "default_multi_source_no_test_min_sources")]
    pub multi_source_no_test_min_sources: usize,
    #[serde(default)]
    pub block_warning_kinds: Vec<String>,
    #[serde(default)]
    pub exemptions: Vec<QualityExemption>,
    #[serde(skip)]
    pub(super) strict_default_block_warning_kinds: Vec<String>,
    #[serde(skip)]
    pub(super) configured_block_warning_kinds: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct QualityExemption {
    pub path: String,
    pub kind: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct QualityFinding {
    pub severity: String,
    pub kind: String,
    pub path: String,
    pub role: String,
    pub detail: String,
    pub recommendation: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocking_policy_source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exemption_reason: Option<String>,
}

pub fn load_quality_config(root: &Path, profile: &str) -> Result<QualityConfig> {
    let mut config = match profile {
        "strict" => QualityConfig::strict(),
        _ => QualityConfig::standard(),
    };
    let path =
        state_paths::managed_state_file(root, Path::new(QUALITY_CONFIG_STATE_RELATIVE), false)?;
    if let Some(file_config) = read_json::<QualityConfig>(&path)? {
        validate_quality_config(&file_config)?;
        config.multi_source_no_test_min_sources = config
            .multi_source_no_test_min_sources
            .min(file_config.multi_source_no_test_min_sources.max(1));
        config.configured_block_warning_kinds = file_config.block_warning_kinds.clone();
        for kind in file_config.block_warning_kinds {
            if !config.block_warning_kinds.contains(&kind) {
                config.block_warning_kinds.push(kind);
            }
        }
        config.exemptions = file_config.exemptions;
        for exemption in &config.exemptions {
            validate_quality_exemption_target(root, exemption)?;
        }
        config.profile = profile.into();
    }
    Ok(config)
}

/// Load strict/standard policy from the exact workspace bytes held by a
/// readiness capture. Exemption targets are proven by membership in that same
/// complete capture, so policy evaluation never reopens a mutable path.
pub fn load_quality_config_from_capture(
    profile: &str,
    files: &BTreeMap<String, Vec<u8>>,
) -> Result<QualityConfig> {
    let mut config = match profile {
        "strict" => QualityConfig::strict(),
        _ => QualityConfig::standard(),
    };
    let file_config = files
        .get(QUALITY_CONFIG_RELATIVE_PATH)
        .map(|bytes| {
            serde_json::from_slice::<QualityConfig>(bytes)
                .context("无法解析 captured .RaymanCodingSkill/quality.json")
        })
        .transpose()?;
    if let Some(file_config) = file_config {
        validate_quality_config(&file_config)?;
        config.multi_source_no_test_min_sources = config
            .multi_source_no_test_min_sources
            .min(file_config.multi_source_no_test_min_sources.max(1));
        config.configured_block_warning_kinds = file_config.block_warning_kinds.clone();
        for kind in file_config.block_warning_kinds {
            if !config.block_warning_kinds.contains(&kind) {
                config.block_warning_kinds.push(kind);
            }
        }
        config.exemptions = file_config.exemptions;
        for exemption in &config.exemptions {
            let present = if cfg!(windows) {
                files
                    .keys()
                    .any(|path| path.eq_ignore_ascii_case(&exemption.path))
            } else {
                files.contains_key(&exemption.path)
            };
            if !present {
                bail!(
                    "{} exemption path must name a file in the same readiness capture: {}",
                    QUALITY_CONFIG_RELATIVE_PATH,
                    exemption.path
                );
            }
        }
        config.profile = profile.into();
    }
    Ok(config)
}

impl QualityConfig {
    pub fn standard() -> Self {
        Self {
            profile: "standard".into(),
            multi_source_no_test_min_sources: default_multi_source_no_test_min_sources(),
            block_warning_kinds: Vec::new(),
            exemptions: Vec::new(),
            strict_default_block_warning_kinds: Vec::new(),
            configured_block_warning_kinds: Vec::new(),
        }
    }

    pub fn strict() -> Self {
        Self {
            profile: "strict".into(),
            multi_source_no_test_min_sources: default_multi_source_no_test_min_sources(),
            // Objective maintainability signals are built in and cannot be
            // removed by a partial/empty workspace configuration.
            block_warning_kinds: vec!["large_file".into(), "high_fan_in".into()],
            exemptions: Vec::new(),
            strict_default_block_warning_kinds: vec!["large_file".into(), "high_fan_in".into()],
            configured_block_warning_kinds: Vec::new(),
        }
    }
}

fn default_quality_profile() -> String {
    "standard".into()
}

fn default_multi_source_no_test_min_sources() -> usize {
    3
}

fn validate_quality_config(config: &QualityConfig) -> Result<()> {
    let mut kinds = BTreeSet::new();
    for kind in &config.block_warning_kinds {
        if !BLOCKABLE_WARNING_KINDS.contains(&kind.as_str()) {
            bail!(
                "{} has unknown block_warning_kinds entry `{}`; allowed: {}",
                QUALITY_CONFIG_RELATIVE_PATH,
                kind,
                BLOCKABLE_WARNING_KINDS.join(", ")
            );
        }
        if !kinds.insert(kind.as_str()) {
            bail!(
                "{} contains duplicate block_warning_kinds entry `{}`",
                QUALITY_CONFIG_RELATIVE_PATH,
                kind
            );
        }
    }
    let mut exemptions = BTreeSet::new();
    for exemption in &config.exemptions {
        validate_quality_exemption(exemption)?;
        if !exemptions.insert((exemption.path.as_str(), exemption.kind.as_str())) {
            bail!(
                "{} contains duplicate exemption for path `{}` and kind `{}`",
                QUALITY_CONFIG_RELATIVE_PATH,
                exemption.path,
                exemption.kind
            );
        }
    }
    Ok(())
}

fn validate_quality_exemption(exemption: &QualityExemption) -> Result<()> {
    if !BLOCKABLE_WARNING_KINDS.contains(&exemption.kind.as_str()) {
        bail!(
            "{} exemption for `{}` has unknown kind `{}`; allowed: {}",
            QUALITY_CONFIG_RELATIVE_PATH,
            exemption.path,
            exemption.kind,
            BLOCKABLE_WARNING_KINDS.join(", ")
        );
    }
    let path = exemption.path.as_str();
    let has_glob = path
        .chars()
        .any(|ch| matches!(ch, '*' | '?' | '[' | ']' | '{' | '}'));
    let has_drive_prefix = path.as_bytes().get(1).copied() == Some(b':');
    let components_are_exact = !path.is_empty()
        && !path.starts_with('/')
        && !path.starts_with('\\')
        && !path.ends_with('/')
        && !path.contains('\\')
        && !path.contains("//")
        && !has_drive_prefix
        && !has_glob
        && Path::new(path)
            .components()
            .all(|component| matches!(component, Component::Normal(_)));
    if !components_are_exact {
        bail!(
            "{} exemption path `{}` must be one exact normalized workspace-relative file path (forward slashes, no glob, dot segment, or directory prefix)",
            QUALITY_CONFIG_RELATIVE_PATH,
            path
        );
    }
    if exemption.reason.trim().is_empty() {
        bail!(
            "{} exemption for `{}` and `{}` requires a non-empty reason",
            QUALITY_CONFIG_RELATIVE_PATH,
            exemption.path,
            exemption.kind
        );
    }
    Ok(())
}

fn validate_quality_exemption_target(root: &Path, exemption: &QualityExemption) -> Result<()> {
    let mut current = root.to_path_buf();
    let components = Path::new(&exemption.path).components().collect::<Vec<_>>();
    for (index, component) in components.iter().enumerate() {
        let Component::Normal(component) = component else {
            unreachable!("string-level exemption validation only permits normal components");
        };
        current.push(component);
        let metadata = std::fs::symlink_metadata(&current).with_context(|| {
            format!(
                "{} exemption path must name an existing ordinary file: {}",
                QUALITY_CONFIG_RELATIVE_PATH, exemption.path
            )
        })?;
        if is_link_or_reparse(&metadata) {
            bail!(
                "{} exemption path cannot traverse a symlink/reparse point: {}",
                QUALITY_CONFIG_RELATIVE_PATH,
                exemption.path
            );
        }
        let is_last = index + 1 == components.len();
        if (is_last && !metadata.is_file()) || (!is_last && !metadata.is_dir()) {
            bail!(
                "{} exemption path must name an existing ordinary file, not a directory or special entry: {}",
                QUALITY_CONFIG_RELATIVE_PATH,
                exemption.path
            );
        }
    }
    let canonical_root = root.canonicalize().with_context(|| {
        format!(
            "cannot canonicalize quality policy workspace: {}",
            root.display()
        )
    })?;
    let canonical_target = current.canonicalize().with_context(|| {
        format!(
            "cannot canonicalize quality exemption target: {}",
            current.display()
        )
    })?;
    if !canonical_target.starts_with(&canonical_root) {
        bail!(
            "{} exemption path escapes the workspace: {} -> {}",
            QUALITY_CONFIG_RELATIVE_PATH,
            exemption.path,
            canonical_target.display()
        );
    }
    Ok(())
}

pub fn quality_report(map: &ProjectMap) -> QualityReport {
    quality_report_with_config(map, &QualityConfig::standard())
}

pub fn quality_report_with_config(map: &ProjectMap, config: &QualityConfig) -> QualityReport {
    let candidate_covered_source_paths: BTreeSet<&str> = map
        .tests
        .iter()
        .flat_map(|test| test.candidate_paths.iter().map(String::as_str))
        .collect();
    let public_api_without_test_paths: BTreeSet<&str> = map
        .risks
        .iter()
        .filter(|risk| risk.kind == "public_api_without_test_evidence")
        .map(|risk| risk.path.as_str())
        .collect();

    let mut findings: Vec<QualityFinding> = map
        .risks
        .iter()
        .map(|risk| QualityFinding {
            severity: risk.severity.clone(),
            kind: risk.kind.clone(),
            path: risk.path.clone(),
            role: quality_role(&risk.path).into(),
            detail: risk.detail.clone(),
            recommendation: recommendation_for_risk(&risk.kind).into(),
            blocking_policy_source: None,
            exemption_reason: None,
        })
        .collect();

    // 逐包判定，不用任何全仓计数——两种全仓口径都会出错，而且方向相反：
    // "全仓存在任意包"会让一个无关的 fixture crate 把 JS 仓库判成硬失败；
    // "全仓受支持源文件数达阈值"又会把真实 Cargo 项目降级（2 个 .rs + 1 个
    // .js 就掉到阈值以下）。文档承诺的是"多源 Cargo/pyproject 包无测试则阻塞，
    // 未建模生态保持 advisory"，那就按包看包自己的源文件数与测试锚。
    let blocking_package = map.packages.iter().find(|package| {
        super::package_collects_own_sources(package)
            && package.source_files >= config.multi_source_no_test_min_sources
            && !super::package_has_test_anchor(map, package)
    });
    let has_supported_package = blocking_package.is_some();
    // advisory 半边不能与同一张 map 自相矛盾：一个自带测试锚（inline #[test]）
    // 的受支持包会让 test_files==0 而 tests 非空，此前的全仓计数照样告警且
    // 声称"未检测到任何 Cargo/pyproject 包"。把已被测试锚覆盖的包源文件从
    // 计数里扣除——advisory 只应度量未被任何已建模、有锚的包覆盖的源文件。
    let anchored_sources: usize = map
        .packages
        .iter()
        .filter(|package| {
            super::package_collects_own_sources(package)
                && super::package_has_test_anchor(map, package)
        })
        .map(|package| package.source_files)
        .sum();
    let unanchored_sources = map.source_files.saturating_sub(anchored_sources);
    if has_supported_package
        || (unanchored_sources >= config.multi_source_no_test_min_sources && map.test_files == 0)
    {
        findings.push(QualityFinding {
            severity: if has_supported_package {
                "error"
            } else {
                "warning"
            }
            .into(),
            kind: "multi_source_project_without_tests".into(),
            path: ".".into(),
            role: "workspace".into(),
            detail: match blocking_package {
                Some(package) => format!(
                    "package {} has {} source files but no indexed test files; large-project edits have no local validation anchor",
                    package.name, package.source_files
                ),
                None => format!(
                    "{} source files outside test-anchored modeled packages but no indexed test files; no blocking Cargo or pyproject package, so this heuristic is advisory only — verify test coverage manually",
                    unanchored_sources
                ),
            },
            recommendation: "add at least one test target or record why this workspace has no executable tests".into(),
            blocking_policy_source: None,
            exemption_reason: None,
        });
    }

    let blocking_warning_kinds: BTreeSet<&str> = config
        .block_warning_kinds
        .iter()
        .map(String::as_str)
        .collect();
    let strict_default_kinds: BTreeSet<&str> = config
        .strict_default_block_warning_kinds
        .iter()
        .map(String::as_str)
        .collect();
    let configured_exemptions: BTreeMap<(&str, &str), &QualityExemption> = config
        .exemptions
        .iter()
        .map(|exemption| {
            (
                (exemption.path.as_str(), exemption.kind.as_str()),
                exemption,
            )
        })
        .collect();

    for finding in &mut findings {
        let is_blocked_kind = blocking_warning_kinds.contains(finding.kind.as_str());
        let policy_source = is_blocked_kind.then(|| {
            if strict_default_kinds.contains(finding.kind.as_str()) {
                "strict_default"
            } else {
                "workspace_config"
            }
        });
        if let Some(exemption) =
            configured_exemptions.get(&(finding.path.as_str(), finding.kind.as_str()))
        {
            finding.severity = "info".into();
            finding.blocking_policy_source = policy_source.map(str::to_string);
            finding.exemption_reason = Some(exemption.reason.clone());
            finding.recommendation = format!(
                "{}; exact workspace exemption records this as informational: {}",
                finding.recommendation, exemption.reason
            );
            continue;
        }
        if finding.severity == "warning" && is_blocked_kind {
            let policy_source = policy_source.expect("blocked kind has a policy source");
            finding.blocking_policy_source = Some(policy_source.into());
            finding.severity = "error".into();
            finding.recommendation = if policy_source == "strict_default" {
                format!(
                    "{}; blocked by the strict built-in default policy",
                    finding.recommendation
                )
            } else {
                format!(
                    "{}; configured as blocking by .RaymanCodingSkill/quality.json",
                    finding.recommendation
                )
            };
        }
    }

    findings.sort_by(|a, b| {
        severity_rank(&a.severity)
            .cmp(&severity_rank(&b.severity))
            .then_with(|| a.kind.cmp(&b.kind))
            .then_with(|| a.path.cmp(&b.path))
    });
    let error_count = findings
        .iter()
        .filter(|finding| finding.severity == "error")
        .count();
    let warning_count = findings
        .iter()
        .filter(|finding| finding.severity == "warning")
        .count();
    let info_count = findings
        .iter()
        .filter(|finding| finding.severity == "info")
        .count();

    let mut findings_by_role = BTreeMap::<String, QualityRoleSummary>::new();
    for finding in &findings {
        let summary = findings_by_role.entry(finding.role.clone()).or_default();
        summary.findings += 1;
        match finding.severity.as_str() {
            "error" => summary.error_count += 1,
            "warning" => summary.warning_count += 1,
            "info" => summary.info_count += 1,
            _ => {}
        }
    }

    QualityReport {
        ready: error_count == 0,
        profile: config.profile.clone(),
        strict_default_block_warning_kinds: config.strict_default_block_warning_kinds.clone(),
        configured_block_warning_kinds: config.configured_block_warning_kinds.clone(),
        configured_exemptions: config.exemptions.clone(),
        source_files: map.source_files,
        test_files: map.test_files,
        candidate_test_covered_source_files: candidate_covered_source_paths.len(),
        public_api_files_without_test_evidence: public_api_without_test_paths.len(),
        error_count,
        warning_count,
        info_count,
        findings,
        findings_by_role,
    }
}

fn quality_role(path: &str) -> &'static str {
    if path == "." {
        return "workspace";
    }
    let segments = path.split('/').collect::<Vec<_>>();
    if segments.contains(&"fixture") {
        return "fixture";
    }
    if segments.contains(&"benches") || segments.contains(&"benchmarks") {
        return "benchmark";
    }
    if segments.contains(&"tests")
        || path.ends_with("_test.py")
        || path
            .rsplit('/')
            .next()
            .is_some_and(|name| name.starts_with("test_"))
    {
        return "test";
    }
    if segments.contains(&"generated") {
        return "generated";
    }
    "first_party"
}

fn recommendation_for_risk(kind: &str) -> &'static str {
    match kind {
        "large_file" => "split the file or inspect it before broad edits",
        "high_fan_in" => "treat changes as shared-contract changes and run broader tests",
        "public_api_without_test_evidence" => {
            "add/record a same-package test target before claiming coverage"
        }
        "no_symbols" => "confirm whether the file is generated, data-only, or missing indexed code",
        _ => "review before large-project edits",
    }
}

pub(super) fn severity_rank(severity: &str) -> usize {
    match severity {
        "error" => 0,
        "warning" => 1,
        "info" => 2,
        _ => 3,
    }
}

pub(super) fn infer_risks(
    index: &ContextIndex,
    symbols: &[MapSymbol],
    dependencies: &[Dependency],
    tests: &[TestTarget],
) -> Vec<MapRisk> {
    let mut risks = Vec::new();
    let mut incoming: BTreeMap<&str, usize> = BTreeMap::new();
    for dependency in dependencies {
        *incoming.entry(&dependency.to_path).or_default() += 1;
    }
    let covered: BTreeSet<&str> = tests
        .iter()
        .flat_map(|test| test.candidate_paths.iter().map(String::as_str))
        .collect();
    let public_by_path: BTreeMap<&str, usize> = symbols
        .iter()
        .filter(|symbol| symbol.visibility == "public")
        .fold(BTreeMap::new(), |mut acc, symbol| {
            *acc.entry(symbol.path.as_str()).or_default() += 1;
            acc
        });

    for file in &index.files {
        if file.kind != "source" && file.kind != "test" {
            continue;
        }
        if file.lines >= large_file_warning_lines(file.kind.as_str()) {
            risks.push(MapRisk {
                severity: "warning".into(),
                kind: "large_file".into(),
                path: file.path.clone(),
                detail: format!("{} lines; inspect before broad edits", file.lines),
            });
        }
        if file.kind == "source" && file.symbols.is_empty() {
            risks.push(MapRisk {
                severity: "info".into(),
                kind: "no_symbols".into(),
                path: file.path.clone(),
                detail: "source file has no indexed symbols".into(),
            });
        }
        if incoming.get(file.path.as_str()).copied().unwrap_or(0) >= 5 {
            risks.push(MapRisk {
                severity: "warning".into(),
                kind: "high_fan_in".into(),
                path: file.path.clone(),
                detail: "many local files depend on this file".into(),
            });
        }
        if file.kind == "source"
            && public_by_path.get(file.path.as_str()).copied().unwrap_or(0) > 0
            && !covered.contains(file.path.as_str())
            // A crate binary root carries `pub` items that no integration test
            // can import. Exempting only the literal root `src/main.rs` flagged
            // the identical layout in every nested crate of a workspace.
            && !is_crate_binary_root(&file.path)
        {
            risks.push(MapRisk {
                severity: "warning".into(),
                kind: "public_api_without_test_evidence".into(),
                path: file.path.clone(),
                detail:
                    "public symbols exist but no same-package candidate test target was inferred"
                        .into(),
            });
        }
    }
    risks
}

/// A Cargo binary crate root, at any nesting depth.
///
/// Exactly the two shapes cargo auto-discovers — `…/src/main.rs` and
/// `…/src/bin/<name>.rs` or `…/src/bin/<name>/main.rs` — and nothing else. A
/// bare `src/bin/` prefix would be the directory-shaped escape hatch
/// `docs/QUALITY_POLICY.md` forbids: it would exempt every file nested anywhere
/// under a `bin` directory, including ordinary modules that a test can import
/// and therefore should be held to the coverage rule.
fn is_crate_binary_root(path: &str) -> bool {
    if path == "src/main.rs" || path.ends_with("/src/main.rs") {
        return true;
    }
    let Some(rest) = path
        .strip_prefix("src/bin/")
        .or_else(|| path.split_once("/src/bin/").map(|(_, rest)| rest))
    else {
        return false;
    };
    match rest.split('/').collect::<Vec<_>>().as_slice() {
        [file] => file.ends_with(".rs"),
        [_directory, "main.rs"] => true,
        _ => false,
    }
}

fn large_file_warning_lines(kind: &str) -> usize {
    if kind == "test" {
        LARGE_TEST_WARNING_LINES
    } else {
        LARGE_SOURCE_WARNING_LINES
    }
}
