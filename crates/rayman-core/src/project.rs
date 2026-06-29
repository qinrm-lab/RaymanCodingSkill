use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use walkdir::WalkDir;

use crate::assets::{AssetRetirementManager, AssetRetirementReport};
use crate::quality::{QualityManager, QualityRegressionItem};
use crate::temp::TempManager;
use crate::{display_path, ensure_within, now_iso, sha256_file, workspace_root, write_text};

const IGNORED_DIRS: &[&str] = &[
    ".git",
    ".RaymanCodingSkill",
    "target",
    ".tmp",
    "node_modules",
    "dist",
    "build",
    "logs",
    "__pycache__",
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Evidence {
    pub path: String,
    pub line: Option<usize>,
    pub sha256: Option<String>,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdapterReport {
    pub language: String,
    pub status: String,
    pub confidence: String,
    pub project_roots: Vec<String>,
    pub manifests: Vec<Evidence>,
    pub toolchain: String,
    pub degraded_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectSymbol {
    pub language: String,
    pub name: String,
    pub kind: String,
    pub visibility: String,
    pub path: String,
    pub line: usize,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DependencyEdge {
    pub language: String,
    pub from: String,
    pub to: String,
    pub kind: String,
    pub evidence: Evidence,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TestTarget {
    pub language: String,
    pub name: String,
    pub path: Option<String>,
    pub command: String,
    pub confidence: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LanguageIndex {
    pub language: String,
    pub roots: Vec<String>,
    pub entry_points: Vec<Evidence>,
    pub symbols: Vec<ProjectSymbol>,
    pub dependency_edges: Vec<DependencyEdge>,
    pub test_targets: Vec<TestTarget>,
    pub verification_commands: Vec<String>,
    pub confidence: String,
    pub stale_reasons: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectIndex {
    pub workspace_path: String,
    pub generated_at: String,
    pub project_adapters: Vec<AdapterReport>,
    pub language_indexes: Vec<LanguageIndex>,
    pub dependency_graph: Vec<DependencyEdge>,
    pub test_selection: Vec<TestTarget>,
    pub confidence: String,
    pub stale_reasons: Vec<String>,
    #[serde(default)]
    pub asset_retirement: AssetRetirementReport,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ImpactReport {
    pub workspace_path: String,
    pub generated_at: String,
    pub directly_changed_files: Vec<Evidence>,
    pub affected_modules: Vec<Evidence>,
    pub affected_public_api: Vec<ProjectSymbol>,
    pub likely_tests: Vec<TestTarget>,
    pub broad_gates: Vec<String>,
    pub docs_config_risk: Vec<String>,
    pub confidence: String,
    pub evidence: Vec<Evidence>,
    pub project_adapters: Vec<AdapterReport>,
    pub stale_reasons: Vec<String>,
    #[serde(default)]
    pub asset_retirement: AssetRetirementReport,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RegressionPlan {
    pub workspace_path: String,
    pub generated_at: String,
    pub risk_level: String,
    pub risk_reasons: Vec<String>,
    pub recommended_focus: Vec<String>,
    pub minimal_tests: Vec<String>,
    pub language_gates: Vec<String>,
    pub broad_gates: Vec<String>,
    pub checklist: Vec<String>,
    pub quality_patterns: Vec<QualityRegressionItem>,
    pub impact: ImpactReport,
    #[serde(default)]
    pub asset_retirement: AssetRetirementReport,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BenchmarkReport {
    pub status: String,
    pub generated_at: String,
    pub cases: Vec<Value>,
}

#[derive(Debug, Clone)]
pub struct ProjectAnalyzer {
    workspace: PathBuf,
}

#[derive(Debug, Clone)]
struct SourceFile {
    path: String,
    absolute: PathBuf,
    language: String,
    sha256: String,
}

impl ProjectAnalyzer {
    pub fn new(workspace: impl Into<PathBuf>) -> Result<Self> {
        let workspace = workspace
            .into()
            .canonicalize()
            .context("无法解析工作区路径")?;
        Ok(Self { workspace })
    }

    pub fn detect(&self) -> Result<ProjectIndex> {
        self.index()
    }

    pub fn index(&self) -> Result<ProjectIndex> {
        let asset_retirement = AssetRetirementManager::new(self.workspace.clone())?.status()?;
        let non_current_paths = asset_retirement.non_current_paths();
        let files = self
            .source_files()?
            .into_iter()
            .filter(|file| !non_current_paths.contains(&file.path))
            .collect::<Vec<_>>();
        let mut adapters = self.adapter_reports(&files)?;
        for adapter in &mut adapters {
            adapter
                .manifests
                .retain(|manifest| !non_current_paths.contains(&manifest.path));
        }
        let language_indexes = self.language_indexes(&files, &adapters)?;
        let dependency_graph = language_indexes
            .iter()
            .flat_map(|index| index.dependency_edges.clone())
            .collect::<Vec<_>>();
        let test_selection = language_indexes
            .iter()
            .flat_map(|index| index.test_targets.clone())
            .collect::<Vec<_>>();
        let stale_reasons = language_indexes
            .iter()
            .flat_map(|index| index.stale_reasons.clone())
            .collect::<Vec<_>>();
        let confidence = if adapters
            .iter()
            .any(|adapter| adapter.confidence == "degraded")
        {
            "degraded"
        } else if adapters.is_empty() {
            "none"
        } else {
            "high"
        }
        .to_string();
        Ok(ProjectIndex {
            workspace_path: display_path(&self.workspace),
            generated_at: now_iso(),
            project_adapters: adapters,
            language_indexes,
            dependency_graph,
            test_selection,
            confidence,
            stale_reasons,
            asset_retirement,
        })
    }

    pub fn write_index(&self) -> Result<ProjectIndex> {
        let index = self.index()?;
        let path = self
            .workspace
            .join(".RaymanCodingSkill")
            .join("project")
            .join("index.json");
        write_text(&path, &serde_json::to_string_pretty(&index)?)?;
        Ok(index)
    }

    pub fn impact(&self, paths: &[PathBuf]) -> Result<ImpactReport> {
        let index = self.index()?;
        let changed = normalize_changed_paths(&self.workspace, paths)?;
        let changed_set = changed.iter().cloned().collect::<BTreeSet<_>>();
        let directly_changed_files = changed
            .iter()
            .map(|path| evidence_for_path(&self.workspace, path, "directly changed input"))
            .collect::<Result<Vec<_>>>()?;
        let mut affected_modules = BTreeMap::<String, Evidence>::new();
        let mut affected_public_api = Vec::new();
        let mut likely_tests = BTreeMap::<String, TestTarget>::new();
        let mut broad_gates = BTreeSet::<String>::new();
        let mut docs_config_risk = BTreeSet::<String>::new();
        let mut evidence = directly_changed_files.clone();

        for language in &index.language_indexes {
            for command in &language.verification_commands {
                broad_gates.insert(command.clone());
            }
            for symbol in &language.symbols {
                if changed_set.contains(&symbol.path)
                    || same_stem_changed(&changed_set, &symbol.path)
                {
                    affected_modules.insert(
                        symbol.path.clone(),
                        Evidence {
                            path: symbol.path.clone(),
                            line: Some(symbol.line),
                            sha256: Some(symbol.sha256.clone()),
                            reason: format!("{} symbol touched by changed file", language.language),
                        },
                    );
                    if symbol.visibility == "public" {
                        affected_public_api.push(symbol.clone());
                    }
                }
            }
            for edge in &language.dependency_edges {
                if changed_set.contains(&edge.to) {
                    affected_modules.insert(
                        edge.from.clone(),
                        Evidence {
                            path: edge.from.clone(),
                            line: edge.evidence.line,
                            sha256: edge.evidence.sha256.clone(),
                            reason: format!("depends on changed {}", edge.to),
                        },
                    );
                    evidence.push(edge.evidence.clone());
                }
            }
            for test in &language.test_targets {
                if let Some(path) = &test.path {
                    if changed_set.contains(path)
                        || changed_set
                            .iter()
                            .any(|changed| related_test(changed, path))
                    {
                        likely_tests.insert(test.command.clone(), test.clone());
                    }
                } else if !changed_set.is_empty() {
                    likely_tests.insert(test.command.clone(), test.clone());
                }
            }
        }

        for path in &changed_set {
            if is_doc_path(path) {
                docs_config_risk.insert(format!("documentation changed: {path}"));
            }
            if is_config_path(path) {
                docs_config_risk.insert(format!("configuration changed: {path}; run broad gates"));
            }
            if path.contains("README") || path.contains("docs/") {
                docs_config_risk
                    .insert("docs drift risk: verify docs match current behavior".into());
            }
            if index.asset_retirement.non_current_paths().contains(path) {
                docs_config_risk.insert(format!(
                    "obsolete asset touched: {path}; retire or exempt it before success"
                ));
            }
        }
        if index.asset_retirement.has_blockers() {
            docs_config_risk.insert(
                "obsolete asset retirement blockers present; run rayman assets status".into(),
            );
        }
        if likely_tests.is_empty() {
            for test in &index.test_selection {
                likely_tests.insert(test.command.clone(), test.clone());
            }
        }
        let confidence = if index.confidence == "degraded" || changed_set.is_empty() {
            "degraded"
        } else {
            "high"
        }
        .to_string();
        Ok(ImpactReport {
            workspace_path: index.workspace_path.clone(),
            generated_at: now_iso(),
            directly_changed_files,
            affected_modules: affected_modules.into_values().collect(),
            affected_public_api,
            likely_tests: likely_tests.into_values().collect(),
            broad_gates: broad_gates.into_iter().collect(),
            docs_config_risk: docs_config_risk.into_iter().collect(),
            confidence,
            evidence,
            project_adapters: index.project_adapters,
            stale_reasons: index.stale_reasons,
            asset_retirement: index.asset_retirement,
        })
    }

    pub fn regression_plan(&self, paths: &[PathBuf]) -> Result<RegressionPlan> {
        let impact = self.impact(paths)?;
        let mut minimal_tests = impact
            .likely_tests
            .iter()
            .map(|test| test.command.clone())
            .collect::<BTreeSet<_>>();
        let language_gates = impact.broad_gates.iter().cloned().collect::<BTreeSet<_>>();
        if minimal_tests.is_empty() {
            minimal_tests.extend(language_gates.iter().cloned());
        }
        let broad_gates = [
            "cargo fmt --check",
            "cargo clippy --all-targets -- -D warnings",
            "cargo test --all",
            "rayman audit",
        ]
        .iter()
        .map(|value| (*value).to_string())
        .collect::<Vec<_>>();
        let mut checklist = vec![
            "Review impact evidence and reread current source ranges before editing.".into(),
            "Run minimal language tests first, then required broad gates.".into(),
            "Check public API and documentation drift when affected_public_api is non-empty."
                .into(),
            "Do not close a goal as success until must requirements have validation evidence."
                .into(),
        ];
        if impact.asset_retirement.has_blockers() {
            checklist.push(
                "Resolve obsolete asset retirement blockers with rayman assets status before success."
                    .into(),
            );
        }
        let quality_patterns = QualityManager::new(&self.workspace)?
            .regression_items_for_text(&regression_corpus(&impact))?;
        let (risk_level, risk_reasons, recommended_focus) =
            regression_risk_recommendations(&impact, &quality_patterns);
        let asset_retirement = impact.asset_retirement.clone();
        Ok(RegressionPlan {
            workspace_path: impact.workspace_path.clone(),
            generated_at: now_iso(),
            risk_level,
            risk_reasons,
            recommended_focus,
            minimal_tests: minimal_tests.into_iter().collect(),
            language_gates: language_gates.into_iter().collect(),
            broad_gates,
            checklist,
            quality_patterns,
            impact,
            asset_retirement,
        })
    }

    fn source_files(&self) -> Result<Vec<SourceFile>> {
        let mut files = Vec::new();
        for entry in WalkDir::new(&self.workspace)
            .into_iter()
            .filter_entry(|entry| !ignored(entry.path(), &self.workspace))
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_type().is_file())
        {
            let path = entry.path();
            if let Some(language) = language_for_file(path) {
                let relative = relative_path(&self.workspace, path);
                files.push(SourceFile {
                    path: relative,
                    absolute: path.to_path_buf(),
                    language,
                    sha256: sha256_file(path)?,
                });
            }
        }
        files.sort_by(|left, right| left.path.cmp(&right.path));
        Ok(files)
    }

    fn adapter_reports(&self, files: &[SourceFile]) -> Result<Vec<AdapterReport>> {
        let mut adapters = Vec::new();
        for language in ["rust", "javascript", "typescript", "python", "csharp", "go"] {
            let manifests = manifest_evidence(&self.workspace, language)?;
            let language_files = files
                .iter()
                .filter(|file| file.language == language || language_pair(&file.language, language))
                .collect::<Vec<_>>();
            if manifests.is_empty() && language_files.is_empty() {
                continue;
            }
            let tool = tool_for_language(language);
            let has_tool = command_available(tool);
            let confidence = if has_tool || language == "javascript" || language == "typescript" {
                "high"
            } else {
                "degraded"
            };
            let roots = roots_for(&self.workspace, &manifests, &language_files);
            adapters.push(AdapterReport {
                language: language.into(),
                status: "available".into(),
                confidence: confidence.into(),
                project_roots: roots,
                manifests,
                toolchain: tool.into(),
                degraded_reason: (!has_tool).then(|| {
                    format!("{tool} not found on PATH; using file/hash/import heuristics")
                }),
            });
        }
        Ok(adapters)
    }

    fn language_indexes(
        &self,
        files: &[SourceFile],
        adapters: &[AdapterReport],
    ) -> Result<Vec<LanguageIndex>> {
        let mut indexes = Vec::new();
        for adapter in adapters {
            let language_files = files
                .iter()
                .filter(|file| {
                    file.language == adapter.language
                        || language_pair(&file.language, &adapter.language)
                })
                .collect::<Vec<_>>();
            let mut symbols = Vec::new();
            let mut dependency_edges = Vec::new();
            let mut entry_points = Vec::new();
            let mut tests = Vec::new();
            for file in &language_files {
                let text = fs::read_to_string(&file.absolute).unwrap_or_default();
                if is_entry_point(&adapter.language, &file.path, &text) {
                    entry_points.push(Evidence {
                        path: file.path.clone(),
                        line: Some(1),
                        sha256: Some(file.sha256.clone()),
                        reason: format!("{} entry point", adapter.language),
                    });
                }
                if is_test_file(&file.path) {
                    tests.push(test_target_for(&adapter.language, &file.path));
                }
                for (line, content) in text.lines().enumerate() {
                    if let Some(symbol) = extract_symbol(
                        &adapter.language,
                        content,
                        &file.path,
                        line + 1,
                        &file.sha256,
                    ) {
                        symbols.push(symbol);
                    }
                    if let Some(edge) = extract_dependency(
                        &adapter.language,
                        content,
                        &file.path,
                        line + 1,
                        &file.sha256,
                    ) {
                        dependency_edges.push(edge);
                    }
                }
            }
            tests.extend(manifest_test_targets(&adapter.language, &adapter.manifests));
            dedupe_tests(&mut tests);
            let mut verification = verification_commands(&adapter.language, !tests.is_empty());
            for test in &tests {
                if !verification.contains(&test.command) {
                    verification.push(test.command.clone());
                }
            }
            indexes.push(LanguageIndex {
                language: adapter.language.clone(),
                roots: adapter.project_roots.clone(),
                entry_points,
                symbols,
                dependency_edges,
                test_targets: tests,
                verification_commands: verification,
                confidence: adapter.confidence.clone(),
                stale_reasons: adapter.degraded_reason.iter().cloned().collect(),
            });
        }
        Ok(indexes)
    }
}

pub fn run_benchmark_smoke() -> Result<BenchmarkReport> {
    run_benchmark_smoke_in_workspace(&workspace_root()?)
}

pub fn run_benchmark_smoke_in_workspace(workspace: &Path) -> Result<BenchmarkReport> {
    let temp = TempManager::new(workspace)?;
    let run = temp.run_dir("benchmark run --smoke")?;
    let root = run.path.join("fixtures");
    let result = run_benchmark_smoke_in_temp_root(&root);
    let _ = fs::remove_dir_all(&root);
    match result {
        Ok(report) => {
            if report.status == "passed" {
                run.complete()?;
            } else {
                run.fail()?;
            }
            Ok(report)
        }
        Err(error) => {
            let _ = run.fail();
            Err(error)
        }
    }
}

fn run_benchmark_smoke_in_temp_root(root: &Path) -> Result<BenchmarkReport> {
    fs::create_dir_all(root)?;
    let cases = [
        (
            "rust",
            "Cargo.toml",
            "[package]\nname=\"demo\"\nversion=\"0.1.0\"\n",
            "src/lib.rs",
            "pub fn add(a:i32,b:i32)->i32 { a+b }\n#[test]\nfn adds(){}\n",
        ),
        (
            "typescript",
            "package.json",
            "{\"scripts\":{\"test\":\"vitest\"}}\n",
            "src/index.ts",
            "export function add(a:number,b:number){ return a+b }\n",
        ),
        (
            "python",
            "pyproject.toml",
            "[project]\nname=\"demo\"\n",
            "demo/__init__.py",
            "def add(a, b):\n    return a + b\n",
        ),
        (
            "csharp",
            "Demo.csproj",
            "<Project Sdk=\"Microsoft.NET.Sdk\"></Project>\n",
            "Program.cs",
            "public class Program { public static void Main() {} }\n",
        ),
        (
            "go",
            "go.mod",
            "module demo\n",
            "main.go",
            "package main\nfunc main() {}\n",
        ),
    ];
    let mut results = Vec::new();
    for (language, manifest, manifest_text, source, source_text) in cases {
        let case_root = root.join(language);
        fs::create_dir_all(case_root.join(Path::new(source).parent().unwrap_or(Path::new("."))))?;
        fs::write(case_root.join(manifest), manifest_text)?;
        fs::write(case_root.join(source), source_text)?;
        let index = ProjectAnalyzer::new(&case_root)?.index()?;
        let impact = ProjectAnalyzer::new(&case_root)?.impact(&[PathBuf::from(source)])?;
        results.push(json!({
            "language": language,
            "adapters": index.project_adapters.len(),
            "symbols": index.language_indexes.iter().map(|item| item.symbols.len()).sum::<usize>(),
            "impact_confidence": impact.confidence,
            "status": if index.project_adapters.is_empty() { "failed" } else { "passed" },
        }));
    }
    let failed = results
        .iter()
        .any(|case| case.get("status").and_then(Value::as_str) != Some("passed"));
    Ok(BenchmarkReport {
        status: if failed { "failed" } else { "passed" }.into(),
        generated_at: now_iso(),
        cases: results,
    })
}

fn normalize_changed_paths(workspace: &Path, paths: &[PathBuf]) -> Result<Vec<String>> {
    if paths.is_empty() {
        return Ok(Vec::new());
    }
    paths
        .iter()
        .map(|path| {
            let joined = if path.is_absolute() {
                path.clone()
            } else {
                workspace.join(path)
            };
            let resolved =
                ensure_within(&joined, workspace, "changed path must be within workspace")?;
            Ok(resolved
                .strip_prefix(workspace)
                .unwrap_or(&resolved)
                .to_string_lossy()
                .replace('\\', "/"))
        })
        .collect()
}

fn regression_corpus(impact: &ImpactReport) -> String {
    let mut parts = Vec::new();
    parts.extend(
        impact
            .directly_changed_files
            .iter()
            .map(|evidence| evidence.path.clone()),
    );
    parts.extend(
        impact
            .affected_modules
            .iter()
            .map(|evidence| evidence.path.clone()),
    );
    parts.extend(impact.likely_tests.iter().map(|test| test.command.clone()));
    parts.extend(impact.broad_gates.clone());
    parts.extend(impact.docs_config_risk.clone());
    parts.extend(impact.stale_reasons.clone());
    parts.join("\n")
}

fn regression_risk_recommendations(
    impact: &ImpactReport,
    quality_patterns: &[QualityRegressionItem],
) -> (String, Vec<String>, Vec<String>) {
    let mut score = 0;
    let mut reasons = Vec::new();
    let mut focus = BTreeSet::new();

    if !impact.affected_public_api.is_empty() {
        score += 3;
        reasons.push(format!(
            "public API touched: {} symbols",
            impact.affected_public_api.len()
        ));
        focus.insert("verify API/CLI compatibility and public contracts".to_string());
    }
    if !quality_patterns.is_empty() {
        score += 2;
        reasons.push(format!(
            "matched quality regression patterns: {}",
            quality_patterns
                .iter()
                .map(|pattern| pattern.pattern_id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
        focus.insert("run quality-pattern regression evidence before success".to_string());
    }
    if !impact.docs_config_risk.is_empty() {
        score += 2;
        reasons.push("docs/config drift risk present".into());
        focus.insert("synchronize docs/config and run docs maintenance checks".to_string());
    }
    if impact.asset_retirement.has_blockers() {
        score += 3;
        reasons.push("asset retirement blockers present".into());
        focus.insert("resolve obsolete asset retirement before success".to_string());
    }
    if impact.directly_changed_files.len() > 3 || impact.affected_modules.len() > 5 {
        score += 1;
        reasons.push("broad multi-file impact".into());
        focus.insert("prefer broad regression profile after focused tests".to_string());
    }
    if impact.stale_reasons.iter().any(|reason| !reason.is_empty()) {
        score += 1;
        reasons.push("project adapter reported stale or degraded evidence".into());
        focus.insert("refresh/reread current project evidence".to_string());
    }

    if reasons.is_empty() {
        reasons.push("limited local impact with no matched historical quality pattern".into());
        focus.insert("run minimal selected tests".to_string());
    }
    let risk_level = if score >= 5 {
        "high"
    } else if score >= 2 {
        "medium"
    } else {
        "low"
    }
    .to_string();
    (risk_level, reasons, focus.into_iter().collect())
}

fn evidence_for_path(workspace: &Path, path: &str, reason: &str) -> Result<Evidence> {
    let absolute = workspace.join(path);
    Ok(Evidence {
        path: path.into(),
        line: Some(1),
        sha256: absolute
            .exists()
            .then(|| sha256_file(&absolute))
            .transpose()?,
        reason: reason.into(),
    })
}

fn language_for_file(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_string_lossy().to_ascii_lowercase();
    let ext = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match name.as_str() {
        "cargo.toml" => Some("rust".into()),
        "package.json" => Some("javascript".into()),
        "tsconfig.json" => Some("typescript".into()),
        "pyproject.toml" | "setup.py" => Some("python".into()),
        "go.mod" => Some("go".into()),
        _ if name.ends_with(".csproj") || name.ends_with(".sln") => Some("csharp".into()),
        _ => match ext.as_str() {
            "rs" => Some("rust".into()),
            "js" | "jsx" | "mjs" | "cjs" => Some("javascript".into()),
            "ts" | "tsx" => Some("typescript".into()),
            "py" => Some("python".into()),
            "cs" => Some("csharp".into()),
            "go" => Some("go".into()),
            _ => None,
        },
    }
}

fn manifest_evidence(workspace: &Path, language: &str) -> Result<Vec<Evidence>> {
    let mut out = Vec::new();
    for entry in WalkDir::new(workspace)
        .into_iter()
        .filter_entry(|entry| !ignored(entry.path(), workspace))
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().is_file())
    {
        let path = entry.path();
        if is_manifest(language, path) {
            out.push(Evidence {
                path: relative_path(workspace, path),
                line: Some(1),
                sha256: Some(sha256_file(path)?),
                reason: format!("{language} manifest"),
            });
        }
    }
    Ok(out)
}

fn is_manifest(language: &str, path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match language {
        "rust" => name == "cargo.toml",
        "javascript" => name == "package.json",
        "typescript" => name == "tsconfig.json" || name == "package.json",
        "python" => {
            name == "pyproject.toml" || name == "setup.py" || name.starts_with("requirements")
        }
        "csharp" => name.ends_with(".sln") || name.ends_with(".csproj"),
        "go" => name == "go.mod",
        _ => false,
    }
}

fn roots_for(workspace: &Path, manifests: &[Evidence], files: &[&SourceFile]) -> Vec<String> {
    let mut roots = BTreeSet::new();
    for manifest in manifests {
        roots.insert(parent_path(&manifest.path));
    }
    for file in files {
        roots.insert(parent_path(&file.path));
    }
    if roots.is_empty() {
        roots.insert(display_path(workspace));
    }
    roots.into_iter().collect()
}

fn parent_path(path: &str) -> String {
    Path::new(path)
        .parent()
        .map(|path| path.to_string_lossy().replace('\\', "/"))
        .filter(|path| !path.is_empty())
        .unwrap_or_else(|| ".".into())
}

fn extract_symbol(
    language: &str,
    line: &str,
    path: &str,
    line_number: usize,
    sha256: &str,
) -> Option<ProjectSymbol> {
    let trimmed = line.trim_start();
    if trimmed.starts_with("//") || trimmed.starts_with('#') {
        return None;
    }
    let (visibility, rest) = if let Some(rest) = trimmed.strip_prefix("pub ") {
        ("public", rest)
    } else if let Some(rest) = trimmed.strip_prefix("export ") {
        ("public", rest)
    } else if let Some(rest) = trimmed.strip_prefix("public ") {
        ("public", rest)
    } else if language == "go" && starts_exported_go_symbol(trimmed) {
        ("public", trimmed)
    } else {
        ("private", trimmed)
    };
    let candidates = match language {
        "rust" => [
            ("fn ", "function"),
            ("struct ", "type"),
            ("enum ", "type"),
            ("trait ", "type"),
            ("mod ", "module"),
        ]
        .as_slice(),
        "javascript" | "typescript" => [
            ("function ", "function"),
            ("class ", "type"),
            ("const ", "constant"),
            ("let ", "variable"),
            ("interface ", "type"),
            ("type ", "type"),
        ]
        .as_slice(),
        "python" => [("def ", "function"), ("class ", "type")].as_slice(),
        "csharp" => [
            ("class ", "type"),
            ("interface ", "type"),
            ("struct ", "type"),
            ("enum ", "type"),
            ("void ", "function"),
            ("static ", "function"),
        ]
        .as_slice(),
        "go" => [("func ", "function"), ("type ", "type")].as_slice(),
        _ => [].as_slice(),
    };
    for (prefix, kind) in candidates {
        if let Some(name) = rest.strip_prefix(prefix).map(symbol_name)
            && !name.is_empty()
        {
            return Some(ProjectSymbol {
                language: language.into(),
                name,
                kind: (*kind).into(),
                visibility: visibility.into(),
                path: path.into(),
                line: line_number,
                sha256: sha256.into(),
            });
        }
    }
    None
}

fn extract_dependency(
    language: &str,
    line: &str,
    path: &str,
    line_number: usize,
    sha256: &str,
) -> Option<DependencyEdge> {
    let trimmed = line.trim();
    let target = match language {
        "rust" => trimmed
            .strip_prefix("use ")
            .or_else(|| trimmed.strip_prefix("mod "))
            .map(|v| v.trim_end_matches(';').to_string()),
        "javascript" | "typescript" => extract_js_import(trimmed),
        "python" => trimmed
            .strip_prefix("import ")
            .or_else(|| trimmed.strip_prefix("from "))
            .map(|v| v.split_whitespace().next().unwrap_or("").to_string()),
        "csharp" => trimmed
            .strip_prefix("using ")
            .map(|v| v.trim_end_matches(';').to_string()),
        "go" => extract_go_import(trimmed),
        _ => None,
    }?;
    if target.is_empty() {
        return None;
    }
    Some(DependencyEdge {
        language: language.into(),
        from: path.into(),
        to: target,
        kind: "import".into(),
        evidence: Evidence {
            path: path.into(),
            line: Some(line_number),
            sha256: Some(sha256.into()),
            reason: format!("{language} dependency"),
        },
    })
}

fn extract_js_import(line: &str) -> Option<String> {
    if let Some((_, rest)) = line.split_once(" from ") {
        return Some(rest.trim().trim_matches(['"', '\'', ';']).to_string());
    }
    if let Some(rest) = line.strip_prefix("import ") {
        return Some(rest.trim().trim_matches(['"', '\'', ';']).to_string());
    }
    if let Some(start) = line.find("require(") {
        let rest = &line[start + "require(".len()..];
        return Some(rest.trim().trim_matches(['"', '\'', ')', ';']).to_string());
    }
    None
}

fn extract_go_import(line: &str) -> Option<String> {
    line.strip_prefix("import ")
        .map(|value| value.trim().trim_matches(['"', '`']).to_string())
}

fn symbol_name(rest: &str) -> String {
    rest.split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_' || ch == '-'))
        .next()
        .unwrap_or("")
        .to_string()
}

fn starts_exported_go_symbol(line: &str) -> bool {
    line.strip_prefix("func ")
        .or_else(|| line.strip_prefix("type "))
        .and_then(|rest| rest.chars().next())
        .map(|ch| ch.is_ascii_uppercase())
        .unwrap_or(false)
}

fn is_entry_point(language: &str, path: &str, text: &str) -> bool {
    match language {
        "rust" => path.ends_with("src/main.rs") || path.ends_with("src/lib.rs"),
        "javascript" | "typescript" => {
            path.ends_with("index.js")
                || path.ends_with("index.ts")
                || path.ends_with("main.ts")
                || path.ends_with("main.js")
        }
        "python" => {
            path.ends_with("__main__.py") || path.ends_with("app.py") || path.ends_with("main.py")
        }
        "csharp" => path.ends_with("Program.cs") || text.contains(" static void Main("),
        "go" => path.ends_with("main.go") || text.contains("package main"),
        _ => false,
    }
}

fn is_test_file(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.contains("/tests/")
        || lower.contains("\\tests\\")
        || lower.ends_with("_test.go")
        || lower.ends_with("_test.rs")
        || lower.ends_with(".test.ts")
        || lower.ends_with(".spec.ts")
        || lower.ends_with(".test.js")
        || lower.ends_with(".spec.js")
        || lower.ends_with("_test.py")
        || lower.ends_with("test.py")
        || lower.ends_with("tests.cs")
}

fn test_target_for(language: &str, path: &str) -> TestTarget {
    TestTarget {
        language: language.into(),
        name: path.into(),
        path: Some(path.into()),
        command: match language {
            "rust" => "cargo test --all",
            "javascript" | "typescript" => "npm test",
            "python" => "pytest",
            "csharp" => "dotnet test",
            "go" => "go test ./...",
            _ => "echo no test command",
        }
        .into(),
        confidence: "medium".into(),
    }
}

fn manifest_test_targets(language: &str, manifests: &[Evidence]) -> Vec<TestTarget> {
    if manifests.is_empty() {
        return Vec::new();
    }
    vec![TestTarget {
        language: language.into(),
        name: format!("{language} default test command"),
        path: manifests.first().map(|manifest| manifest.path.clone()),
        command: match language {
            "rust" => "cargo test --all",
            "javascript" | "typescript" => "npm test",
            "python" => "pytest",
            "csharp" => "dotnet test",
            "go" => "go test ./...",
            _ => "echo no test command",
        }
        .into(),
        confidence: "medium".into(),
    }]
}

fn verification_commands(language: &str, has_tests: bool) -> Vec<String> {
    let mut commands = match language {
        "rust" => vec![
            "cargo fmt --check",
            "cargo clippy --all-targets -- -D warnings",
            "cargo test --all",
        ],
        "javascript" => vec!["npm test"],
        "typescript" => vec!["npm test", "npx tsc --noEmit"],
        "python" => vec!["pytest"],
        "csharp" => vec!["dotnet test"],
        "go" => vec!["go test ./..."],
        _ => vec![],
    }
    .into_iter()
    .map(str::to_string)
    .collect::<Vec<_>>();
    if !has_tests && matches!(language, "javascript" | "typescript") {
        commands.push("npm run build --if-present".into());
    }
    commands
}

fn dedupe_tests(tests: &mut Vec<TestTarget>) {
    let mut seen = BTreeSet::new();
    tests.retain(|test| seen.insert(test.command.clone() + test.path.as_deref().unwrap_or("")));
}

fn command_available(command: &str) -> bool {
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path).any(|dir| {
        let exe = dir.join(command);
        let win = dir.join(format!("{command}.exe"));
        exe.exists() || win.exists()
    })
}

fn tool_for_language(language: &str) -> &'static str {
    match language {
        "rust" => "cargo",
        "javascript" | "typescript" => "node",
        "python" => "python",
        "csharp" => "dotnet",
        "go" => "go",
        _ => "unknown",
    }
}

fn ignored(path: &Path, workspace: &Path) -> bool {
    path.strip_prefix(workspace)
        .ok()
        .map(|relative| {
            relative.components().any(|component| {
                IGNORED_DIRS.contains(&component.as_os_str().to_string_lossy().as_ref())
            })
        })
        .unwrap_or(true)
}

fn relative_path(workspace: &Path, path: &Path) -> String {
    path.strip_prefix(workspace)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn language_pair(left: &str, right: &str) -> bool {
    matches!(
        (left, right),
        ("javascript", "typescript") | ("typescript", "javascript")
    )
}

fn same_stem_changed(changed: &BTreeSet<String>, candidate: &str) -> bool {
    let stem = Path::new(candidate)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("");
    !stem.is_empty() && changed.iter().any(|path| path.contains(stem))
}

fn related_test(source: &str, test: &str) -> bool {
    let source_stem = Path::new(source)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("");
    !source_stem.is_empty() && test.contains(source_stem)
}

fn is_doc_path(path: &str) -> bool {
    path.ends_with(".md") || path.starts_with("docs/")
}

fn is_config_path(path: &str) -> bool {
    path.ends_with(".toml")
        || path.ends_with(".yaml")
        || path.ends_with(".yml")
        || path.ends_with(".json")
        || path.ends_with(".csproj")
        || path.ends_with(".sln")
        || path.ends_with("go.mod")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_all_first_wave_languages() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("Cargo.toml"), "[package]\nname='demo'\n").unwrap();
        fs::create_dir_all(temp.path().join("src")).unwrap();
        fs::write(temp.path().join("src").join("lib.rs"), "pub fn demo() {}\n").unwrap();
        fs::write(
            temp.path().join("package.json"),
            "{\"scripts\":{\"test\":\"vitest\"}}\n",
        )
        .unwrap();
        fs::write(temp.path().join("tsconfig.json"), "{}").unwrap();
        fs::write(temp.path().join("index.ts"), "export function run() {}\n").unwrap();
        fs::write(
            temp.path().join("pyproject.toml"),
            "[project]\nname='demo'\n",
        )
        .unwrap();
        fs::write(temp.path().join("app.py"), "def main():\n    pass\n").unwrap();
        fs::write(temp.path().join("Demo.csproj"), "<Project />").unwrap();
        fs::write(temp.path().join("Program.cs"), "public class Program {}\n").unwrap();
        fs::write(temp.path().join("go.mod"), "module demo\n").unwrap();
        fs::write(
            temp.path().join("main.go"),
            "package main\nfunc main() {}\n",
        )
        .unwrap();

        let index = ProjectAnalyzer::new(temp.path()).unwrap().index().unwrap();
        let languages = index
            .project_adapters
            .iter()
            .map(|adapter| adapter.language.as_str())
            .collect::<BTreeSet<_>>();

        for language in ["rust", "javascript", "typescript", "python", "csharp", "go"] {
            assert!(languages.contains(language), "missing {language}");
        }
        assert!(!index.dependency_graph.is_empty() || !index.language_indexes.is_empty());
    }

    #[test]
    fn impact_selects_related_tests_and_flags_docs_config() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(
            temp.path().join("package.json"),
            "{\"scripts\":{\"test\":\"vitest\"}}\n",
        )
        .unwrap();
        fs::create_dir_all(temp.path().join("src")).unwrap();
        fs::write(
            temp.path().join("src").join("math.ts"),
            "export function add() {}\n",
        )
        .unwrap();
        fs::write(
            temp.path().join("src").join("math.test.ts"),
            "import './math'\n",
        )
        .unwrap();

        let impact = ProjectAnalyzer::new(temp.path())
            .unwrap()
            .impact(&[PathBuf::from("src/math.ts"), PathBuf::from("package.json")])
            .unwrap();

        assert!(
            impact
                .likely_tests
                .iter()
                .any(|test| test.command == "npm test")
        );
        assert!(!impact.docs_config_risk.is_empty());
        assert!(
            impact
                .affected_public_api
                .iter()
                .any(|symbol| symbol.name == "add")
        );
    }

    #[test]
    fn impact_rejects_paths_outside_workspace() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(
            temp.path().join("Cargo.toml"),
            "[package]\nname = \"demo\"\n",
        )
        .unwrap();
        let outside = tempfile::NamedTempFile::new().unwrap();

        let result = ProjectAnalyzer::new(temp.path())
            .unwrap()
            .impact(&[outside.path().to_path_buf()]);

        assert!(result.is_err());
    }

    #[test]
    fn regression_plan_includes_workspace_quality_patterns() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(
            temp.path().join("Cargo.toml"),
            "[package]\nname = \"demo\"\n",
        )
        .unwrap();
        fs::create_dir_all(temp.path().join("src")).unwrap();
        fs::write(temp.path().join("src").join("lib.rs"), "pub fn run() {}\n").unwrap();
        crate::quality::QualityManager::new(temp.path())
            .unwrap()
            .add_incident(crate::quality::QualityIncidentDraft {
                source: "codex://threads/example".into(),
                symptom: "customer program missed release build".into(),
                root_cause: "debug/release delivery was not gated".into(),
                fix: "require both modes".into(),
                generalized_behavior: "debug and release builds must pass".into(),
                pattern_id: Some("debug_release_delivery".into()),
                tags: Vec::new(),
            })
            .unwrap();

        let plan = ProjectAnalyzer::new(temp.path())
            .unwrap()
            .regression_plan(&[PathBuf::from("src/lib.rs")])
            .unwrap();

        assert!(plan.quality_patterns.iter().any(|pattern| {
            pattern.pattern_id == "debug_release_delivery" && pattern.incident_count == 1
        }));
        assert_ne!(plan.risk_level, "low");
        assert!(
            plan.recommended_focus
                .iter()
                .any(|item| item.contains("quality-pattern"))
        );
    }

    #[test]
    fn project_index_excludes_obsolete_assets_from_current_symbols() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(
            temp.path().join("Cargo.toml"),
            "[package]\nname = \"demo\"\n",
        )
        .unwrap();
        fs::create_dir_all(temp.path().join("src")).unwrap();
        fs::write(
            temp.path().join("src").join("old.rs"),
            "pub fn old_behavior() {}\n",
        )
        .unwrap();
        crate::assets::AssetRetirementManager::new(temp.path())
            .unwrap()
            .retire(crate::assets::AssetRetireRequest {
                path: PathBuf::from("src/old.rs"),
                replacement_behavior: "src/new.rs".into(),
                deletion_reason: "replaced by new behavior".into(),
                validation_command: "cargo test".into(),
                apply_delete: false,
            })
            .unwrap();

        let index = ProjectAnalyzer::new(temp.path()).unwrap().index().unwrap();

        assert!(index.asset_retirement.has_blockers());
        assert!(!index.language_indexes.iter().any(|language| {
            language
                .symbols
                .iter()
                .any(|symbol| symbol.name == "old_behavior")
        }));
    }

    #[test]
    fn impact_and_regression_surface_asset_retirement_blockers() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(
            temp.path().join("Cargo.toml"),
            "[package]\nname = \"demo\"\n",
        )
        .unwrap();
        fs::create_dir_all(temp.path().join("src")).unwrap();
        fs::write(
            temp.path().join("src").join("lib.rs"),
            "pub fn current() {}\n",
        )
        .unwrap();
        fs::write(temp.path().join("src").join("old.rs"), "pub fn old() {}\n").unwrap();
        crate::assets::AssetRetirementManager::new(temp.path())
            .unwrap()
            .retire(crate::assets::AssetRetireRequest {
                path: PathBuf::from("src/old.rs"),
                replacement_behavior: "src/lib.rs".into(),
                deletion_reason: "replaced by current".into(),
                validation_command: "cargo test".into(),
                apply_delete: false,
            })
            .unwrap();

        let impact = ProjectAnalyzer::new(temp.path())
            .unwrap()
            .impact(&[PathBuf::from("src/lib.rs")])
            .unwrap();
        let plan = ProjectAnalyzer::new(temp.path())
            .unwrap()
            .regression_plan(&[PathBuf::from("src/lib.rs")])
            .unwrap();

        assert!(impact.asset_retirement.has_blockers());
        assert!(
            impact
                .docs_config_risk
                .iter()
                .any(|item| { item.contains("obsolete asset retirement blockers") })
        );
        assert!(
            plan.checklist
                .iter()
                .any(|item| { item.contains("Resolve obsolete asset retirement blockers") })
        );
    }

    #[test]
    fn project_index_includes_rust_src_bin_sources() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(
            temp.path().join("Cargo.toml"),
            "[package]\nname = \"demo\"\n",
        )
        .unwrap();
        fs::create_dir_all(temp.path().join("src").join("bin")).unwrap();
        fs::write(
            temp.path().join("src").join("bin").join("cli.rs"),
            "pub fn cli_entry() {}\n",
        )
        .unwrap();

        let index = ProjectAnalyzer::new(temp.path()).unwrap().index().unwrap();

        assert!(index.language_indexes.iter().any(|language| {
            language
                .symbols
                .iter()
                .any(|symbol| symbol.path == "src/bin/cli.rs" && symbol.name == "cli_entry")
        }));
    }

    #[test]
    fn benchmark_smoke_covers_first_wave_languages() {
        let temp = tempfile::tempdir().unwrap();
        let report = run_benchmark_smoke_in_workspace(temp.path()).unwrap();
        assert_eq!(report.status, "passed");
        assert_eq!(report.cases.len(), 5);
        assert!(temp.path().join(".RaymanCodingSkill").join("tmp").exists());
    }
}
