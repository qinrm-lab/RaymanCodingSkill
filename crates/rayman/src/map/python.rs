use std::collections::BTreeSet;
use std::path::Path;

use anyhow::Result;

use crate::context::{self, ContextIndex, FileEntry};

use super::{
    PackageEntry, TestInference, manifest_root_path, path_is_under_package, quoted_assignment,
};

pub(super) fn discover_packages(root: &Path, index: &ContextIndex) -> Result<Vec<PackageEntry>> {
    let mut packages = Vec::new();
    for entry in index
        .files
        .iter()
        .filter(|file| file.path == "pyproject.toml" || file.path.ends_with("/pyproject.toml"))
    {
        // Same tolerance as indexed source files and Cargo manifests: a non-UTF-8 pyproject
        // must not bail the whole project map.
        let text = String::from_utf8_lossy(&context::read_verified_file(root, entry)?).into_owned();
        let root_path = manifest_root_path(&entry.path);
        let name = parse_pyproject_name(&text).unwrap_or_else(|| {
            if root_path == "." {
                root.file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("python-project")
                    .to_string()
            } else {
                Path::new(&root_path)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("python-project")
                    .to_string()
            }
        });
        let source_files = index
            .files
            .iter()
            .filter(|file| {
                file.kind == "source"
                    && file.path.ends_with(".py")
                    && path_is_under_package(&file.path, &root_path)
            })
            .count();
        let test_files = index
            .files
            .iter()
            .filter(|file| {
                file.kind == "test"
                    && file.path.ends_with(".py")
                    && path_is_under_package(&file.path, &root_path)
            })
            .count();
        packages.push(PackageEntry {
            name,
            root_path,
            manifest_path: entry.path.clone(),
            workspace_member: true,
            source_files,
            test_files,
        });
    }
    packages.sort_by(|left, right| left.manifest_path.cmp(&right.manifest_path));
    packages.dedup_by(|left, right| left.manifest_path == right.manifest_path);
    Ok(packages)
}

fn parse_pyproject_name(text: &str) -> Option<String> {
    let mut section = String::new();
    for raw in text.lines() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.starts_with('[') && line.ends_with(']') {
            section = line
                .trim_start_matches('[')
                .trim_end_matches(']')
                .trim()
                .to_ascii_lowercase();
            continue;
        }
        if matches!(section.as_str(), "project" | "tool.poetry")
            && let Some(name) = quoted_assignment(line, "name")
        {
            return Some(name);
        }
    }
    None
}

pub(super) fn infer_test_candidates(
    file: &FileEntry,
    text: &str,
    index: &ContextIndex,
) -> TestInference {
    let path_set = index
        .files
        .iter()
        .map(|entry| entry.path.clone())
        .collect::<BTreeSet<_>>();
    let mut covered = text
        .lines()
        .flat_map(|line| local_targets(&file.path, line, &path_set))
        .map(|(path, _, _)| path)
        .filter(|path| {
            index
                .files
                .iter()
                .any(|entry| entry.path == *path && entry.kind == "source")
        })
        .collect::<Vec<_>>();
    let imported = !covered.is_empty();

    let stem = Path::new(&file.path)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("")
        .trim_start_matches("test_")
        .trim_end_matches("_test")
        .to_ascii_lowercase();
    if !stem.is_empty() {
        covered.extend(
            index
                .files
                .iter()
                .filter(|entry| entry.kind == "source" && entry.path.ends_with(".py"))
                .filter(|entry| {
                    Path::new(&entry.path)
                        .file_stem()
                        .and_then(|candidate| candidate.to_str())
                        .is_some_and(|candidate| candidate.eq_ignore_ascii_case(&stem))
                })
                .map(|entry| entry.path.clone()),
        );
    }
    covered.sort();
    covered.dedup();
    TestInference {
        paths: covered,
        basis: if imported {
            "python_import_graph".into()
        } else if !stem.is_empty() {
            "python_test_filename".into()
        } else {
            "python_test_without_source_anchor".into()
        },
        confidence: if imported {
            "high".into()
        } else if !stem.is_empty() {
            "medium".into()
        } else {
            "none".into()
        },
    }
}

pub(super) fn local_targets(
    from_path: &str,
    line: &str,
    path_set: &BTreeSet<String>,
) -> Vec<(String, String, String)> {
    let code = line.split('#').next().unwrap_or("").trim();
    let mut modules = Vec::new();
    if let Some(rest) = code.strip_prefix("from ")
        && let Some((module, imports)) = rest.split_once(" import ")
    {
        modules.push(module.trim().to_string());
        for imported in imports.split(',') {
            let name = imported
                .split_whitespace()
                .next()
                .unwrap_or("")
                .trim_matches(['(', ')']);
            if !name.is_empty() && name != "*" {
                modules.push(format!("{}.{}", module.trim_end_matches('.'), name));
            }
        }
    } else if let Some(rest) = code.strip_prefix("import ") {
        modules.extend(rest.split(',').filter_map(|entry| {
            let module = entry.split_whitespace().next().unwrap_or("");
            (!module.is_empty()).then(|| module.to_string())
        }));
    }

    let mut targets = Vec::new();
    let mut seen = BTreeSet::new();
    for module in modules {
        for target in module_candidates(from_path, &module, path_set) {
            if target != from_path && seen.insert(target.clone()) {
                targets.push((target, "python_import".into(), module.clone()));
            }
        }
    }
    targets
}

fn nearest_pyproject_root(from_path: &str, path_set: &BTreeSet<String>) -> Option<String> {
    let mut current = Path::new(from_path).parent();
    while let Some(directory) = current {
        let normalized = directory.to_string_lossy().replace('\\', "/");
        let manifest = if normalized.is_empty() {
            "pyproject.toml".to_string()
        } else {
            format!("{normalized}/pyproject.toml")
        };
        if path_set.contains(&manifest) {
            return Some(normalized);
        }
        current = directory.parent();
    }
    None
}

fn module_candidates(from_path: &str, module: &str, path_set: &BTreeSet<String>) -> Vec<String> {
    let dot_count = module
        .chars()
        .take_while(|character| *character == '.')
        .count();
    let module = module.trim_start_matches('.');
    let mut base = if dot_count == 0 {
        Vec::new()
    } else {
        Path::new(from_path)
            .parent()
            .map(|path| {
                path.to_string_lossy()
                    .replace('\\', "/")
                    .split('/')
                    .filter(|segment| !segment.is_empty())
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    };
    for _ in 1..dot_count {
        base.pop();
    }
    if !module.is_empty() {
        base.extend(module.split('.').map(str::to_string));
    }
    let joined = base.join("/");
    let mut prefixes = BTreeSet::new();
    if dot_count == 0 {
        if let Some(package_root) = nearest_pyproject_root(from_path, path_set) {
            prefixes.insert(package_root.clone());
            prefixes.insert(format!("{package_root}/src").trim_matches('/').to_string());
        } else {
            prefixes.insert(String::new());
            prefixes.insert("src".into());
        }
    } else {
        prefixes.insert(String::new());
    }

    let mut candidates = Vec::new();
    for prefix in prefixes {
        let stem = [prefix.as_str(), joined.as_str()]
            .into_iter()
            .filter(|segment| !segment.is_empty())
            .collect::<Vec<_>>()
            .join("/");
        for candidate in [format!("{stem}.py"), format!("{stem}/__init__.py")] {
            if path_set.contains(&candidate) {
                candidates.push(candidate);
            }
        }
    }
    candidates.sort();
    candidates.dedup();
    candidates
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_pyproject_names_and_local_imports() {
        assert_eq!(
            parse_pyproject_name("[project]\nname = \"sample\"\n"),
            Some("sample".into())
        );
        assert_eq!(
            parse_pyproject_name("[tool.poetry]\nname = \"legacy\"\n"),
            Some("legacy".into())
        );

        let paths = BTreeSet::from([
            "src/sample/__init__.py".to_string(),
            "src/sample/parser.py".to_string(),
        ]);
        let targets = local_targets("tests/test_parser.py", "from sample import parser", &paths);
        assert!(targets.iter().any(|(path, kind, evidence)| {
            path == "src/sample/parser.py" && kind == "python_import" && evidence == "sample.parser"
        }));
    }
    #[test]
    fn resolves_absolute_imports_from_the_nearest_nested_pyproject_root() {
        let paths = BTreeSet::from([
            "packages/api/pyproject.toml".to_string(),
            "packages/api/src/api/__init__.py".to_string(),
            "src/api/core.py".to_string(),
            "packages/api/src/api/core.py".to_string(),
            "packages/api/tests/test_behavior.py".to_string(),
        ]);
        let targets = local_targets(
            "packages/api/tests/test_behavior.py",
            "from api import core",
            &paths,
        );
        assert!(!targets.iter().any(|(path, _, _)| path == "src/api/core.py"));
        assert!(targets.iter().any(|(path, kind, evidence)| {
            path == "packages/api/src/api/core.py"
                && kind == "python_import"
                && evidence == "api.core"
        }));
    }
}
