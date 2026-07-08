use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::context::{self, ContextIndex, FileEntry};
use crate::fsutil::{now_iso, read_json, write_json};

const PROJECT_MAP_RELATIVE_PATH: &str = ".RaymanCodingSkill/context/project_map.json";
const QUALITY_CONFIG_RELATIVE_PATH: &str = ".RaymanCodingSkill/quality.json";
const BLOCKABLE_WARNING_KINDS: &[&str] = &[
    "large_file",
    "high_fan_in",
    "public_api_without_test_evidence",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectMap {
    pub generated_at: String,
    pub workspace: String,
    pub source_files: usize,
    pub test_files: usize,
    pub docs_files: usize,
    pub config_files: usize,
    pub asset_files: usize,
    pub modules: Vec<ModuleEntry>,
    pub symbols: Vec<MapSymbol>,
    pub dependencies: Vec<Dependency>,
    pub packages: Vec<PackageEntry>,
    pub package_dependencies: Vec<PackageDependency>,
    pub entrypoints: Vec<EntryPoint>,
    pub tests: Vec<TestTarget>,
    pub risks: Vec<MapRisk>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleEntry {
    pub name: String,
    pub path: String,
    pub kind: String,
    pub lines: usize,
    pub symbols: usize,
    pub public_symbols: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MapSymbol {
    pub name: String,
    pub kind: String,
    pub visibility: String,
    pub module: String,
    pub path: String,
    pub line: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Dependency {
    pub from_path: String,
    pub to_path: String,
    pub kind: String,
    pub evidence: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageEntry {
    pub name: String,
    pub root_path: String,
    pub manifest_path: String,
    pub workspace_member: bool,
    pub source_files: usize,
    pub test_files: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageDependency {
    pub from_package: String,
    pub from_root_path: String,
    pub to_package: String,
    pub to_root_path: String,
    pub dependency_name: String,
    pub kind: String,
    pub manifest_path: String,
    pub evidence: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntryPoint {
    pub name: String,
    pub kind: String,
    pub path: String,
    pub line: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestTarget {
    pub path: String,
    pub kind: String,
    pub test_count: usize,
    pub candidate_paths: Vec<String>,
    pub basis: String,
    pub confidence: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MapRisk {
    pub severity: String,
    pub kind: String,
    pub path: String,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct MapSummary {
    pub generated_at: String,
    pub files: usize,
    pub source_files: usize,
    pub test_files: usize,
    pub modules: usize,
    pub symbols: usize,
    pub dependencies: usize,
    pub packages: usize,
    pub package_dependencies: usize,
    pub entrypoints: usize,
    pub tests: usize,
    pub risks: usize,
    pub warnings: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct FileReport {
    pub path: String,
    pub module: Option<ModuleEntry>,
    pub symbols: Vec<MapSymbol>,
    pub outgoing_dependencies: Vec<Dependency>,
    pub incoming_dependencies: Vec<Dependency>,
    pub related_tests: Vec<TestTarget>,
    pub risks: Vec<MapRisk>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SymbolReport {
    pub query: String,
    pub matches: Vec<MapSymbol>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TopologyReport {
    pub packages: Vec<PackageEntry>,
    pub package_dependencies: Vec<PackageDependency>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ImpactReport {
    pub changed_path: String,
    pub package: Option<String>,
    pub direct_dependencies: Vec<Dependency>,
    pub direct_dependents: Vec<Dependency>,
    pub package_dependencies: Vec<PackageDependency>,
    pub package_dependents: Vec<PackageDependency>,
    pub related_tests: Vec<TestTarget>,
    pub risks: Vec<MapRisk>,
    pub recommended_checks: Vec<String>,
    pub recommendation_basis: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChangePlan {
    pub ready: bool,
    pub changed_paths: Vec<String>,
    pub review_priority: String,
    pub impacted_files: Vec<PlanFile>,
    pub related_tests: Vec<TestTarget>,
    pub risks: Vec<MapRisk>,
    pub recommended_checks: Vec<String>,
    pub blockers: Vec<String>,
    pub warnings: Vec<String>,
    pub recommendation_basis: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlanFile {
    pub path: String,
    pub role: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct QualityReport {
    pub ready: bool,
    pub profile: String,
    pub source_files: usize,
    pub test_files: usize,
    pub candidate_test_covered_source_files: usize,
    pub public_api_files_without_test_evidence: usize,
    pub error_count: usize,
    pub warning_count: usize,
    pub info_count: usize,
    pub findings: Vec<QualityFinding>,
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
}

#[derive(Debug, Clone, Serialize)]
pub struct QualityFinding {
    pub severity: String,
    pub kind: String,
    pub path: String,
    pub detail: String,
    pub recommendation: String,
}

#[derive(Debug, Clone, Default)]
struct WorkspaceInfo {
    member_patterns: Vec<String>,
    exclude_patterns: Vec<String>,
    path_dependencies: BTreeMap<String, String>,
    has_workspace_section: bool,
}

pub fn build(root: &Path) -> Result<ProjectMap> {
    let map = build_readonly(root)?;
    write_json(&project_map_path(root), &map)?;
    Ok(map)
}

pub fn build_readonly(root: &Path) -> Result<ProjectMap> {
    let freshness = context::freshness(root);
    if freshness.status != "ready" {
        bail!(
            "上下文索引不是 ready（当前: {}）。先运行 `rayman context refresh`。",
            freshness.status
        );
    }
    let Some(index) = context::load(root)? else {
        bail!("上下文索引缺失。先运行 `rayman context refresh`。");
    };
    build_from_index(root, &index)
}

pub fn load_quality_config(root: &Path, profile: &str) -> Result<QualityConfig> {
    let mut config = match profile {
        "strict" => QualityConfig::strict(),
        _ => QualityConfig::standard(),
    };
    let path = root.join(QUALITY_CONFIG_RELATIVE_PATH);
    if path.exists() {
        let Some(file_config) = read_json::<QualityConfig>(&path)? else {
            return Ok(config);
        };
        validate_quality_config(&file_config)?;
        config.multi_source_no_test_min_sources =
            file_config.multi_source_no_test_min_sources.max(1);
        config.block_warning_kinds = file_config.block_warning_kinds;
        config.profile = profile.into();
    }
    Ok(config)
}

pub fn summary(map: &ProjectMap) -> MapSummary {
    MapSummary {
        generated_at: map.generated_at.clone(),
        files: map.source_files
            + map.test_files
            + map.docs_files
            + map.config_files
            + map.asset_files,
        source_files: map.source_files,
        test_files: map.test_files,
        modules: map.modules.len(),
        symbols: map.symbols.len(),
        dependencies: map.dependencies.len(),
        packages: map.packages.len(),
        package_dependencies: map.package_dependencies.len(),
        entrypoints: map.entrypoints.len(),
        tests: map.tests.len(),
        risks: map.risks.len(),
        warnings: map
            .risks
            .iter()
            .filter(|risk| risk.severity == "warning")
            .count(),
    }
}

pub fn file_report(map: &ProjectMap, path: &str) -> Result<FileReport> {
    let path = normalize_query_path(path);
    let Some(module) = map
        .modules
        .iter()
        .find(|module| module.path == path)
        .cloned()
    else {
        bail!("项目地图中没有文件: {path}");
    };
    Ok(FileReport {
        path: path.clone(),
        module: Some(module),
        symbols: map
            .symbols
            .iter()
            .filter(|symbol| symbol.path == path)
            .cloned()
            .collect(),
        outgoing_dependencies: map
            .dependencies
            .iter()
            .filter(|dependency| dependency.from_path == path)
            .cloned()
            .collect(),
        incoming_dependencies: map
            .dependencies
            .iter()
            .filter(|dependency| dependency.to_path == path)
            .cloned()
            .collect(),
        related_tests: related_tests(map, &path),
        risks: map
            .risks
            .iter()
            .filter(|risk| risk.path == path)
            .cloned()
            .collect(),
    })
}

pub fn symbol_report(map: &ProjectMap, name: &str) -> SymbolReport {
    let query = name.to_ascii_lowercase();
    SymbolReport {
        query: name.to_string(),
        matches: map
            .symbols
            .iter()
            .filter(|symbol| symbol.name.to_ascii_lowercase().contains(&query))
            .cloned()
            .collect(),
    }
}

pub fn topology_report(map: &ProjectMap) -> TopologyReport {
    TopologyReport {
        packages: map.packages.clone(),
        package_dependencies: map.package_dependencies.clone(),
    }
}

pub fn impact_report(map: &ProjectMap, path: &str) -> Result<ImpactReport> {
    let path = normalize_query_path(path);
    if !map.modules.iter().any(|module| module.path == path) {
        bail!("项目地图中没有文件: {path}");
    }
    let package_entry = package_for_path(map, &path);
    let package = package_entry.map(|package| package.name.clone());
    let direct_dependencies: Vec<Dependency> = map
        .dependencies
        .iter()
        .filter(|dependency| dependency.from_path == path)
        .cloned()
        .collect();
    let direct_dependents: Vec<Dependency> = map
        .dependencies
        .iter()
        .filter(|dependency| dependency.to_path == path)
        .cloned()
        .collect();
    let package_dependencies: Vec<PackageDependency> = package_entry
        .map(|package| {
            map.package_dependencies
                .iter()
                .filter(|dependency| dependency.from_root_path == package.root_path)
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    let package_dependents: Vec<PackageDependency> = package_entry
        .map(|package| {
            map.package_dependencies
                .iter()
                .filter(|dependency| dependency.to_root_path == package.root_path)
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    let related_tests = related_tests(map, &path);
    let mut recommended_checks = Vec::new();
    if related_tests.is_empty() {
        recommended_checks.push("run the project's focused test or add one for this change".into());
    } else {
        for test in &related_tests {
            recommended_checks.push(format!("run candidate related test {}", test.path));
        }
    }
    if let Some(package) = package_entry {
        recommended_checks.push(package_test_command(map, package));
    }
    for dependent in &package_dependents {
        if let Some(package) = map
            .packages
            .iter()
            .find(|package| package.root_path == dependent.from_root_path)
        {
            recommended_checks.push(package_test_command(map, package));
        }
    }
    if path.ends_with("Cargo.toml") || path.ends_with("Cargo.lock") {
        recommended_checks.push("cargo check --all".into());
        recommended_checks.push("cargo deny check".into());
        recommended_checks.push("cargo test --all".into());
    }
    if path.ends_with(".rs") {
        recommended_checks.push("cargo test --all".into());
        recommended_checks.push("cargo clippy --all-targets -- -D warnings".into());
    }
    recommended_checks.sort();
    recommended_checks.dedup();

    Ok(ImpactReport {
        changed_path: path.clone(),
        package,
        direct_dependencies,
        direct_dependents,
        package_dependencies,
        package_dependents,
        related_tests,
        risks: map
            .risks
            .iter()
            .filter(|risk| risk.path == path)
            .cloned()
            .collect(),
        recommended_checks,
        recommendation_basis:
            "dependency facts plus heuristic test candidates; not proof of real coverage".into(),
    })
}

pub fn change_plan(map: &ProjectMap, paths: &[String]) -> Result<ChangePlan> {
    if paths.is_empty() {
        bail!("至少提供一个变更路径。");
    }

    let mut changed_paths = paths
        .iter()
        .map(|path| normalize_query_path(path))
        .collect::<Vec<_>>();
    changed_paths.sort();
    changed_paths.dedup();

    let module_by_path: BTreeMap<&str, &ModuleEntry> = map
        .modules
        .iter()
        .map(|module| (module.path.as_str(), module))
        .collect();
    let mut impacted_by_path: BTreeMap<String, PlanFile> = BTreeMap::new();
    let mut tests_by_path: BTreeMap<String, TestTarget> = BTreeMap::new();
    let mut risks = Vec::new();
    let mut recommended_checks = BTreeSet::new();
    let mut warnings = Vec::new();
    let mut source_changed_count = 0usize;
    let mut has_package_test_anchor = false;

    for path in &changed_paths {
        let Some(module) = module_by_path.get(path.as_str()) else {
            bail!("项目地图中没有文件: {path}");
        };
        if module.kind == "source" {
            source_changed_count += 1;
        }

        let report = impact_report(map, path)?;
        record_plan_file(
            &mut impacted_by_path,
            path.clone(),
            "changed",
            "explicitly requested change path".into(),
        );
        for dependency in &report.direct_dependencies {
            record_plan_file(
                &mut impacted_by_path,
                dependency.to_path.clone(),
                "dependency",
                format!("{path} depends on this file"),
            );
        }
        for dependent in &report.direct_dependents {
            record_plan_file(
                &mut impacted_by_path,
                dependent.from_path.clone(),
                "dependent",
                format!("depends on changed file {path}"),
            );
            for test in related_tests(map, &dependent.from_path) {
                tests_by_path
                    .entry(test.path.clone())
                    .or_insert_with(|| test);
            }
        }
        for test in &report.related_tests {
            tests_by_path
                .entry(test.path.clone())
                .or_insert_with(|| test.clone());
        }
        if impact_has_package_test_anchor(map, &report) {
            has_package_test_anchor = true;
        }
        risks.extend(report.risks);
        recommended_checks.extend(report.recommended_checks);
    }

    risks.sort_by(|a, b| {
        severity_rank(&a.severity)
            .cmp(&severity_rank(&b.severity))
            .then_with(|| a.kind.cmp(&b.kind))
            .then_with(|| a.path.cmp(&b.path))
            .then_with(|| a.detail.cmp(&b.detail))
    });
    risks.dedup_by(|a, b| {
        a.severity == b.severity && a.kind == b.kind && a.path == b.path && a.detail == b.detail
    });

    if changed_paths.len() > 1 {
        warnings.push(format!(
            "{} changed paths requested; review as one change set, not isolated files",
            changed_paths.len()
        ));
    }
    if impacted_by_path.len() >= 8 {
        warnings.push(format!(
            "{} files in the immediate impact set; split or run broad checks before claiming ready",
            impacted_by_path.len()
        ));
    }
    if risks.iter().any(|risk| risk.kind == "high_fan_in") {
        warnings.push("one or more changed files are depended on by many local files".into());
    }

    let has_validation_anchor = !tests_by_path.is_empty() || has_package_test_anchor;

    let mut blockers = Vec::new();
    if source_changed_count >= 3 && !has_validation_anchor {
        blockers.push(format!(
            "{source_changed_count} source files are in scope but no same-package candidate test target or indexed package test anchor was inferred"
        ));
    }

    let review_priority = if !blockers.is_empty()
        || risks.iter().any(|risk| risk.kind == "high_fan_in")
        || impacted_by_path.len() >= 8
    {
        "high"
    } else if changed_paths.len() > 1 || impacted_by_path.len() >= 4 {
        "broad"
    } else {
        "normal"
    }
    .into();

    Ok(ChangePlan {
        ready: blockers.is_empty(),
        changed_paths,
        review_priority,
        impacted_files: impacted_by_path.into_values().collect(),
        related_tests: tests_by_path.into_values().collect(),
        risks,
        recommended_checks: recommended_checks.into_iter().collect(),
        blockers,
        warnings,
        recommendation_basis:
            "multi-file impact aggregation from project-map dependencies, heuristic test candidates, and local risk signals; not proof of real coverage"
                .into(),
    })
}

impl QualityConfig {
    pub fn standard() -> Self {
        Self {
            profile: "standard".into(),
            multi_source_no_test_min_sources: default_multi_source_no_test_min_sources(),
            block_warning_kinds: Vec::new(),
        }
    }

    pub fn strict() -> Self {
        Self {
            profile: "strict".into(),
            multi_source_no_test_min_sources: default_multi_source_no_test_min_sources(),
            block_warning_kinds: Vec::new(),
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
    for kind in &config.block_warning_kinds {
        if !BLOCKABLE_WARNING_KINDS.contains(&kind.as_str()) {
            bail!(
                "{} has unknown block_warning_kinds entry `{}`; allowed: {}",
                QUALITY_CONFIG_RELATIVE_PATH,
                kind,
                BLOCKABLE_WARNING_KINDS.join(", ")
            );
        }
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
            detail: risk.detail.clone(),
            recommendation: recommendation_for_risk(&risk.kind).into(),
        })
        .collect();

    if map.source_files >= config.multi_source_no_test_min_sources && map.test_files == 0 {
        findings.push(QualityFinding {
            severity: "error".into(),
            kind: "multi_source_project_without_tests".into(),
            path: ".".into(),
            detail: format!(
                "{} source files but no indexed test files; large-project edits have no local validation anchor",
                map.source_files
            ),
            recommendation: "add at least one test target or record why this workspace has no executable tests".into(),
        });
    }
    let blocking_warning_kinds: BTreeSet<&str> = config
        .block_warning_kinds
        .iter()
        .map(String::as_str)
        .collect();
    for finding in &mut findings {
        if finding.severity == "warning" && blocking_warning_kinds.contains(finding.kind.as_str()) {
            finding.severity = "error".into();
            finding.recommendation = format!(
                "{}; configured as blocking by .RaymanCodingSkill/quality.json",
                finding.recommendation
            );
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

    QualityReport {
        ready: error_count == 0,
        profile: config.profile.clone(),
        source_files: map.source_files,
        test_files: map.test_files,
        candidate_test_covered_source_files: candidate_covered_source_paths.len(),
        public_api_files_without_test_evidence: public_api_without_test_paths.len(),
        error_count,
        warning_count,
        info_count,
        findings,
    }
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

fn severity_rank(severity: &str) -> usize {
    match severity {
        "error" => 0,
        "warning" => 1,
        "info" => 2,
        _ => 3,
    }
}

fn record_plan_file(
    files: &mut BTreeMap<String, PlanFile>,
    path: String,
    role: &str,
    reason: String,
) {
    if let Some(existing) = files.get_mut(&path) {
        if existing.role != "changed" && role == "changed" {
            existing.role = role.into();
            existing.reason = reason;
        } else if !existing.reason.contains(&reason) {
            existing.reason = format!("{}; {}", existing.reason, reason);
        }
        return;
    }

    files.insert(
        path.clone(),
        PlanFile {
            path,
            role: role.into(),
            reason,
        },
    );
}

fn impact_has_package_test_anchor(map: &ProjectMap, report: &ImpactReport) -> bool {
    if let Some(package) = package_for_path(map, &report.changed_path)
        && package_has_test_anchor(map, package)
    {
        return true;
    }
    report.package_dependents.iter().any(|dependent| {
        map.packages
            .iter()
            .find(|package| package.root_path == dependent.from_root_path)
            .is_some_and(|package| package_has_test_anchor(map, package))
    })
}

fn package_has_test_anchor(map: &ProjectMap, package: &PackageEntry) -> bool {
    package.test_files > 0
        || map
            .tests
            .iter()
            .any(|test| path_is_under_package(&test.path, &package.root_path))
}

fn project_map_path(root: &Path) -> PathBuf {
    root.join(PROJECT_MAP_RELATIVE_PATH)
}

fn build_from_index(root: &Path, index: &ContextIndex) -> Result<ProjectMap> {
    let mut modules = Vec::new();
    let mut symbols = Vec::new();
    let mut entrypoints = Vec::new();
    let mut tests = Vec::new();
    let mut text_by_path = BTreeMap::new();
    let path_set: BTreeSet<String> = index.files.iter().map(|file| file.path.clone()).collect();

    for file in &index.files {
        if file.kind == "source" || file.kind == "test" {
            let text = std::fs::read_to_string(root.join(&file.path))
                .with_context(|| format!("无法读取源码文件用于项目地图: {}", file.path))?;
            text_by_path.insert(file.path.clone(), text);
        }
    }

    for file in &index.files {
        let module_name = module_name_for(&file.path);
        let text = text_by_path
            .get(&file.path)
            .map(String::as_str)
            .unwrap_or("");
        let public_symbols = file
            .symbols
            .iter()
            .filter(|symbol| symbol_visibility(text, symbol.line) == "public")
            .count();
        modules.push(ModuleEntry {
            name: module_name.clone(),
            path: file.path.clone(),
            kind: file.kind.clone(),
            lines: file.lines,
            symbols: file.symbols.len(),
            public_symbols,
        });

        for symbol in &file.symbols {
            let visibility = symbol_visibility(text, symbol.line);
            if symbol.name == "main" || symbol.kind == "route" {
                entrypoints.push(EntryPoint {
                    name: symbol.name.clone(),
                    kind: symbol.kind.clone(),
                    path: file.path.clone(),
                    line: symbol.line,
                });
            }
            symbols.push(MapSymbol {
                name: symbol.name.clone(),
                kind: symbol.kind.clone(),
                visibility: visibility.into(),
                module: module_name.clone(),
                path: file.path.clone(),
                line: symbol.line,
            });
        }

        if file.kind == "test" || text.contains("#[cfg(test)]") || text.contains("#[test]") {
            let inference = infer_candidate_paths(file, text, index);
            tests.push(TestTarget {
                path: file.path.clone(),
                kind: if file.kind == "test" {
                    "integration".into()
                } else {
                    "inline".into()
                },
                test_count: count_tests(text),
                candidate_paths: inference.paths,
                basis: inference.basis,
                confidence: inference.confidence,
            });
        }
    }

    let workspace = read_workspace_info(root)?;
    let packages = discover_packages(root, index, &workspace)?;
    let package_dependencies = infer_package_dependencies(root, &packages, &workspace)?;
    let dependencies = infer_dependencies(&text_by_path, &path_set);
    let risks = infer_risks(index, &symbols, &dependencies, &tests);
    let (source_files, test_files, docs_files, config_files, asset_files) = count_kinds(index);

    Ok(ProjectMap {
        generated_at: now_iso(),
        workspace: index.workspace.clone(),
        source_files,
        test_files,
        docs_files,
        config_files,
        asset_files,
        modules,
        symbols,
        dependencies,
        packages,
        package_dependencies,
        entrypoints,
        tests,
        risks,
    })
}

fn normalize_query_path(path: &str) -> String {
    path.trim()
        .trim_start_matches("./")
        .trim_start_matches(".\\")
        .replace('\\', "/")
}

fn count_kinds(index: &ContextIndex) -> (usize, usize, usize, usize, usize) {
    let source = index
        .files
        .iter()
        .filter(|file| file.kind == "source")
        .count();
    let test = index
        .files
        .iter()
        .filter(|file| file.kind == "test")
        .count();
    let docs = index
        .files
        .iter()
        .filter(|file| file.kind == "docs")
        .count();
    let config = index
        .files
        .iter()
        .filter(|file| file.kind == "config")
        .count();
    let asset = index
        .files
        .iter()
        .filter(|file| file.kind == "asset")
        .count();
    (source, test, docs, config, asset)
}

fn read_workspace_info(root: &Path) -> Result<WorkspaceInfo> {
    let manifest = root.join("Cargo.toml");
    if !manifest.exists() {
        return Ok(WorkspaceInfo::default());
    }
    let text = std::fs::read_to_string(&manifest)
        .with_context(|| format!("无法读取 Cargo workspace manifest: {}", manifest.display()))?;
    let mut info = WorkspaceInfo {
        member_patterns: parse_workspace_array(&text, "members"),
        exclude_patterns: parse_workspace_array(&text, "exclude"),
        path_dependencies: BTreeMap::new(),
        has_workspace_section: has_section(&text, "workspace"),
    };

    let mut section = String::new();
    let mut nested_dependency: Option<String> = None;
    for raw in text.lines() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            section = line
                .trim_start_matches('[')
                .trim_end_matches(']')
                .trim()
                .to_string();
            nested_dependency = section
                .strip_prefix("workspace.dependencies.")
                .map(|name| name.trim_matches('"').to_string());
            continue;
        }
        let dependency_name = nested_dependency.clone().or_else(|| {
            if section == "workspace.dependencies" {
                line.split_once('=')
                    .map(|(name, _)| dependency_key_name(name))
            } else {
                None
            }
        });
        let Some(dependency_name) = dependency_name else {
            continue;
        };
        if let Some(path) = quoted_value_after(line, "path") {
            info.path_dependencies
                .insert(dependency_name, normalize_join_relative(".", &path));
        }
    }

    Ok(info)
}

fn parse_workspace_array(text: &str, key: &str) -> Vec<String> {
    let mut values = Vec::new();
    let mut in_workspace = false;
    let mut collecting = false;
    let mut buffer = String::new();
    for raw in text.lines() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.starts_with('[') && line.ends_with(']') {
            in_workspace = line == "[workspace]";
            collecting = false;
            buffer.clear();
            continue;
        }
        if !in_workspace {
            continue;
        }
        if collecting {
            buffer.push(' ');
            buffer.push_str(line);
            if line.contains(']') {
                values.extend(quoted_strings(&buffer));
                collecting = false;
                buffer.clear();
            }
            continue;
        }
        let Some((left, right)) = line.split_once('=') else {
            continue;
        };
        if left.trim() != key {
            continue;
        }
        buffer.push_str(right);
        if right.contains(']') {
            values.extend(quoted_strings(&buffer));
            buffer.clear();
        } else {
            collecting = true;
        }
    }
    values
        .into_iter()
        .map(|value| normalize_join_relative(".", &value))
        .collect()
}

fn has_section(text: &str, section: &str) -> bool {
    text.lines().any(|raw| {
        let line = raw.split('#').next().unwrap_or("").trim();
        line == format!("[{section}]")
    })
}

fn discover_packages(
    root: &Path,
    index: &ContextIndex,
    workspace: &WorkspaceInfo,
) -> Result<Vec<PackageEntry>> {
    let mut packages = Vec::new();
    for file in &index.files {
        if !file.path.ends_with("Cargo.toml") {
            continue;
        }
        let text = std::fs::read_to_string(root.join(&file.path))
            .with_context(|| format!("无法读取 Cargo manifest: {}", file.path))?;
        let Some(name) = parse_package_name(&text) else {
            continue;
        };
        let root_path = manifest_root_path(&file.path);
        packages.push(PackageEntry {
            name,
            workspace_member: package_is_workspace_member(&root_path, workspace),
            root_path,
            manifest_path: file.path.clone(),
            source_files: 0,
            test_files: 0,
        });
    }

    packages.sort_by(|a, b| a.root_path.cmp(&b.root_path));
    for file in &index.files {
        if file.kind != "source" && file.kind != "test" {
            continue;
        }
        if let Some(index) = package_index_for_path(&packages, &file.path) {
            if file.kind == "source" {
                packages[index].source_files += 1;
            } else {
                packages[index].test_files += 1;
            }
        }
    }
    Ok(packages)
}

fn infer_package_dependencies(
    root: &Path,
    packages: &[PackageEntry],
    workspace: &WorkspaceInfo,
) -> Result<Vec<PackageDependency>> {
    let package_by_root: BTreeMap<&str, &PackageEntry> = packages
        .iter()
        .map(|package| (package.root_path.as_str(), package))
        .collect();
    let mut dependencies = Vec::new();
    let mut seen = BTreeSet::new();

    for package in packages {
        let text = std::fs::read_to_string(root.join(&package.manifest_path))
            .with_context(|| format!("无法读取 Cargo manifest: {}", package.manifest_path))?;
        let mut section = String::new();
        let mut nested_dependency: Option<String> = None;
        for (line_index, raw) in text.lines().enumerate() {
            let line = raw.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            if line.starts_with('[') && line.ends_with(']') {
                section = line
                    .trim_start_matches('[')
                    .trim_end_matches(']')
                    .trim()
                    .to_string();
                nested_dependency = nested_dependency_name(&section);
                continue;
            }
            let dependency_name = nested_dependency.clone().or_else(|| {
                if is_dependency_section(&section) {
                    line.split_once('=')
                        .map(|(name, _)| dependency_key_name(name))
                } else {
                    None
                }
            });
            let Some(dependency_name) = dependency_name else {
                continue;
            };
            let path_value = if let Some(path) = quoted_value_after(line, "path") {
                Some((path, package.root_path.as_str()))
            } else if bool_value_after(line, "workspace") == Some(true) {
                workspace
                    .path_dependencies
                    .get(&dependency_name)
                    .cloned()
                    .map(|path| (path, "."))
            } else {
                None
            };
            let Some((path_value, path_base)) = path_value else {
                continue;
            };
            let dependency_root = normalize_join_relative(path_base, &path_value);
            let Some(target_package) = package_by_root.get(dependency_root.as_str()) else {
                continue;
            };
            let key = format!(
                "{}->{}:{}",
                package.root_path, target_package.root_path, dependency_name
            );
            if seen.insert(key) {
                dependencies.push(PackageDependency {
                    from_package: package.name.clone(),
                    from_root_path: package.root_path.clone(),
                    to_package: target_package.name.clone(),
                    to_root_path: target_package.root_path.clone(),
                    dependency_name,
                    kind: "cargo_path".into(),
                    manifest_path: package.manifest_path.clone(),
                    evidence: format!("{}: path={path_value}", line_index + 1),
                });
            }
        }
    }

    Ok(dependencies)
}

fn module_name_for(path: &str) -> String {
    if path == "src/lib.rs" {
        return "crate".into();
    }
    if path == "src/main.rs" {
        return "bin".into();
    }
    let trimmed = path
        .strip_prefix("src/")
        .or_else(|| path.strip_prefix("tests/"))
        .unwrap_or(path)
        .trim_end_matches(".rs");
    let trimmed = trimmed.trim_end_matches("/mod");
    trimmed.replace('/', "::")
}

fn parse_package_name(text: &str) -> Option<String> {
    let mut in_package = false;
    for raw in text.lines() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.starts_with('[') && line.ends_with(']') {
            in_package = line == "[package]";
            continue;
        }
        if in_package && let Some(value) = quoted_assignment(line, "name") {
            return Some(value);
        }
    }
    None
}

fn quoted_assignment(line: &str, key: &str) -> Option<String> {
    let (left, right) = line.split_once('=')?;
    if left.trim() != key {
        return None;
    }
    quoted_string(right)
}

fn dependency_key_name(left: &str) -> String {
    left.trim()
        .split('.')
        .next()
        .unwrap_or("")
        .trim()
        .trim_matches('"')
        .to_string()
}

fn quoted_value_after(line: &str, key: &str) -> Option<String> {
    let key_index = line.find(key)?;
    let after_key = &line[key_index + key.len()..];
    let (_, value) = after_key.split_once('=')?;
    quoted_string(value)
}

fn bool_value_after(line: &str, key: &str) -> Option<bool> {
    let key_index = line.find(key)?;
    let after_key = &line[key_index + key.len()..];
    let (_, value) = after_key.split_once('=')?;
    let value = value.trim_start();
    if value.starts_with("true") {
        Some(true)
    } else if value.starts_with("false") {
        Some(false)
    } else {
        None
    }
}

fn quoted_string(text: &str) -> Option<String> {
    let start = text.find('"')? + 1;
    let rest = &text[start..];
    let end = rest.find('"')?;
    Some(rest[..end].replace('\\', "/"))
}

fn quoted_strings(text: &str) -> Vec<String> {
    let mut values = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find('"') {
        let tail = &rest[start + 1..];
        let Some(end) = tail.find('"') else {
            break;
        };
        values.push(tail[..end].replace('\\', "/"));
        rest = &tail[end + 1..];
    }
    values
}

fn manifest_root_path(manifest_path: &str) -> String {
    let parent = Path::new(manifest_path)
        .parent()
        .and_then(|parent| parent.to_str())
        .unwrap_or("");
    if parent.is_empty() {
        ".".into()
    } else {
        parent.replace('\\', "/")
    }
}

fn package_is_workspace_member(root_path: &str, workspace: &WorkspaceInfo) -> bool {
    if workspace
        .exclude_patterns
        .iter()
        .any(|pattern| path_matches_workspace_exclude_pattern(root_path, pattern))
    {
        return false;
    }
    if workspace.member_patterns.is_empty() {
        return !workspace.has_workspace_section && root_path == ".";
    }
    workspace
        .member_patterns
        .iter()
        .any(|pattern| path_matches_workspace_member_pattern(root_path, pattern))
}

fn path_matches_workspace_member_pattern(path: &str, pattern: &str) -> bool {
    if pattern == "." {
        return path == ".";
    }
    if !pattern_has_glob(pattern) {
        return path == pattern;
    }
    path_matches_workspace_glob(path, pattern)
}

fn path_matches_workspace_exclude_pattern(path: &str, pattern: &str) -> bool {
    if pattern == "." {
        return path == ".";
    }
    if !pattern_has_glob(pattern) {
        return path == pattern || path_is_under_root(path, pattern);
    }
    path_matches_workspace_glob(path, pattern)
}

fn path_matches_workspace_glob(path: &str, pattern: &str) -> bool {
    let path_segments = workspace_path_segments(path);
    let pattern_segments = workspace_path_segments(pattern);
    path_segments_match(&pattern_segments, &path_segments)
}

fn pattern_has_glob(pattern: &str) -> bool {
    pattern.contains('*') || pattern.contains('?')
}

fn workspace_path_segments(path: &str) -> Vec<&str> {
    path.split('/')
        .filter(|segment| !segment.is_empty() && *segment != ".")
        .collect()
}

fn path_segments_match(pattern: &[&str], path: &[&str]) -> bool {
    if pattern.is_empty() {
        return path.is_empty();
    }
    if pattern[0] == "**" {
        return path_segments_match(&pattern[1..], path)
            || (!path.is_empty() && path_segments_match(pattern, &path[1..]));
    }
    !path.is_empty()
        && segment_matches(pattern[0], path[0])
        && path_segments_match(&pattern[1..], &path[1..])
}

fn segment_matches(pattern: &str, value: &str) -> bool {
    segment_match_bytes(pattern.as_bytes(), value.as_bytes())
}

fn segment_match_bytes(pattern: &[u8], value: &[u8]) -> bool {
    if pattern.is_empty() {
        return value.is_empty();
    }
    match pattern[0] {
        b'*' => {
            segment_match_bytes(&pattern[1..], value)
                || (!value.is_empty() && segment_match_bytes(pattern, &value[1..]))
        }
        b'?' => !value.is_empty() && segment_match_bytes(&pattern[1..], &value[1..]),
        literal => {
            !value.is_empty()
                && literal == value[0]
                && segment_match_bytes(&pattern[1..], &value[1..])
        }
    }
}

fn path_is_under_root(path: &str, root: &str) -> bool {
    path == root
        || path
            .strip_prefix(root)
            .is_some_and(|rest| rest.starts_with('/'))
}

fn package_for_path<'a>(map: &'a ProjectMap, path: &str) -> Option<&'a PackageEntry> {
    map.packages
        .iter()
        .filter(|package| path_is_under_package(path, &package.root_path))
        .max_by_key(|package| package.root_path.len())
}

fn package_test_command(map: &ProjectMap, package: &PackageEntry) -> String {
    let unique_workspace_name = package.workspace_member
        && map
            .packages
            .iter()
            .filter(|candidate| candidate.workspace_member && candidate.name == package.name)
            .count()
            == 1;
    if unique_workspace_name {
        format!("cargo test -p {}", package.name)
    } else {
        format!("cargo test --manifest-path {}", package.manifest_path)
    }
}

fn package_index_for_path(packages: &[PackageEntry], path: &str) -> Option<usize> {
    packages
        .iter()
        .enumerate()
        .filter(|(_, package)| path_is_under_package(path, &package.root_path))
        .max_by_key(|(_, package)| package.root_path.len())
        .map(|(index, _)| index)
}

fn path_is_under_package(path: &str, package_root: &str) -> bool {
    package_root == "." || path_is_under_root(path, package_root)
}

fn is_dependency_section(section: &str) -> bool {
    section == "dependencies"
        || section == "dev-dependencies"
        || section == "build-dependencies"
        || (section.starts_with("target.") && section.ends_with(".dependencies"))
}

fn nested_dependency_name(section: &str) -> Option<String> {
    for prefix in ["dependencies.", "dev-dependencies.", "build-dependencies."] {
        if let Some(name) = section.strip_prefix(prefix) {
            return Some(name.trim_matches('"').to_string());
        }
    }
    if section.starts_with("target.") {
        for marker in [
            ".dependencies.",
            ".dev-dependencies.",
            ".build-dependencies.",
        ] {
            if let Some((_, name)) = section.split_once(marker) {
                return Some(name.trim_matches('"').to_string());
            }
        }
    }
    None
}

fn normalize_join_relative(base: &str, raw: &str) -> String {
    let mut parts = Vec::new();
    if base != "." {
        parts.extend(
            base.split('/')
                .filter(|part| !part.is_empty())
                .map(str::to_string),
        );
    }
    let raw = raw.replace('\\', "/");
    for part in raw.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other.to_string()),
        }
    }
    if parts.is_empty() {
        ".".into()
    } else {
        parts.join("/")
    }
}

fn symbol_visibility(text: &str, line: usize) -> &'static str {
    let Some(raw) = text.lines().nth(line.saturating_sub(1)) else {
        return "unknown";
    };
    let trimmed = raw.trim_start();
    if trimmed.starts_with("pub ") || trimmed.starts_with("pub(") {
        "public"
    } else {
        "private"
    }
}

fn count_tests(text: &str) -> usize {
    text.lines()
        .filter(|line| {
            let trimmed = line.trim_start();
            trimmed.starts_with("#[test") || trimmed.starts_with("#[tokio::test")
        })
        .count()
}

struct TestInference {
    paths: Vec<String>,
    basis: String,
    confidence: String,
}

fn infer_candidate_paths(file: &FileEntry, text: &str, index: &ContextIndex) -> TestInference {
    if file.kind == "source" {
        return TestInference {
            paths: vec![file.path.clone()],
            basis: "inline_test_in_source_file".into(),
            confidence: "high".into(),
        };
    }
    let lower_text = text.to_ascii_lowercase();
    let mut covered = Vec::new();
    let Some(source_root) = test_source_root_for(&file.path) else {
        return TestInference {
            paths: Vec::new(),
            basis: "no_same_package_source_root".into(),
            confidence: "none".into(),
        };
    };
    for source in index
        .files
        .iter()
        .filter(|candidate| candidate.kind == "source" && candidate.path.starts_with(&source_root))
    {
        let stem = Path::new(&source.path)
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if !stem.is_empty() && lower_text.contains(&stem) {
            covered.push(source.path.clone());
        }
    }
    covered.sort();
    covered.dedup();
    TestInference {
        paths: covered,
        basis: "same_package_test_text_reference_heuristic".into(),
        confidence: "low".into(),
    }
}

fn test_source_root_for(path: &str) -> Option<String> {
    let marker = "/tests/";
    if let Some(index) = path.find(marker) {
        let package_root = &path[..index];
        return Some(if package_root.is_empty() {
            "src/".into()
        } else {
            format!("{package_root}/src/")
        });
    }
    if path.starts_with("tests/") {
        return Some("src/".into());
    }
    None
}

fn infer_dependencies(
    text_by_path: &BTreeMap<String, String>,
    path_set: &BTreeSet<String>,
) -> Vec<Dependency> {
    let mut dependencies = Vec::new();
    let mut seen = BTreeSet::new();
    for (from_path, text) in text_by_path {
        for (line_index, line) in text.lines().enumerate() {
            for (target_path, kind, evidence) in local_targets(from_path, line, path_set) {
                if target_path == *from_path {
                    continue;
                }
                let key = format!("{from_path}->{target_path}:{kind}:{evidence}");
                if seen.insert(key) {
                    dependencies.push(Dependency {
                        from_path: from_path.clone(),
                        to_path: target_path,
                        kind,
                        evidence: format!("{}: {}", line_index + 1, evidence),
                    });
                }
            }
        }
    }
    dependencies
}

fn local_targets(
    from_path: &str,
    line: &str,
    path_set: &BTreeSet<String>,
) -> Vec<(String, String, String)> {
    let trimmed = line.trim_start();
    if trimmed.starts_with("//") {
        return Vec::new();
    }
    let mut targets = Vec::new();
    if let Some(module) = module_declaration(trimmed) {
        for candidate in module_candidates(from_path, &module) {
            if path_set.contains(&candidate) {
                targets.push((candidate, "module".into(), format!("mod {module}")));
                break;
            }
        }
    }
    for root in crate_roots(trimmed) {
        if let Some(candidate) = root_to_path(from_path, &root, path_set) {
            targets.push((candidate, "use".into(), root));
        }
    }
    targets
}

fn module_declaration(line: &str) -> Option<String> {
    let rest = line
        .strip_prefix("pub mod ")
        .or_else(|| line.strip_prefix("mod "))?;
    if !rest.contains(';') {
        return None;
    }
    let name = rest
        .split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_'))
        .next()
        .unwrap_or("");
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

fn module_candidates(from_path: &str, module: &str) -> Vec<String> {
    let base_dir = if from_path.ends_with("/mod.rs") {
        from_path.trim_end_matches("/mod.rs").to_string()
    } else {
        Path::new(from_path)
            .parent()
            .map(|path| path.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|| ".".into())
    };
    vec![
        format!("{base_dir}/{module}.rs")
            .trim_start_matches("./")
            .into(),
        format!("{base_dir}/{module}/mod.rs")
            .trim_start_matches("./")
            .into(),
    ]
}

fn crate_roots(line: &str) -> Vec<String> {
    let mut roots = BTreeSet::new();
    collect_after_prefix(line, "crate::", &mut roots);
    collect_after_prefix(line, "rayman::", &mut roots);
    collect_braced_use(line, "crate::{", &mut roots);
    collect_braced_use(line, "rayman::{", &mut roots);
    roots.into_iter().collect()
}

fn collect_after_prefix(line: &str, prefix: &str, roots: &mut BTreeSet<String>) {
    let mut rest = line;
    while let Some(index) = rest.find(prefix) {
        let tail = &rest[index + prefix.len()..];
        let ident = tail
            .split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_'))
            .next()
            .unwrap_or("");
        if !ident.is_empty() {
            roots.insert(ident.to_string());
        }
        rest = tail;
    }
}

fn collect_braced_use(line: &str, prefix: &str, roots: &mut BTreeSet<String>) {
    let Some(start) = line.find(prefix) else {
        return;
    };
    let tail = &line[start + prefix.len()..];
    let Some(body) = braced_body(tail) else {
        return;
    };
    for part in split_top_level_commas(body) {
        let ident = part
            .trim()
            .split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_'))
            .next()
            .unwrap_or("");
        if !ident.is_empty() && ident != "self" {
            roots.insert(ident.to_string());
        }
    }
}

fn braced_body(text: &str) -> Option<&str> {
    let mut depth = 0usize;
    for (index, ch) in text.char_indices() {
        match ch {
            '{' => depth += 1,
            '}' if depth == 0 => return Some(&text[..index]),
            '}' => depth -= 1,
            _ => {}
        }
    }
    None
}

fn split_top_level_commas(text: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0usize;
    let mut depth = 0usize;
    for (index, ch) in text.char_indices() {
        match ch {
            '{' => depth += 1,
            '}' if depth > 0 => depth -= 1,
            ',' if depth == 0 => {
                parts.push(&text[start..index]);
                start = index + ch.len_utf8();
            }
            _ => {}
        }
    }
    parts.push(&text[start..]);
    parts
}

fn root_to_path(from_path: &str, root: &str, path_set: &BTreeSet<String>) -> Option<String> {
    let src_root = source_root_for(from_path);
    let direct = format!("{src_root}/{root}.rs");
    if path_set.contains(&direct) {
        return Some(direct);
    }
    let module = format!("{src_root}/{root}/mod.rs");
    if path_set.contains(&module) {
        return Some(module);
    }
    None
}

fn source_root_for(path: &str) -> String {
    let parts: Vec<&str> = path.split('/').collect();
    if let Some(index) = parts.iter().rposition(|part| *part == "src") {
        parts[..=index].join("/")
    } else {
        "src".into()
    }
}

fn infer_risks(
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
        if file.lines >= 500 {
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
        if public_by_path.get(file.path.as_str()).copied().unwrap_or(0) > 0
            && !covered.contains(file.path.as_str())
            && file.path != "src/main.rs"
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

fn related_tests(map: &ProjectMap, path: &str) -> Vec<TestTarget> {
    map.tests
        .iter()
        .filter(|test| {
            test.path == path || test.candidate_paths.iter().any(|covered| covered == path)
        })
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write(path: &Path, body: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
    }

    #[test]
    fn map_builds_dependencies_and_impact_from_current_context() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(
            root.join("src/lib.rs").as_path(),
            "pub mod parser;\npub mod evaluator;\n",
        );
        write(
            root.join("src/parser.rs").as_path(),
            "pub fn parse() -> i32 { 1 }\n",
        );
        write(
            root.join("src/evaluator.rs").as_path(),
            "use crate::parser;\npub fn eval() -> i32 { parser::parse() }\n",
        );
        write(
            root.join("tests/evaluator_test.rs").as_path(),
            "use sample::evaluator;\n#[test]\nfn eval_works() { assert_eq!(1, 1); }\n",
        );
        context::refresh(root).unwrap();

        let map = build(root).unwrap();
        assert!(
            map.dependencies
                .iter()
                .any(|dependency| dependency.from_path == "src/evaluator.rs"
                    && dependency.to_path == "src/parser.rs")
        );
        let impact = impact_report(&map, "src/evaluator.rs").unwrap();
        assert!(
            impact
                .related_tests
                .iter()
                .any(|test| test.path == "tests/evaluator_test.rs")
        );
        assert!(
            impact
                .recommended_checks
                .iter()
                .any(|check| check == "cargo test --all")
        );
    }

    #[test]
    fn change_plan_groups_impacted_files_tests_and_checks() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(
            root.join("src/lib.rs").as_path(),
            "pub mod parser;\npub mod evaluator;\n",
        );
        write(
            root.join("src/parser.rs").as_path(),
            "pub fn parse() -> i32 { 1 }\n",
        );
        write(
            root.join("src/evaluator.rs").as_path(),
            "use crate::parser;\npub fn eval() -> i32 { parser::parse() }\n",
        );
        write(
            root.join("tests/evaluator_test.rs").as_path(),
            "use sample::evaluator;\n#[test]\nfn evaluator_works() {}\n",
        );
        context::refresh(root).unwrap();

        let map = build_readonly(root).unwrap();
        let plan = change_plan(&map, &["src/parser.rs".to_string()]).unwrap();

        assert!(plan.ready);
        assert_eq!(plan.review_priority, "normal");
        assert!(
            plan.impacted_files
                .iter()
                .any(|file| { file.path == "src/parser.rs" && file.role == "changed" })
        );
        assert!(
            plan.impacted_files
                .iter()
                .any(|file| { file.path == "src/evaluator.rs" && file.role == "dependent" })
        );
        assert!(
            plan.related_tests
                .iter()
                .any(|test| test.path == "tests/evaluator_test.rs")
        );
        assert!(
            plan.recommended_checks
                .iter()
                .any(|check| check == "cargo test --all")
        );
    }

    #[test]
    fn change_plan_blocks_broad_source_change_without_test_anchor() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(
            root.join("src/lib.rs").as_path(),
            "pub mod a;\npub mod b;\n",
        );
        write(root.join("src/a.rs").as_path(), "pub fn a() {}\n");
        write(root.join("src/b.rs").as_path(), "pub fn b() {}\n");
        context::refresh(root).unwrap();

        let map = build_readonly(root).unwrap();
        let plan = change_plan(
            &map,
            &[
                "src/lib.rs".to_string(),
                "src/a.rs".to_string(),
                "src/b.rs".to_string(),
            ],
        )
        .unwrap();

        assert!(!plan.ready);
        assert_eq!(plan.review_priority, "high");
        assert!(plan.blockers.iter().any(|blocker| {
            blocker.contains("3 source files") && blocker.contains("no same-package candidate test")
        }));
    }

    #[test]
    fn map_resolves_use_dependencies_inside_nested_crate_src_roots() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(
            root.join("crates/app/src/lib.rs").as_path(),
            "pub mod context;\npub mod map;\n",
        );
        write(
            root.join("crates/app/src/context.rs").as_path(),
            "pub fn refresh() {}\n",
        );
        write(
            root.join("crates/app/src/map.rs").as_path(),
            "use crate::context;\npub fn build() { context::refresh(); }\n",
        );
        context::refresh(root).unwrap();

        let map = build(root).unwrap();
        assert!(
            map.dependencies
                .iter()
                .any(|dependency| dependency.from_path == "crates/app/src/map.rs"
                    && dependency.to_path == "crates/app/src/context.rs"),
            "dependencies={:?}",
            map.dependencies
        );
    }

    #[test]
    fn quality_report_blocks_multi_source_project_without_tests() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root.join("src/lib.rs").as_path(), "pub mod parser;\n");
        write(root.join("src/parser.rs").as_path(), "pub fn parse() {}\n");
        write(
            root.join("src/evaluator.rs").as_path(),
            "pub fn eval() {}\n",
        );
        context::refresh(root).unwrap();

        let map = build_readonly(root).unwrap();
        let quality = quality_report(&map);

        assert!(!quality.ready);
        assert_eq!(quality.error_count, 1);
        assert!(quality.findings.iter().any(|finding| {
            finding.kind == "multi_source_project_without_tests" && finding.severity == "error"
        }));
    }

    #[test]
    fn quality_report_keeps_uncovered_public_api_as_warning() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root.join("src/lib.rs").as_path(), "pub fn api() {}\n");
        write(
            root.join("tests/api_test.rs").as_path(),
            "use sample::api;\n#[test]\nfn api_works() {}\n",
        );
        context::refresh(root).unwrap();

        let map = build_readonly(root).unwrap();
        let quality = quality_report(&map);

        assert!(quality.ready);
        assert_eq!(quality.error_count, 0);
        assert!(quality.warning_count <= quality.findings.len());
    }
}
