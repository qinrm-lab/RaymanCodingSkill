use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use super::{
    ContextIndex, MapFileSource, PackageDependency, PackageEntry, count_package_files,
    discover_packages, indexed_file_bytes, is_cargo_manifest_path, manifest_root_path,
    read_workspace_info,
};

#[derive(Debug, Deserialize)]
pub(super) struct CargoMetadataDocument {
    pub(super) packages: Vec<CargoMetadataPackage>,
    pub(super) workspace_members: Vec<String>,
    pub(super) workspace_root: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct CargoMetadataPackage {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) manifest_path: String,
    #[serde(default)]
    pub(super) dependencies: Vec<CargoMetadataDependency>,
}

#[derive(Debug, Deserialize)]
pub(super) struct CargoMetadataDependency {
    pub(super) name: String,
    #[serde(default)]
    pub(super) rename: Option<String>,
    #[serde(default)]
    pub(super) path: Option<String>,
    #[serde(default)]
    pub(super) kind: Option<String>,
}

/// Cargo itself is the authoritative interpreter for manifests. When it can
/// run, never infer workspace/package/path-dependency relationships from
/// line-oriented heuristics; only fall back for non-Cargo workspaces or when
/// metadata is unavailable, and preserve that weaker provenance.
///
/// Limit nested metadata invocations so repositories with many fixtures do
/// not trigger an unbounded subprocess storm.
const MAX_NESTED_METADATA_MANIFESTS: usize = 32;
const CARGO_METADATA_ARGS: &[&str] =
    &["metadata", "--locked", "--no-deps", "--format-version", "1"];

#[derive(Debug)]
pub(super) struct CapturedManifestAuthority {
    by_comparison_key: BTreeMap<String, String>,
}

impl CapturedManifestAuthority {
    pub(super) fn from_index(
        root: &Path,
        index: &ContextIndex,
        source: MapFileSource<'_>,
    ) -> Result<Self> {
        let mut by_comparison_key = BTreeMap::new();
        for entry in index
            .files
            .iter()
            .filter(|entry| is_cargo_manifest_path(&entry.path))
        {
            // Validate presence, size and content hash before Cargo can observe
            // the live workspace. In captured mode this is the authority for
            // the accepted manifest set, not merely an index path claim.
            indexed_file_bytes(root, entry, source)?;
            let key = manifest_comparison_key(&entry.path);
            if let Some(previous) = by_comparison_key.insert(key, entry.path.clone())
                && previous != entry.path
            {
                bail!(
                    "decision capture contains aliased Cargo manifests: {previous} and {}",
                    entry.path
                );
            }
        }
        Ok(Self { by_comparison_key })
    }

    #[cfg(test)]
    pub(super) fn from_paths(paths: impl IntoIterator<Item = String>) -> Result<Self> {
        let mut by_comparison_key = BTreeMap::new();
        for path in paths {
            let key = manifest_comparison_key(&path);
            if let Some(previous) = by_comparison_key.insert(key, path.clone())
                && previous != path
            {
                bail!("decision capture contains aliased Cargo manifests: {previous} and {path}");
            }
        }
        Ok(Self { by_comparison_key })
    }

    fn contains(&self, path: &str) -> bool {
        self.by_comparison_key
            .contains_key(&manifest_comparison_key(path))
    }
}

fn manifest_comparison_key(path: &str) -> String {
    let normalized = path.replace('\\', "/");
    #[cfg(windows)]
    {
        normalized.to_lowercase()
    }
    #[cfg(not(windows))]
    {
        normalized
    }
}

pub(super) fn cargo_metadata_command(root: &Path, manifest: Option<&str>) -> Command {
    let mut command = Command::new("cargo");
    command.args(CARGO_METADATA_ARGS).current_dir(root);
    if let Some(manifest) = manifest {
        command.args(["--manifest-path", manifest]);
    }
    command
}

/// Try `cargo metadata` for every indexed manifest when the repository root
/// has no Cargo.toml. Only a complete successful traversal is authoritative;
/// any rejected manifest forces the whole topology back to heuristic status.
pub(super) fn nested_cargo_metadata_topology(
    root: &Path,
    index: &ContextIndex,
    source: MapFileSource<'_>,
    manifests: &CapturedManifestAuthority,
) -> Option<Result<(Vec<PackageEntry>, Vec<PackageDependency>)>> {
    let mut discovered_manifests: Vec<&str> = index
        .files
        .iter()
        .map(|file| file.path.as_str())
        .filter(|path| is_cargo_manifest_path(path))
        .collect();
    discovered_manifests.sort_unstable();
    if discovered_manifests.is_empty() {
        return None;
    }

    if discovered_manifests.len() > MAX_NESTED_METADATA_MANIFESTS {
        return Some(Err(anyhow::anyhow!(
            "嵌套 Cargo manifest 超过 {MAX_NESTED_METADATA_MANIFESTS} 个，已停止逐个解析；把它们纳入同一个 workspace（根 Cargo.toml 的 `[workspace] members`），或把 fixture manifest 排除出索引"
        )));
    }

    let mut packages: Vec<PackageEntry> = Vec::new();
    let mut dependencies: Vec<PackageDependency> = Vec::new();
    // Every manifest is resolved by its own `cargo metadata` run. Skipping the
    // ones an earlier run "already returned" looked like a free optimization,
    // but `cargo_metadata_at` also appends heuristically-discovered packages, so
    // the skip-set was seeded with manifests cargo had never parsed: one
    // invocation marked the whole repo covered, and a manifest cargo rejects
    // sailed through as authoritative `cargo_metadata` topology.
    // Each run also returns heuristic entries for the manifests cargo did not
    // resolve, so the same manifest appears in several runs. Keep the entry
    // produced by that manifest's OWN run — plain first-wins dedup kept an
    // earlier run's guess, which mislabels workspace membership and yields a
    // `cargo test -p <name>` recommendation that cannot run.
    let mut authoritative: BTreeSet<String> = BTreeSet::new();
    let mut by_manifest: BTreeMap<String, PackageEntry> = BTreeMap::new();
    for manifest in discovered_manifests {
        let (found, deps) = match cargo_metadata_at(root, index, Some(manifest), source, manifests)
        {
            Ok(result) => result,
            Err(error) => return Some(Err(error)),
        };
        for package in found {
            let own_run = package.manifest_path == manifest;
            if own_run {
                authoritative.insert(package.manifest_path.clone());
                by_manifest.insert(package.manifest_path.clone(), package);
            } else if !authoritative.contains(&package.manifest_path) {
                by_manifest
                    .entry(package.manifest_path.clone())
                    .or_insert(package);
            }
        }
        dependencies.extend(deps);
    }
    packages.extend(by_manifest.into_values());
    packages.sort_by(|left, right| left.manifest_path.cmp(&right.manifest_path));
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
    Some(Ok((packages, dependencies)))
}

pub(super) fn cargo_metadata_topology(
    root: &Path,
    index: &ContextIndex,
    source: MapFileSource<'_>,
    manifests: &CapturedManifestAuthority,
) -> Result<(Vec<PackageEntry>, Vec<PackageDependency>)> {
    cargo_metadata_at(root, index, None, source, manifests)
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
    source: MapFileSource<'_>,
    manifests: &CapturedManifestAuthority,
) -> Result<(Vec<PackageEntry>, Vec<PackageDependency>)> {
    if let Some(requested) = manifest
        && !manifests.contains(requested)
    {
        bail!("cargo metadata requested manifest was not in the decision capture: {requested}");
    }
    let mut command = cargo_metadata_command(root, manifest);
    // Keep environment failures distinct from untrusted topology. Both fail
    // closed, but only the former is repaired by the operator's PATH.
    let output = command.output().map_err(|error| {
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
    validate_cargo_metadata_document(root, manifest, manifests, &document)?;
    let workspace_members: BTreeSet<&str> = document
        .workspace_members
        .iter()
        .map(String::as_str)
        .collect();
    let mut packages = document
        .packages
        .iter()
        .map(|package| {
            let relative_manifest = metadata_workspace_relative_path(
                root,
                Path::new(&package.manifest_path),
                "package manifest",
            )?;
            Ok(PackageEntry {
                name: package.name.clone(),
                root_path: manifest_root_path(&relative_manifest),
                manifest_path: relative_manifest,
                workspace_member: workspace_members.contains(package.id.as_str()),
                source_files: 0,
                test_files: 0,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    // Metadata owns workspace membership/dependencies. Add indexed nested
    // manifests that Cargo deliberately excludes so impact analysis still
    // receives an exact manifest-path test command.
    let heuristic_workspace = read_workspace_info(root, index, source)?;
    for candidate in discover_packages(root, index, &heuristic_workspace, source)? {
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
        let relative_manifest = metadata_workspace_relative_path(
            root,
            Path::new(&package.manifest_path),
            "package manifest",
        )?;
        let Some(from) = by_manifest.get(&relative_manifest) else {
            continue;
        };
        for dependency in &package.dependencies {
            let Some(path) = dependency.path.as_deref() else {
                continue;
            };
            let dependency_root =
                metadata_workspace_relative_path(root, Path::new(path), "path dependency root")?;
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
                evidence: "cargo metadata --locked --no-deps".into(),
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

fn metadata_workspace_relative_path(root: &Path, path: &Path, label: &str) -> Result<String> {
    let canonical_root = root.canonicalize().with_context(|| {
        format!(
            "无法规范化 cargo metadata workspace root: {}",
            root.display()
        )
    })?;
    // Canonicalize Cargo's ordinary absolute path before comparing it with the
    // workspace root. This normalizes Windows verbatim paths and resolves
    // symlink/junction aliases so lexical in-workspace escapes fail closed.
    let canonical_path = path
        .canonicalize()
        .with_context(|| format!("无法规范化 cargo metadata {label}: {}", path.display()))?;
    let relative = canonical_path.strip_prefix(&canonical_root).map_err(|_| {
        anyhow::anyhow!(
            "cargo metadata {label} escapes the captured workspace: {}",
            path.display()
        )
    })?;
    if relative
        .components()
        .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        bail!(
            "cargo metadata {label} is not a normalized workspace path: {}",
            path.display()
        );
    }
    Ok(relative.to_string_lossy().replace('\\', "/"))
}

pub(super) fn validate_cargo_metadata_document(
    root: &Path,
    requested_manifest: Option<&str>,
    captured_manifests: &CapturedManifestAuthority,
    document: &CargoMetadataDocument,
) -> Result<()> {
    if let Some(requested) = requested_manifest
        && !captured_manifests.contains(requested)
    {
        bail!("cargo metadata requested manifest was not in the decision capture: {requested}");
    }
    let workspace_root = metadata_workspace_relative_path(
        root,
        Path::new(&document.workspace_root),
        "workspace_root",
    )?;
    let workspace_manifest = if workspace_root.is_empty() {
        "Cargo.toml".to_string()
    } else {
        format!("{workspace_root}/Cargo.toml")
    };
    if !captured_manifests.contains(&workspace_manifest) {
        bail!(
            "cargo metadata workspace manifest was not in the decision capture: {workspace_manifest}"
        );
    }
    let package_ids = document
        .packages
        .iter()
        .map(|package| package.id.as_str())
        .collect::<BTreeSet<_>>();
    if document
        .workspace_members
        .iter()
        .any(|member| !package_ids.contains(member.as_str()))
    {
        bail!("cargo metadata workspace_members contains an unknown package id");
    }
    for package in &document.packages {
        let manifest = metadata_workspace_relative_path(
            root,
            Path::new(&package.manifest_path),
            "package manifest",
        )?;
        if !captured_manifests.contains(&manifest) {
            bail!("cargo metadata package manifest was not in the decision capture: {manifest}");
        }
        for dependency in &package.dependencies {
            let Some(path) = dependency.path.as_deref() else {
                continue;
            };
            let dependency_root =
                metadata_workspace_relative_path(root, Path::new(path), "path dependency root")?;
            let dependency_manifest = if dependency_root.is_empty() {
                "Cargo.toml".to_string()
            } else {
                format!("{dependency_root}/Cargo.toml")
            };
            if !captured_manifests.contains(&dependency_manifest) {
                bail!(
                    "cargo metadata path dependency manifest was not in the decision capture: {dependency_manifest}"
                );
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_cargo_marker_is_distinct_from_untrusted_topology() {
        assert!(topology_blocked_by_missing_cargo(&format!(
            "heuristic_fallback: {TOPOLOGY_TOOL_UNAVAILABLE}: cargo unavailable"
        )));
        assert!(!topology_blocked_by_missing_cargo("cargo_metadata"));
    }
}
