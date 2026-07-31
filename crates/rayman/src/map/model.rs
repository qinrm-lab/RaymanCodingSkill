use serde::{Deserialize, Serialize};

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
