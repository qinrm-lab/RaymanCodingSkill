use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::context::{self, ContextIndex, FileEntry};
use crate::file_io::write_json;
use crate::state_paths;
use crate::timefmt::now_iso;

mod python;
mod quality;

pub use quality::{
    QualityConfig, QualityExemption, QualityFinding, QualityReport, load_quality_config,
    quality_report, quality_report_with_config,
};
use quality::{infer_risks, severity_rank};

const PROJECT_MAP_STATE_RELATIVE: &str = "context/project_map.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectMap {
    pub generated_at: String,
    pub workspace: String,
    #[serde(default)]
    pub topology_provenance: String,
    pub source_files: usize,
    pub test_files: usize,
    pub docs_files: usize,
    pub config_files: usize,
    #[serde(default)]
    pub script_files: usize,
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
    pub provenance: String,
    pub packages: Vec<PackageEntry>,
    pub package_dependencies: Vec<PackageDependency>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ImpactReport {
    pub changed_path: String,
    pub package: Option<String>,
    pub manifest_path: Option<String>,
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

#[derive(Debug, Clone, Default)]
struct WorkspaceInfo {
    member_patterns: Vec<String>,
    exclude_patterns: Vec<String>,
    path_dependencies: BTreeMap<String, String>,
    has_workspace_section: bool,
}

#[derive(Debug, Deserialize)]
struct CargoMetadataDocument {
    packages: Vec<CargoMetadataPackage>,
    workspace_members: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct CargoMetadataPackage {
    id: String,
    name: String,
    manifest_path: String,
    #[serde(default)]
    dependencies: Vec<CargoMetadataDependency>,
}

#[derive(Debug, Deserialize)]
struct CargoMetadataDependency {
    name: String,
    #[serde(default)]
    rename: Option<String>,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    kind: Option<String>,
}

pub fn build(root: &Path) -> Result<ProjectMap> {
    let map = build_readonly(root)?;
    write_json(&project_map_path(root, true)?, &map)?;
    Ok(map)
}

pub fn build_readonly(root: &Path) -> Result<ProjectMap> {
    // Map conclusions are used by standard/release planning, so a stat-only ready
    // result is insufficient. `verified_index` returns the same in-memory index
    // object whose full entries were compared; never validate and reopen the
    // cache because that permits an unverified replacement between the calls.
    let index = context::verified_index(root)?;
    build_from_index(root, &index)
}

pub fn summary(map: &ProjectMap) -> MapSummary {
    MapSummary {
        generated_at: map.generated_at.clone(),
        files: map.source_files
            + map.test_files
            + map.docs_files
            + map.config_files
            + map.script_files
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
        provenance: map.topology_provenance.clone(),
        packages: map.packages.clone(),
        package_dependencies: map.package_dependencies.clone(),
    }
}

/// A Cargo topology is authoritative only when it came from Cargo metadata.
/// Heuristic fallback remains useful for interactive map output, but
/// standard/release readiness must not rely on it.
pub fn topology_is_authoritative(root: &Path, map: &ProjectMap) -> bool {
    topology_provenance_is_authoritative(root, &map.topology_provenance)
}

/// 权威性只由 provenance 决定，不看"发现了哪些包"。
///
/// 按发现的包判断会两头出错：一个损坏到解析不出任何包的 Cargo.toml 会让仓库
/// 看起来"根本没有 Cargo 包"从而被判为权威；而根目录没有 manifest 只说明在根上
/// 跑 cargo metadata 会失败，不说明拓扑不可信。构建侧现在会对子目录 manifest
/// 逐个尝试 metadata，因此 provenance 已经能如实区分三种情形：真的没有 Cargo
/// 包（no_cargo_manifest）、拿到了权威（cargo_metadata）、以及尝试过但失败
/// （heuristic_fallback: …，必须 fail-closed）。
fn topology_provenance_is_authoritative(root: &Path, provenance: &str) -> bool {
    let _ = root;
    matches!(provenance, "cargo_metadata" | "no_cargo_manifest")
}

pub fn impact_report(map: &ProjectMap, path: &str) -> Result<ImpactReport> {
    let path = normalize_query_path(path);
    let directory_prefix = if path == "." {
        String::new()
    } else {
        format!("{}/", path.trim_end_matches('/'))
    };
    let has_descendants = (!directory_prefix.is_empty()
        && (map
            .modules
            .iter()
            .any(|module| module.path.starts_with(&directory_prefix))
            || map
                .tests
                .iter()
                .any(|test| test.path.starts_with(&directory_prefix))
            || map
                .packages
                .iter()
                .any(|package| package.manifest_path.starts_with(&directory_prefix))))
        || (path == "." && (!map.modules.is_empty() || !map.tests.is_empty()));
    if has_descendants {
        bail!(
            "map impact accepts one file path, but `{path}` is an indexed directory; use `map plan <files...>` with concrete files"
        );
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
    if is_evals_dependency_policy_path(&path) {
        recommended_checks.push(
            "cargo deny --manifest-path evals/Cargo.toml check --config evals/deny.toml".into(),
        );
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
        manifest_path: package_entry.map(|package| package.manifest_path.clone()),
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
    let mut supported_source_changed_count = 0usize;
    let mut has_package_test_anchor = false;

    for path in &changed_paths {
        let module = module_by_path.get(path.as_str());
        if module.is_some_and(|module| module.kind == "source") || is_source_path(path) {
            source_changed_count += 1;
            if path_has_supported_package(map, path) {
                supported_source_changed_count += 1;
            }
        }
        if module.is_none() {
            warnings.push(format!(
                "{path} is not in the current index; treating it as an add/delete candidate and using nearest package checks"
            ));
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
        let message = format!(
            "{source_changed_count} source files are in scope but no same-package candidate test target or indexed package test anchor was inferred"
        );
        // The blocking threshold applies to the files the modeled ecosystems actually collect.
        // An unrelated manifest elsewhere in the tree (a `fixtures/tiny/Cargo.toml` inside a
        // JavaScript repository) is not evidence that these changed files have a package-level
        // test anchor, so it must not promote them from advisory to blocking.
        if supported_source_changed_count >= 3 {
            blockers.push(message);
        } else {
            warnings.push(format!(
                "{message}; no Cargo or pyproject package detected for the changed source files, so this is advisory only — verify test coverage manually"
            ));
        }
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

fn is_source_path(path: &str) -> bool {
    matches!(
        Path::new(path)
            .extension()
            .and_then(|extension| extension.to_str()),
        Some(
            "rs" | "js"
                | "jsx"
                | "ts"
                | "tsx"
                | "py"
                | "go"
                | "java"
                | "cs"
                | "cpp"
                | "c"
                | "h"
                | "hpp"
                | "rb"
                | "php"
                | "swift"
                | "kt"
                | "scala"
        )
    )
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

/// Only Cargo and pyproject packages have language-aware test conventions, and only for the
/// files their own tooling collects. A `.js` file that merely sits in a repository containing
/// some unrelated `Cargo.toml` has no modeled test anchor, so its missing-test conclusion must
/// stay advisory exactly like any other unsupported ecosystem.
fn path_has_supported_package(map: &ProjectMap, path: &str) -> bool {
    map.packages.iter().any(|package| {
        path_is_under_package(path, &package.root_path) && package_collects_path(package, path)
    })
}

/// 该包的工具链是否能收集自己的源文件（即它是受支持生态）。
fn package_collects_own_sources(package: &PackageEntry) -> bool {
    package.manifest_path.ends_with("Cargo.toml")
        || package.manifest_path.ends_with("pyproject.toml")
}

fn package_collects_path(package: &PackageEntry, path: &str) -> bool {
    if package.manifest_path.ends_with("Cargo.toml") {
        path.ends_with(".rs")
    } else if package.manifest_path.ends_with("pyproject.toml") {
        path.ends_with(".py")
    } else {
        false
    }
}

/// Does this package have a local validation anchor for *its own* sources?
///
/// The discovered-test fallback used to accept any test file under the package
/// root regardless of language, so a `tests/*.py` inside a Cargo package
/// counted as an anchor for Rust sources that `cargo test` would never touch.
/// Both halves now agree with `package_collects_path`.
fn package_has_test_anchor(map: &ProjectMap, package: &PackageEntry) -> bool {
    package.test_files > 0
        || map.tests.iter().any(|test| {
            test.test_count > 0
                && path_is_under_package(&test.path, &package.root_path)
                && package_collects_path(package, &test.path)
        })
}

fn is_evals_dependency_policy_path(path: &str) -> bool {
    matches!(
        path,
        "evals/Cargo.toml" | "evals/Cargo.lock" | "evals/deny.toml"
    )
}

fn project_map_path(root: &Path, create_parents: bool) -> Result<PathBuf> {
    state_paths::managed_state_file(root, Path::new(PROJECT_MAP_STATE_RELATIVE), create_parents)
}

/// Cargo 本身是 manifest 的权威解释器。能运行时绝不从逐行字符串启发式推断
/// workspace/package/path-dependency；只在非 Cargo 或 metadata 不可用时降级并标记来源。
/// 每个仓库最多尝试多少个子目录 manifest，避免 fixture 众多的仓库触发子进程风暴。
const MAX_NESTED_METADATA_MANIFESTS: usize = 8;

/// 根目录没有 Cargo.toml 时，对索引到的 Cargo manifest 逐个尝试 `cargo metadata`。
///
/// "根上没有 manifest" 只说明在根上跑 cargo metadata 一定失败，不说明这个仓库没有
/// Cargo 包。crate 全在子目录的多语言 monorepo 此前因此永远拿不到权威拓扑，而权威性
/// 又是 standard/release 的硬前提——结果是整类仓库被永久阻塞且没有任何解除路径。
/// 只有当每个被发现的 manifest 都成功解析时才算权威；任何一个失败就退回启发式，
/// 因为部分权威的拓扑仍然会漏掉包与依赖边。
fn nested_cargo_metadata_topology(
    root: &Path,
    index: &ContextIndex,
) -> Option<(Vec<PackageEntry>, Vec<PackageDependency>)> {
    let mut manifests: Vec<&str> = index
        .files
        .iter()
        .map(|file| file.path.as_str())
        .filter(|path| path.ends_with("Cargo.toml"))
        .collect();
    manifests.sort_unstable();
    if manifests.is_empty() || manifests.len() > MAX_NESTED_METADATA_MANIFESTS {
        return None;
    }

    let mut packages: Vec<PackageEntry> = Vec::new();
    let mut dependencies: Vec<PackageDependency> = Vec::new();
    for manifest in manifests {
        let (found, deps) = cargo_metadata_at(root, index, Some(manifest)).ok()?;
        packages.extend(found);
        dependencies.extend(deps);
    }
    packages.sort_by(|left, right| left.manifest_path.cmp(&right.manifest_path));
    packages.dedup_by(|left, right| left.manifest_path == right.manifest_path);
    dependencies.sort_by(|left, right| {
        (
            &left.from_package,
            &left.to_package,
            &left.kind,
            &left.manifest_path,
        )
            .cmp(&(
                &right.from_package,
                &right.to_package,
                &right.kind,
                &right.manifest_path,
            ))
    });
    dependencies.dedup_by(|left, right| {
        (
            &left.from_package,
            &left.to_package,
            &left.kind,
            &left.manifest_path,
        ) == (
            &right.from_package,
            &right.to_package,
            &right.kind,
            &right.manifest_path,
        )
    });
    Some((packages, dependencies))
}

fn cargo_metadata_topology(
    root: &Path,
    index: &ContextIndex,
) -> Result<(Vec<PackageEntry>, Vec<PackageDependency>)> {
    cargo_metadata_at(root, index, None)
}

/// Marker inside a heuristic-fallback provenance meaning "the tool could not be
/// run at all", as opposed to "the tool ran and the topology is untrustworthy".
pub const TOPOLOGY_TOOL_UNAVAILABLE: &str = "cargo_unavailable";

/// Was this provenance produced because cargo itself could not be executed?
pub fn topology_blocked_by_missing_cargo(provenance: &str) -> bool {
    provenance.contains(TOPOLOGY_TOOL_UNAVAILABLE)
}

fn cargo_metadata_at(
    root: &Path,
    index: &ContextIndex,
    manifest: Option<&str>,
) -> Result<(Vec<PackageEntry>, Vec<PackageDependency>)> {
    let mut command = Command::new("cargo");
    command.args(["metadata", "--no-deps", "--format-version", "1"]);
    if let Some(manifest) = manifest {
        command.args(["--manifest-path", manifest]);
    }
    // "cargo 跑不起来"与"拓扑不可信"会产出同一个 BLOCKED，但含义完全不同：前者
    // 装上 cargo 就能解除，后者要修 manifest。诊断里必须分得开，否则用户看到的是
    // 一条无从下手的"拓扑未获权威确认"。
    // Tag the "cargo is not reachable" case so callers can separate an
    // environment boundary from a damaged manifest. Both fail closed, but only
    // one of them is fixed by the operator's PATH rather than by the repository.
    let output = command.current_dir(root).output().map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            anyhow::anyhow!("{TOPOLOGY_TOOL_UNAVAILABLE}: cargo 不在本进程 PATH 中")
        } else {
            anyhow::Error::new(error).context("无法执行 cargo metadata（cargo 是否在 PATH 中？）")
        }
    })?;
    if !output.status.success() {
        bail!(
            "cargo metadata 失败: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let document: CargoMetadataDocument =
        serde_json::from_slice(&output.stdout).context("无法解析 cargo metadata JSON")?;
    let workspace_members: BTreeSet<&str> = document
        .workspace_members
        .iter()
        .map(String::as_str)
        .collect();
    let mut packages = document
        .packages
        .iter()
        .map(|package| {
            let manifest = Path::new(&package.manifest_path);
            let relative_manifest = manifest
                .strip_prefix(root)
                .unwrap_or(manifest)
                .to_string_lossy()
                .replace('\\', "/");
            PackageEntry {
                name: package.name.clone(),
                root_path: manifest_root_path(&relative_manifest),
                manifest_path: relative_manifest,
                workspace_member: workspace_members.contains(package.id.as_str()),
                source_files: 0,
                test_files: 0,
            }
        })
        .collect::<Vec<_>>();
    // Metadata owns workspace membership/dependencies. Add indexed nested manifests that
    // Cargo deliberately excludes (fixtures, excluded tools) so their impact still gets a
    // manifest-path test command rather than incorrectly borrowing an ancestor package.
    let heuristic_workspace = read_workspace_info(root, index)?;
    for candidate in discover_packages(root, index, &heuristic_workspace)? {
        if !packages
            .iter()
            .any(|package| package.manifest_path == candidate.manifest_path)
        {
            packages.push(candidate);
        }
    }
    packages.sort_by(|left, right| left.root_path.cmp(&right.root_path));
    count_package_files(&mut packages, index);

    let by_manifest: BTreeMap<String, &PackageEntry> = packages
        .iter()
        .map(|package| (package.manifest_path.clone(), package))
        .collect();
    let mut dependencies = Vec::new();
    for package in &document.packages {
        let manifest = Path::new(&package.manifest_path);
        let relative_manifest = manifest
            .strip_prefix(root)
            .unwrap_or(manifest)
            .to_string_lossy()
            .replace('\\', "/");
        let Some(from) = by_manifest.get(&relative_manifest) else {
            continue;
        };
        for dependency in &package.dependencies {
            let Some(path) = dependency.path.as_deref() else {
                continue;
            };
            let dependency_root = Path::new(path)
                .strip_prefix(root)
                .unwrap_or_else(|_| Path::new(path))
                .to_string_lossy()
                .replace('\\', "/");
            let Some(to) = packages.iter().find(|candidate| {
                candidate.root_path == dependency_root
                    || candidate.manifest_path
                        == format!("{}/Cargo.toml", dependency_root.trim_end_matches('/'))
            }) else {
                continue;
            };
            dependencies.push(PackageDependency {
                from_package: from.name.clone(),
                from_root_path: from.root_path.clone(),
                to_package: to.name.clone(),
                to_root_path: to.root_path.clone(),
                dependency_name: dependency
                    .rename
                    .clone()
                    .unwrap_or_else(|| dependency.name.clone()),
                kind: dependency.kind.clone().unwrap_or_else(|| "normal".into()),
                manifest_path: from.manifest_path.clone(),
                evidence: "cargo metadata --no-deps".into(),
            });
        }
    }
    dependencies.sort_by(|left, right| {
        left.from_root_path
            .cmp(&right.from_root_path)
            .then_with(|| left.to_root_path.cmp(&right.to_root_path))
            .then_with(|| left.dependency_name.cmp(&right.dependency_name))
    });
    dependencies.dedup_by(|left, right| {
        left.from_root_path == right.from_root_path
            && left.to_root_path == right.to_root_path
            && left.dependency_name == right.dependency_name
    });
    Ok((packages, dependencies))
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
            let bytes = context::read_verified_file(root, file)
                .with_context(|| format!("项目地图读取失败: {}", file.path))?;
            text_by_path.insert(
                file.path.clone(),
                String::from_utf8_lossy(&bytes).into_owned(),
            );
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

        let test_count = count_tests(file, text);
        if test_count > 0 {
            let inference = infer_candidate_paths(file, text, index);
            tests.push(TestTarget {
                path: file.path.clone(),
                kind: if file.kind == "test" {
                    "integration".into()
                } else {
                    "inline".into()
                },
                test_count,
                candidate_paths: inference.paths,
                basis: inference.basis,
                confidence: inference.confidence,
            });
        }
    }

    let (mut packages, package_dependencies, topology_provenance) = if root
        .join("Cargo.toml")
        .is_file()
    {
        match cargo_metadata_topology(root, index) {
            Ok((packages, dependencies)) => (packages, dependencies, "cargo_metadata".to_string()),
            Err(error) => {
                // Do not dress the fallback up as Cargo authority. It remains useful for
                // damaged/nonstandard workspaces, but callers can see the weaker provenance.
                let workspace = read_workspace_info(root, index)?;
                let packages = discover_packages(root, index, &workspace)?;
                let dependencies = infer_package_dependencies(root, index, &packages, &workspace)?;
                (
                    packages,
                    dependencies,
                    format!("heuristic_fallback: {error:#}"),
                )
            }
        }
    } else if let Some((packages, dependencies)) = nested_cargo_metadata_topology(root, index) {
        (packages, dependencies, "cargo_metadata".to_string())
    } else if index
        .files
        .iter()
        .any(|file| file.path.ends_with("Cargo.toml"))
    {
        // 索引里有 Cargo manifest 但没能从中取得权威拓扑。必须与"这个仓库根本
        // 没有 Cargo 包"区分开：后者可以照常 ready，前者只能 fail-closed，否则
        // 一个损坏到解析不出任何包的 Cargo.toml 会让仓库看起来无 Cargo 包而蒙混过关。
        let workspace = read_workspace_info(root, index)?;
        let packages = discover_packages(root, index, &workspace)?;
        let dependencies = infer_package_dependencies(root, index, &packages, &workspace)?;
        (
            packages,
            dependencies,
            "heuristic_fallback: nested cargo metadata unavailable".to_string(),
        )
    } else {
        let workspace = read_workspace_info(root, index)?;
        let packages = discover_packages(root, index, &workspace)?;
        let dependencies = infer_package_dependencies(root, index, &packages, &workspace)?;
        (packages, dependencies, "no_cargo_manifest".to_string())
    };
    packages.extend(python::discover_packages(root, index)?);
    // 本地 crate 名来自实际发现的 package，而不是硬编码本工具自己的名字。
    let local_crates: BTreeSet<String> = packages
        .iter()
        .filter(|package| package.manifest_path.ends_with("Cargo.toml"))
        .map(|package| package.name.replace('-', "_"))
        .collect();
    let dependencies = infer_dependencies(&text_by_path, &path_set, &local_crates);
    let risks = infer_risks(index, &symbols, &dependencies, &tests);
    let (source_files, test_files, docs_files, config_files, script_files, asset_files) =
        count_kinds(index);

    Ok(ProjectMap {
        generated_at: now_iso(),
        workspace: index.workspace.clone(),
        topology_provenance,
        source_files,
        test_files,
        docs_files,
        config_files,
        script_files,
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

fn count_kinds(index: &ContextIndex) -> (usize, usize, usize, usize, usize, usize) {
    let count = |kind: &str| index.files.iter().filter(|file| file.kind == kind).count();
    (
        count("source"),
        count("test"),
        count("docs"),
        count("config"),
        count("script"),
        count("asset"),
    )
}

/// Does the root Cargo workspace explicitly exclude the package owning `path`?
///
/// `exclude = ["evals"]` means `cargo test --workspace` never compiles a line
/// of it, yet a workspace-wide command used to be accepted as covering changes
/// there — a receipt claiming validation of code the command provably never
/// built. Parsing lives here, next to the other workspace-manifest rules, so
/// the validation layer cannot drift from it.
pub(crate) fn path_is_excluded_from_root_cargo_workspace(root: &Path, path: &str) -> bool {
    let Ok(text) = std::fs::read_to_string(root.join("Cargo.toml")) else {
        return false;
    };
    let normalized = path.replace('\\', "/");
    parse_workspace_array(&text, "exclude")
        .iter()
        .any(|pattern| path_matches_workspace_exclude_pattern(&normalized, pattern))
}

fn read_workspace_info(root: &Path, index: &ContextIndex) -> Result<WorkspaceInfo> {
    let Some(entry) = index.files.iter().find(|file| file.path == "Cargo.toml") else {
        return Ok(WorkspaceInfo::default());
    };
    // Manifests decode exactly like indexed source files: one non-UTF-8 manifest anywhere in
    // the tree (a fixture with a GBK comment, even one the workspace excludes) must not bail
    // the whole map/check/prepare/finish pipeline and leave the workspace unable to be READY.
    let text = String::from_utf8_lossy(&context::read_verified_file(root, entry)?).into_owned();
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
        let text = String::from_utf8_lossy(&context::read_verified_file(root, file)?).into_owned();
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
    count_package_files(&mut packages, index);
    Ok(packages)
}

/// Attribute indexed source/test files to their owning package.
///
/// The cargo-metadata path and this heuristic path each used to carry their own
/// copy of this loop, so a fix applied to one silently left the other counting
/// differently. Counting *every* language meant one `tests/*.py` inside a Cargo
/// package satisfied `package_has_test_anchor` and silenced
/// `multi_source_project_without_tests` — the only blocking quality error — on
/// a package with no Rust tests at all.
fn count_package_files(packages: &mut [PackageEntry], index: &ContextIndex) {
    for package in packages.iter_mut() {
        package.source_files = 0;
        package.test_files = 0;
    }
    for file in &index.files {
        if !matches!(file.kind.as_str(), "source" | "test") {
            continue;
        }
        let Some(position) = package_index_for_path(packages, &file.path) else {
            continue;
        };
        if !package_collects_path(&packages[position], &file.path) {
            continue;
        }
        if file.kind == "source" {
            packages[position].source_files += 1;
        } else {
            packages[position].test_files += 1;
        }
    }
}

fn infer_package_dependencies(
    root: &Path,
    index: &ContextIndex,
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
        let entry = index
            .files
            .iter()
            .find(|file| file.path == package.manifest_path)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Cargo manifest 不在已验证上下文索引中: {}",
                    package.manifest_path
                )
            })?;
        let text = String::from_utf8_lossy(&context::read_verified_file(root, entry)?).into_owned();
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
    let python_path = path.ends_with(".py") || path.ends_with("pyproject.toml");
    map.packages
        .iter()
        .filter(|package| path_is_under_package(path, &package.root_path))
        .max_by_key(|package| {
            let preferred = python_path == package.manifest_path.ends_with("pyproject.toml");
            (preferred, package.root_path.len())
        })
}

fn package_test_command(map: &ProjectMap, package: &PackageEntry) -> String {
    if package.manifest_path.ends_with("pyproject.toml") {
        return if package.root_path == "." {
            "python -m pytest".into()
        } else {
            format!("python -m pytest {}", package.root_path)
        };
    }
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

fn count_tests(file: &FileEntry, text: &str) -> usize {
    if file.path.ends_with(".py") {
        // Python has no Rust-style inline test module. pytest only collects modules it
        // recognises as tests, so a production module that happens to define `TestResult`,
        // `Testimonial`, or a `test_` helper method is never executed by the test run. Counting
        // those names here used to mint a self-covering `inline_test_in_source_file` anchor and
        // silently unblock broad changes that have no test coverage at all.
        if file.kind != "test" {
            return 0;
        }
        return text
            .lines()
            .filter(|line| {
                let trimmed = line.trim_start();
                trimmed.starts_with("def test_")
                    || trimmed.starts_with("async def test_")
                    || trimmed.starts_with("class Test")
            })
            .count();
    }
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
    if file.path.ends_with(".py") && file.kind == "test" {
        return python::infer_test_candidates(file, text, index);
    }
    // Self-coverage is only real for Rust's `#[test]` modules, which the production file
    // compiles and `cargo test` runs. No other modeled language collects tests out of a
    // production file, so none of them may claim to cover themselves.
    if file.kind == "source" && file.path.ends_with(".rs") {
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
    local_crates: &BTreeSet<String>,
) -> Vec<Dependency> {
    let mut dependencies = Vec::new();
    let mut seen = BTreeSet::new();
    for (from_path, text) in text_by_path {
        for (line_index, line) in text.lines().enumerate() {
            let targets = if from_path.ends_with(".py") {
                python::local_targets(from_path, line, path_set)
            } else {
                local_targets(from_path, line, path_set, local_crates)
            };
            for (target_path, kind, evidence) in targets {
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
    local_crates: &BTreeSet<String>,
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
    for root in crate_roots(trimmed, local_crates) {
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

fn crate_roots(line: &str, local_crates: &BTreeSet<String>) -> Vec<String> {
    let mut roots = BTreeSet::new();
    collect_after_prefix(line, "crate::", &mut roots);
    collect_braced_use(line, "crate::{", &mut roots);
    // 工作区内各 package 的 crate 名：集成测试/bin 里 `use <libname>::…` 也是本地依赖边。
    for crate_name in local_crates {
        collect_after_prefix(line, &format!("{crate_name}::"), &mut roots);
        collect_braced_use(line, &format!("{crate_name}::{{"), &mut roots);
    }
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
mod tests;
