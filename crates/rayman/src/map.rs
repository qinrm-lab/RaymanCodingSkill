use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::context::{self, ContextIndex, FileEntry};
use crate::fsutil::{now_iso, write_json};

const PROJECT_MAP_RELATIVE_PATH: &str = ".RaymanCodingSkill/context/project_map.json";

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
pub struct ImpactReport {
    pub changed_path: String,
    pub direct_dependencies: Vec<Dependency>,
    pub direct_dependents: Vec<Dependency>,
    pub related_tests: Vec<TestTarget>,
    pub risks: Vec<MapRisk>,
    pub recommended_checks: Vec<String>,
    pub recommendation_basis: String,
}

pub fn build(root: &Path) -> Result<ProjectMap> {
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
    let map = build_from_index(root, &index)?;
    write_json(&project_map_path(root), &map)?;
    Ok(map)
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

pub fn impact_report(map: &ProjectMap, path: &str) -> Result<ImpactReport> {
    let path = normalize_query_path(path);
    if !map.modules.iter().any(|module| module.path == path) {
        bail!("项目地图中没有文件: {path}");
    }
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
    let related_tests = related_tests(map, &path);
    let mut recommended_checks = Vec::new();
    if related_tests.is_empty() {
        recommended_checks.push("run the project's focused test or add one for this change".into());
    } else {
        for test in &related_tests {
            recommended_checks.push(format!("run candidate related test {}", test.path));
        }
    }
    if path.ends_with("Cargo.toml") || path.ends_with("Cargo.lock") {
        recommended_checks.push("cargo deny check".into());
    }
    if path.ends_with(".rs") {
        recommended_checks.push("cargo test --all".into());
        recommended_checks.push("cargo clippy --all-targets -- -D warnings".into());
    }
    recommended_checks.sort();
    recommended_checks.dedup();

    Ok(ImpactReport {
        changed_path: path.clone(),
        direct_dependencies,
        direct_dependents,
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
}
