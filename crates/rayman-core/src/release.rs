use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::SystemTime;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

use crate::customer_deploy::CustomerDeployManager;
use crate::dependency_policy::{
    DependencyPolicyManager, DependencyPolicyReport, DependencyPolicyRunner,
    default_dependency_runner,
};
use crate::regression_history::{RegressionHistoryManager, RegressionRunRecord};
use crate::selfcheck::{SelfManager, SelfStatus};
use crate::{display_path, ensure_within, now_iso, rayman_exe_name, sha256_file, write_text};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReleaseEvidenceReport {
    pub workspace_path: String,
    pub generated_at: String,
    pub label: String,
    pub status: String,
    pub provenance_status: String,
    pub provenance: ReleaseProvenance,
    pub evidence_path: Option<String>,
    pub self_status: Option<SelfStatus>,
    pub artifacts: Vec<ReleaseArtifactEvidence>,
    pub latest_regression: Option<RegressionRunRecord>,
    pub required_actions: Vec<String>,
    pub recommended_actions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ReleaseEvidenceOptions {
    pub label: String,
    pub write_default: bool,
    pub sbom_path: Option<PathBuf>,
    pub attestation_path: Option<PathBuf>,
    pub signed: bool,
    pub require_provenance: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReleaseProvenance {
    pub status: String,
    pub git_status: String,
    pub git_commit: Option<String>,
    pub git_branch: Option<String>,
    pub git_dirty: Option<bool>,
    pub ci_provider: Option<String>,
    pub ci_run_id: Option<String>,
    pub sbom_path: Option<String>,
    pub sbom_exists: Option<bool>,
    pub attestation_path: Option<String>,
    pub attestation_exists: Option<bool>,
    pub signed: bool,
    pub required: bool,
    pub completeness: ReleaseProvenanceCompleteness,
    pub dependency_policy: DependencyPolicyReport,
    pub required_actions: Vec<String>,
    pub recommended_actions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReleaseProvenanceCompleteness {
    pub git_commit_bound: bool,
    pub clean_worktree: bool,
    pub ci_bound: bool,
    pub sbom_attached: bool,
    pub attestation_attached: bool,
    pub signed_artifact: bool,
    pub external_distribution_ready: bool,
    pub missing: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReleaseArtifactEvidence {
    pub name: String,
    pub path: String,
    pub exists: bool,
    pub sha256: Option<String>,
}

pub struct ReleaseEvidenceManager {
    workspace: PathBuf,
    dependency_runner: Arc<dyn DependencyPolicyRunner>,
}

impl ReleaseEvidenceManager {
    pub fn new(workspace: impl Into<PathBuf>) -> Result<Self> {
        Self::with_dependency_policy_runner(workspace, default_dependency_runner())
    }

    pub fn with_dependency_policy_runner(
        workspace: impl Into<PathBuf>,
        dependency_runner: Arc<dyn DependencyPolicyRunner>,
    ) -> Result<Self> {
        Ok(Self {
            workspace: workspace
                .into()
                .canonicalize()
                .context("无法解析工作区路径")?,
            dependency_runner,
        })
    }

    pub fn generate(&self, label: &str, write_default: bool) -> Result<ReleaseEvidenceReport> {
        self.generate_with_options(ReleaseEvidenceOptions {
            label: label.to_string(),
            write_default,
            ..ReleaseEvidenceOptions::default()
        })
    }

    pub fn generate_with_options(
        &self,
        options: ReleaseEvidenceOptions,
    ) -> Result<ReleaseEvidenceReport> {
        let dependency_policy = DependencyPolicyManager::with_runner(
            &self.workspace,
            Arc::clone(&self.dependency_runner),
        )?
        .audit()?;
        self.generate_with_options_and_dependency_policy(options, dependency_policy)
    }

    pub fn generate_with_options_and_dependency_policy(
        &self,
        options: ReleaseEvidenceOptions,
        dependency_policy: DependencyPolicyReport,
    ) -> Result<ReleaseEvidenceReport> {
        let generated_at = now_iso();
        let provenance = self.provenance(&options, dependency_policy)?;
        let (latest_regression, regression_history_error) =
            match RegressionHistoryManager::new(&self.workspace)
                .and_then(|manager| manager.latest())
            {
                Ok(record) => (record, None),
                Err(error) => (None, Some(format!("{error:#}"))),
            };
        let mut report = ReleaseEvidenceReport {
            workspace_path: display_path(&self.workspace),
            generated_at: generated_at.clone(),
            label: options.label.clone(),
            status: "ready".into(),
            provenance_status: provenance.status.clone(),
            provenance,
            evidence_path: None,
            self_status: SelfManager::new(&self.workspace)
                .and_then(|manager| manager.status())
                .ok(),
            artifacts: self.artifacts()?,
            latest_regression,
            required_actions: Vec::new(),
            recommended_actions: vec![
                "For external distribution, attach a CI-signed artifact attestation and SBOM."
                    .into(),
                "Keep release evidence with the exact regression run that approved the artifact."
                    .into(),
            ],
        };
        if report
            .artifacts
            .iter()
            .any(|artifact| is_release_binary_artifact(&artifact.name) && !artifact.exists)
        {
            report
                .required_actions
                .push("build configured release artifact before release evidence is ready".into());
        }
        if report
            .artifacts
            .iter()
            .any(|artifact| artifact.name == "cargo_lock" && !artifact.exists)
        {
            report
                .required_actions
                .push("include Cargo.lock hash in release evidence".into());
        }
        if let Some((path, _)) = stale_release_binary_input(&self.workspace)? {
            report.required_actions.push(format!(
                "rebuild configured release artifact after current source changes before release evidence is ready: {}",
                display_path(&path)
            ));
        }
        if let Some(error) = regression_history_error {
            report
                .required_actions
                .push(format!("regression history unreadable: {error}"));
        } else if report.latest_regression.is_none() {
            report
                .required_actions
                .push("run rayman regression run before producing release evidence".into());
        } else if report
            .latest_regression
            .as_ref()
            .is_some_and(|record| !record.status.eq_ignore_ascii_case("passed"))
        {
            report
                .required_actions
                .push("latest regression history record must be passed".into());
        } else if let Some(record) = report.latest_regression.as_ref()
            && let Some(action) = stale_regression_action(&self.workspace, record)?
        {
            report.required_actions.push(action);
        }
        if options.require_provenance && !report.provenance.required_actions.is_empty() {
            report
                .required_actions
                .extend(report.provenance.required_actions.clone());
        }
        if report.provenance.dependency_policy.status == "blocked" && !options.require_provenance {
            report.required_actions.extend(
                report
                    .provenance
                    .dependency_policy
                    .required_actions
                    .iter()
                    .map(|action| format!("dependency policy requires: {action}")),
            );
        }
        if !report.required_actions.is_empty() {
            report.status = "partial".into();
        }
        if options.write_default {
            let path = self.default_evidence_path(&options.label, &generated_at)?;
            report.evidence_path = Some(display_path(&path));
            write_text(&path, &serde_json::to_string_pretty(&report)?)?;
        }
        Ok(report)
    }

    fn provenance(
        &self,
        options: &ReleaseEvidenceOptions,
        dependency_policy: DependencyPolicyReport,
    ) -> Result<ReleaseProvenance> {
        let git = git_provenance(&self.workspace);
        let ci_provider = ci_provider();
        let ci_run_id = ci_run_id();
        let sbom_path = display_optional_artifact(&self.workspace, options.sbom_path.as_ref())?;
        let attestation_path =
            display_optional_artifact(&self.workspace, options.attestation_path.as_ref())?;
        let sbom_exists = options.sbom_path.as_ref().map(|path| path.exists());
        let attestation_exists = options.attestation_path.as_ref().map(|path| path.exists());
        let mut required_actions = Vec::new();
        let mut recommended_actions = Vec::new();
        if git.status == "not_git" {
            recommended_actions
                .push("Run release evidence from a Git workspace for commit provenance.".into());
            if options.require_provenance {
                required_actions.push("release provenance requires a Git workspace".into());
            }
        }
        if git.git_dirty == Some(true) {
            recommended_actions.push("Commit or explicitly account for dirty working tree changes before external distribution.".into());
            if options.require_provenance {
                required_actions
                    .push("release provenance requires a clean Git working tree".into());
            }
        }
        if options.require_provenance && !options.signed {
            required_actions.push("release provenance requires --signed".into());
        }
        if options.require_provenance && sbom_exists != Some(true) {
            required_actions.push("release provenance requires an existing --sbom path".into());
        }
        if options.require_provenance && attestation_exists != Some(true) {
            required_actions
                .push("release provenance requires an existing --attestation path".into());
        }
        if sbom_exists == Some(false) {
            recommended_actions.push("SBOM path does not exist yet.".into());
        }
        if attestation_exists == Some(false) {
            recommended_actions.push("Attestation path does not exist yet.".into());
        }
        if dependency_policy.status == "blocked" {
            required_actions.extend(
                dependency_policy
                    .required_actions
                    .iter()
                    .map(|action| format!("dependency policy requires: {action}")),
            );
        }
        let completeness = release_provenance_completeness(
            &git,
            ci_provider.as_deref(),
            sbom_exists,
            attestation_exists,
            options.signed,
        );
        if !completeness.external_distribution_ready {
            recommended_actions.push(format!(
                "Release provenance is not externally complete: missing {}.",
                completeness.missing.join(", ")
            ));
        }
        let status = if !required_actions.is_empty() {
            "incomplete"
        } else if options.signed && sbom_exists == Some(true) && attestation_exists == Some(true) {
            "signed"
        } else if git.status == "not_git" {
            "not_git"
        } else if git.git_dirty == Some(true) {
            "git_dirty"
        } else {
            "local_unsigned"
        };
        Ok(ReleaseProvenance {
            status: status.into(),
            git_status: git.status,
            git_commit: git.git_commit,
            git_branch: git.git_branch,
            git_dirty: git.git_dirty,
            ci_provider,
            ci_run_id,
            sbom_path,
            sbom_exists,
            attestation_path,
            attestation_exists,
            signed: options.signed,
            required: options.require_provenance,
            completeness,
            dependency_policy,
            required_actions,
            recommended_actions,
        })
    }

    fn artifacts(&self) -> Result<Vec<ReleaseArtifactEvidence>> {
        let mut paths = release_binary_artifact_paths(&self.workspace)?;
        paths.extend([
            ("cargo_lock".to_string(), self.workspace.join("Cargo.lock")),
            (
                "workspace_manifest".to_string(),
                self.workspace.join("Cargo.toml"),
            ),
            ("skill".to_string(), self.workspace.join("SKILL.md")),
        ]);
        let crates_dir = self.workspace.join("crates");
        if crates_dir.exists() {
            for entry in fs::read_dir(&crates_dir)
                .with_context(|| format!("无法读取 crates 目录: {}", crates_dir.display()))?
            {
                let entry = entry?;
                let manifest = entry.path().join("Cargo.toml");
                if manifest.exists() {
                    let name = format!(
                        "crate_manifest_{}",
                        entry.file_name().to_string_lossy().replace('-', "_")
                    );
                    paths.push((name, manifest));
                }
            }
        }
        paths
            .into_iter()
            .map(|(name, path)| artifact_evidence(&name, &self.workspace, &path))
            .collect()
    }

    fn default_evidence_path(&self, label: &str, generated_at: &str) -> Result<PathBuf> {
        let file_name = format!(
            "{}_{}.json",
            safe_file_component(generated_at),
            safe_file_component(label)
        );
        ensure_within(
            &self
                .workspace
                .join(".RaymanCodingSkill")
                .join("release")
                .join("evidence")
                .join(file_name),
            &self.workspace,
            "release evidence path must stay inside workspace",
        )
    }
}

#[derive(Debug, Clone)]
struct GitProvenance {
    status: String,
    git_commit: Option<String>,
    git_branch: Option<String>,
    git_dirty: Option<bool>,
}

fn git_provenance(workspace: &Path) -> GitProvenance {
    let inside = git_output(workspace, &["rev-parse", "--is-inside-work-tree"]);
    if inside.as_deref() != Some("true") {
        return GitProvenance {
            status: "not_git".into(),
            git_commit: None,
            git_branch: None,
            git_dirty: None,
        };
    }
    let git_commit = git_output(workspace, &["rev-parse", "HEAD"]);
    let git_branch = git_output(workspace, &["branch", "--show-current"])
        .filter(|branch| !branch.trim().is_empty())
        .or_else(|| git_output(workspace, &["rev-parse", "--abbrev-ref", "HEAD"]));
    let dirty = git_output(workspace, &["status", "--short"])
        .map(|status| !status.trim().is_empty())
        .unwrap_or(true);
    GitProvenance {
        status: if dirty { "git_dirty" } else { "git_clean" }.into(),
        git_commit,
        git_branch,
        git_dirty: Some(dirty),
    }
}

fn git_output(workspace: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(workspace)
        .args(args)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn ci_provider() -> Option<String> {
    if std::env::var_os("GITHUB_ACTIONS").is_some() {
        Some("github_actions".into())
    } else if std::env::var_os("CI").is_some() {
        Some("generic_ci".into())
    } else {
        None
    }
}

fn ci_run_id() -> Option<String> {
    [
        "GITHUB_RUN_ID",
        "CI_PIPELINE_ID",
        "BUILD_BUILDID",
        "BUILD_ID",
    ]
    .iter()
    .find_map(|name| std::env::var(name).ok().filter(|value| !value.is_empty()))
}

fn release_provenance_completeness(
    git: &GitProvenance,
    ci_provider: Option<&str>,
    sbom_exists: Option<bool>,
    attestation_exists: Option<bool>,
    signed: bool,
) -> ReleaseProvenanceCompleteness {
    let git_commit_bound = git.git_commit.is_some();
    let clean_worktree = git.git_dirty == Some(false);
    let ci_bound = ci_provider.is_some();
    let sbom_attached = sbom_exists == Some(true);
    let attestation_attached = attestation_exists == Some(true);
    let signed_artifact = signed;
    let mut missing = Vec::new();
    if !git_commit_bound {
        missing.push("git_commit".into());
    }
    if !clean_worktree {
        missing.push("clean_worktree".into());
    }
    if !ci_bound {
        missing.push("ci_run".into());
    }
    if !sbom_attached {
        missing.push("sbom".into());
    }
    if !attestation_attached {
        missing.push("attestation".into());
    }
    if !signed_artifact {
        missing.push("signature".into());
    }
    ReleaseProvenanceCompleteness {
        git_commit_bound,
        clean_worktree,
        ci_bound,
        sbom_attached,
        attestation_attached,
        signed_artifact,
        external_distribution_ready: missing.is_empty(),
        missing,
    }
}

fn display_optional_artifact(workspace: &Path, path: Option<&PathBuf>) -> Result<Option<String>> {
    let Some(path) = path else {
        return Ok(None);
    };
    let display = if path.exists() {
        path.canonicalize()
            .with_context(|| format!("无法解析证据路径: {}", path.display()))?
    } else if path.is_absolute() {
        path.clone()
    } else {
        workspace.join(path)
    };
    Ok(Some(display_path(&display)))
}

fn artifact_evidence(name: &str, workspace: &Path, path: &Path) -> Result<ReleaseArtifactEvidence> {
    Ok(ReleaseArtifactEvidence {
        name: name.into(),
        path: path
            .strip_prefix(workspace)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/"),
        exists: path.exists(),
        sha256: path.exists().then(|| sha256_file(path)).transpose()?,
    })
}

fn stale_release_binary_input(workspace: &Path) -> Result<Option<(PathBuf, SystemTime)>> {
    let Some((path, modified)) = latest_binary_input_mtime(workspace)? else {
        return Ok(None);
    };
    for binary in release_binary_paths(workspace)? {
        if !binary.exists() {
            continue;
        }
        let binary_modified = binary
            .metadata()
            .with_context(|| format!("无法读取 release binary 元数据: {}", binary.display()))?
            .modified()
            .with_context(|| format!("无法读取 release binary 修改时间: {}", binary.display()))?;
        if modified > binary_modified {
            return Ok(Some((path, modified)));
        }
    }
    Ok(None)
}

fn stale_regression_action(
    workspace: &Path,
    record: &RegressionRunRecord,
) -> Result<Option<String>> {
    let Some((path, modified)) = latest_release_input_mtime(workspace)? else {
        return Ok(None);
    };
    let finished_at = match DateTime::parse_from_rfc3339(&record.finished_at) {
        Ok(value) => value.with_timezone(&Utc),
        Err(_) => {
            return Ok(Some(
                "latest regression history finished_at must be parseable before release evidence is ready"
                    .into(),
            ));
        }
    };
    let finished_at: SystemTime = finished_at.into();
    if modified > finished_at {
        Ok(Some(format!(
            "rerun rayman regression run after current release input changes before release evidence is ready: {}",
            display_path(&path)
        )))
    } else {
        Ok(None)
    }
}

fn default_rayman_release_binary_path(workspace: &Path) -> PathBuf {
    workspace
        .join("target")
        .join("release")
        .join(rayman_exe_name())
}

fn release_binary_paths(workspace: &Path) -> Result<Vec<PathBuf>> {
    Ok(release_binary_artifact_paths(workspace)?
        .into_iter()
        .map(|(_, path)| path)
        .collect())
}

fn release_binary_artifact_paths(workspace: &Path) -> Result<Vec<(String, PathBuf)>> {
    if let Some(paths) = customer_deploy_release_artifact_paths(workspace)? {
        return Ok(paths);
    }
    if let Some(paths) = cargo_release_artifact_paths(workspace)? {
        return Ok(paths);
    }
    Ok(vec![(
        "release_binary".to_string(),
        default_rayman_release_binary_path(workspace),
    )])
}

fn customer_deploy_release_artifact_paths(
    workspace: &Path,
) -> Result<Option<Vec<(String, PathBuf)>>> {
    let Some(config) = CustomerDeployManager::new(workspace)?.read()? else {
        return Ok(None);
    };
    let artifacts = config
        .artifact_paths
        .iter()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    if artifacts.is_empty() {
        return Ok(None);
    }

    let mut paths = Vec::new();
    for (index, artifact) in artifacts.into_iter().enumerate() {
        let path = resolve_workspace_artifact_path(workspace, artifact)?;
        let binary_name = path
            .file_stem()
            .and_then(|name| name.to_str())
            .unwrap_or("artifact");
        paths.push((release_artifact_name(index, binary_name), path));
    }
    Ok(Some(paths))
}

fn cargo_release_artifact_paths(workspace: &Path) -> Result<Option<Vec<(String, PathBuf)>>> {
    if !workspace.join("Cargo.toml").exists() {
        return Ok(None);
    }
    let output = match Command::new("cargo")
        .arg("metadata")
        .arg("--no-deps")
        .arg("--format-version")
        .arg("1")
        .current_dir(workspace)
        .env_remove("CARGO_TARGET_DIR")
        .output()
    {
        Ok(output) if output.status.success() => output,
        _ => return Ok(None),
    };
    let metadata: serde_json::Value =
        serde_json::from_slice(&output.stdout).context("无法解析 cargo metadata 输出")?;
    let Some(target_directory) = metadata
        .get("target_directory")
        .and_then(serde_json::Value::as_str)
        .map(PathBuf::from)
    else {
        return Ok(None);
    };

    let mut names = BTreeSet::new();
    for package in metadata
        .get("packages")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
    {
        for target in package
            .get("targets")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
        {
            let is_binary = target
                .get("kind")
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
                .any(|kind| kind.as_str() == Some("bin"));
            if is_binary
                && let Some(name) = target.get("name").and_then(serde_json::Value::as_str)
                && !name.trim().is_empty()
            {
                names.insert(name.trim().to_string());
            }
        }
    }
    if names.is_empty() {
        return Ok(None);
    }

    let mut paths = Vec::new();
    for (index, name) in names.into_iter().enumerate() {
        let path = resolve_workspace_artifact_path(
            workspace,
            &target_directory
                .join("release")
                .join(executable_name(&name))
                .to_string_lossy(),
        )?;
        paths.push((release_artifact_name(index, &name), path));
    }
    Ok(Some(paths))
}

fn resolve_workspace_artifact_path(workspace: &Path, artifact: &str) -> Result<PathBuf> {
    let path = PathBuf::from(artifact);
    let path = if path.is_absolute() {
        path
    } else {
        workspace.join(path)
    };
    ensure_within(
        &path,
        workspace,
        "release artifact path must stay inside workspace",
    )
}

fn executable_name(target_name: &str) -> String {
    if cfg!(windows) {
        format!("{target_name}.exe")
    } else {
        target_name.to_string()
    }
}

fn release_artifact_name(index: usize, binary_name: &str) -> String {
    if index == 0 {
        return "release_binary".to_string();
    }
    let component = safe_file_component(binary_name);
    if component.is_empty() {
        format!("release_binary_{}", index + 1)
    } else {
        format!("release_binary_{component}")
    }
}

fn is_release_binary_artifact(name: &str) -> bool {
    name == "release_binary" || name.starts_with("release_binary_")
}

fn latest_binary_input_mtime(workspace: &Path) -> Result<Option<(PathBuf, SystemTime)>> {
    latest_input_mtime(workspace, true)
}

fn latest_release_input_mtime(workspace: &Path) -> Result<Option<(PathBuf, SystemTime)>> {
    latest_input_mtime(workspace, false)
}

fn latest_input_mtime(
    workspace: &Path,
    binary_only: bool,
) -> Result<Option<(PathBuf, SystemTime)>> {
    let mut latest = None;
    for path in release_input_paths(workspace, binary_only)? {
        if !path.exists() {
            continue;
        }
        let modified = path
            .metadata()
            .with_context(|| format!("无法读取 release input 元数据: {}", path.display()))?
            .modified()
            .with_context(|| format!("无法读取 release input 修改时间: {}", path.display()))?;
        if latest
            .as_ref()
            .map(|(_, current): &(PathBuf, SystemTime)| modified > *current)
            .unwrap_or(true)
        {
            latest = Some((path, modified));
        }
    }
    Ok(latest)
}

fn release_input_paths(workspace: &Path, binary_only: bool) -> Result<Vec<PathBuf>> {
    let mut paths = vec![workspace.join("Cargo.toml"), workspace.join("Cargo.lock")];
    if !binary_only {
        paths.push(workspace.join("SKILL.md"));
    }
    for source_dir in ["crates", "apps", "src"] {
        collect_release_source_paths(&workspace.join(source_dir), &mut paths);
    }
    Ok(paths)
}

fn collect_release_source_paths(root: &Path, paths: &mut Vec<PathBuf>) {
    if !root.exists() {
        return;
    }
    for entry in WalkDir::new(root)
        .into_iter()
        .filter_map(|entry| entry.ok())
    {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        let extension = path.extension().and_then(|ext| ext.to_str()).unwrap_or("");
        if extension == "rs" || file_name == "Cargo.toml" || file_name == "build.rs" {
            paths.push(path.to_path_buf());
        }
    }
}

fn safe_file_component(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim_matches('_')
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dependency_policy::{
        DependencyPolicyCommandOutput, DependencyPolicyRunError, DependencyPolicyRunner,
    };

    struct StaticDependencyRunner {
        check_success: bool,
    }

    impl DependencyPolicyRunner for StaticDependencyRunner {
        fn run(
            &self,
            _workspace: &Path,
            args: &[&str],
        ) -> std::result::Result<DependencyPolicyCommandOutput, DependencyPolicyRunError> {
            if args == ["deny", "--version"] {
                return Ok(DependencyPolicyCommandOutput {
                    success: true,
                    exit_code: Some(0),
                    stdout: "cargo-deny 0.16.4\n".into(),
                    stderr: String::new(),
                });
            }
            Ok(DependencyPolicyCommandOutput {
                success: self.check_success,
                exit_code: Some(if self.check_success { 0 } else { 1 }),
                stdout: String::new(),
                stderr: if self.check_success {
                    String::new()
                } else {
                    "GPL-3.0 license denied\n".into()
                },
            })
        }
    }

    fn release_manager(path: &Path) -> ReleaseEvidenceManager {
        fs::write(path.join("deny.toml"), "[licenses]\n").unwrap();
        ReleaseEvidenceManager::with_dependency_policy_runner(
            path,
            Arc::new(StaticDependencyRunner {
                check_success: true,
            }),
        )
        .unwrap()
    }

    fn append_future_passed_regression(path: &Path) {
        RegressionHistoryManager::new(path)
            .unwrap()
            .append(&RegressionRunRecord {
                id: "regression_full_future".into(),
                profile: "full".into(),
                status: "passed".into(),
                started_at: "2999-01-01T00:00:00Z".into(),
                finished_at: "2999-01-01T00:00:01Z".into(),
                duration_ms: 1000,
                steps: Vec::new(),
            })
            .unwrap();
    }

    #[test]
    fn release_evidence_uses_customer_deploy_artifact_paths() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(
            temp.path().join("Cargo.lock"),
            "# This file is automatically @generated by Cargo.\nversion = 4\n",
        )
        .unwrap();
        fs::write(temp.path().join("Cargo.toml"), "[workspace]\n").unwrap();
        fs::write(temp.path().join("SKILL.md"), "# skill").unwrap();
        let artifact_name = executable_name("customer-release");
        let artifact_path = temp
            .path()
            .join("target")
            .join("release")
            .join(&artifact_name);
        fs::create_dir_all(artifact_path.parent().unwrap()).unwrap();
        fs::write(&artifact_path, "binary").unwrap();
        crate::customer_deploy::CustomerDeployManager::new(temp.path())
            .unwrap()
            .set(crate::customer_deploy::CustomerDeployUpdate {
                artifact_paths: vec![format!("target/release/{artifact_name}")],
                ..crate::customer_deploy::CustomerDeployUpdate::default()
            })
            .unwrap();
        append_future_passed_regression(temp.path());

        let report = release_manager(temp.path())
            .generate("customer", false)
            .unwrap();

        assert_eq!(report.status, "ready");
        let release_binary = report
            .artifacts
            .iter()
            .find(|artifact| artifact.name == "release_binary")
            .unwrap();
        assert_eq!(
            release_binary.path,
            format!("target/release/{artifact_name}").replace('\\', "/")
        );
        assert!(release_binary.exists);
    }

    #[test]
    fn release_evidence_discovers_cargo_bin_artifacts_for_customer_workspace() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(
            temp.path().join("Cargo.lock"),
            "# This file is automatically @generated by Cargo.\nversion = 4\n",
        )
        .unwrap();
        fs::write(
            temp.path().join("Cargo.toml"),
            "[workspace]\nmembers = [\"apps/customer\"]\nresolver = \"2\"\n",
        )
        .unwrap();
        let app_dir = temp.path().join("apps").join("customer");
        fs::create_dir_all(app_dir.join("src")).unwrap();
        fs::write(
            app_dir.join("Cargo.toml"),
            "[package]\nname = \"customer-workspace\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[[bin]]\nname = \"customer-app\"\npath = \"src/main.rs\"\n",
        )
        .unwrap();
        fs::write(app_dir.join("src").join("main.rs"), "fn main() {}\n").unwrap();
        let artifact_name = executable_name("customer-app");
        let artifact_path = temp
            .path()
            .join("target")
            .join("release")
            .join(&artifact_name);
        fs::create_dir_all(artifact_path.parent().unwrap()).unwrap();
        fs::write(&artifact_path, "binary").unwrap();
        append_future_passed_regression(temp.path());

        let report = release_manager(temp.path())
            .generate("cargo", false)
            .unwrap();

        assert_eq!(report.status, "ready");
        assert!(
            report
                .artifacts
                .iter()
                .any(|artifact| artifact.name == "release_binary"
                    && artifact.path
                        == format!("target/release/{artifact_name}").replace('\\', "/")
                    && artifact.exists)
        );
    }

    #[test]
    fn release_evidence_reports_missing_regression_as_partial() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("Cargo.lock"), "# lock").unwrap();
        fs::write(temp.path().join("Cargo.toml"), "[workspace]\n").unwrap();
        fs::write(temp.path().join("SKILL.md"), "# skill").unwrap();
        fs::create_dir_all(temp.path().join("target").join("release")).unwrap();
        fs::write(
            temp.path()
                .join("target")
                .join("release")
                .join(rayman_exe_name()),
            "binary",
        )
        .unwrap();

        let report = release_manager(temp.path())
            .generate("test", false)
            .unwrap();

        assert_eq!(report.status, "partial");
        assert!(report.latest_regression.is_none());
        assert!(
            report
                .required_actions
                .iter()
                .any(|action| action.contains("regression run"))
        );
        assert_eq!(report.provenance_status, "not_git");
    }

    #[test]
    fn release_evidence_reports_stale_release_binary_as_partial() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("Cargo.lock"), "# lock").unwrap();
        fs::write(temp.path().join("Cargo.toml"), "[workspace]\n").unwrap();
        fs::write(temp.path().join("SKILL.md"), "# skill").unwrap();
        RegressionHistoryManager::new(temp.path())
            .unwrap()
            .append(&RegressionRunRecord {
                id: "regression_full_1".into(),
                profile: "full".into(),
                status: "passed".into(),
                started_at: "2999-01-01T00:00:00Z".into(),
                finished_at: "2999-01-01T00:00:01Z".into(),
                duration_ms: 1000,
                steps: Vec::new(),
            })
            .unwrap();
        fs::create_dir_all(temp.path().join("target").join("release")).unwrap();
        fs::write(
            temp.path()
                .join("target")
                .join("release")
                .join(rayman_exe_name()),
            "binary",
        )
        .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1200));
        fs::create_dir_all(temp.path().join("crates").join("rayman-core").join("src")).unwrap();
        fs::write(
            temp.path()
                .join("crates")
                .join("rayman-core")
                .join("src")
                .join("lib.rs"),
            "pub fn changed() {}\n",
        )
        .unwrap();

        let report = release_manager(temp.path())
            .generate("test", false)
            .unwrap();

        assert_eq!(report.status, "partial");
        assert!(
            report
                .required_actions
                .iter()
                .any(|action| action.contains("rebuild configured release artifact"))
        );
    }

    #[test]
    fn release_evidence_reports_stale_regression_as_partial() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("Cargo.lock"), "# lock").unwrap();
        fs::write(temp.path().join("Cargo.toml"), "[workspace]\n").unwrap();
        fs::write(temp.path().join("SKILL.md"), "# skill").unwrap();
        RegressionHistoryManager::new(temp.path())
            .unwrap()
            .append(&RegressionRunRecord {
                id: "regression_full_1".into(),
                profile: "full".into(),
                status: "passed".into(),
                started_at: "2026-06-08T00:00:00Z".into(),
                finished_at: "2026-06-08T00:00:01Z".into(),
                duration_ms: 1000,
                steps: Vec::new(),
            })
            .unwrap();
        fs::create_dir_all(temp.path().join("target").join("release")).unwrap();
        fs::write(
            temp.path()
                .join("target")
                .join("release")
                .join(rayman_exe_name()),
            "binary",
        )
        .unwrap();

        let report = release_manager(temp.path())
            .generate("test", false)
            .unwrap();

        assert_eq!(report.status, "partial");
        assert!(
            report
                .required_actions
                .iter()
                .any(|action| action.contains("rerun rayman regression run"))
        );
    }

    #[test]
    fn release_evidence_can_write_default_bundle() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("Cargo.lock"), "# lock").unwrap();
        fs::write(temp.path().join("Cargo.toml"), "[workspace]\n").unwrap();
        fs::write(temp.path().join("SKILL.md"), "# skill").unwrap();
        let history = RegressionHistoryManager::new(temp.path()).unwrap();
        history
            .append(&RegressionRunRecord {
                id: "regression_quick_1".into(),
                profile: "quick".into(),
                status: "passed".into(),
                started_at: "2026-06-08T00:00:00Z".into(),
                finished_at: "2999-01-01T00:00:01Z".into(),
                duration_ms: 1000,
                steps: Vec::new(),
            })
            .unwrap();
        fs::create_dir_all(temp.path().join("target").join("release")).unwrap();
        fs::write(
            temp.path()
                .join("target")
                .join("release")
                .join(rayman_exe_name()),
            "binary",
        )
        .unwrap();

        let report = release_manager(temp.path())
            .generate("release candidate", true)
            .unwrap();

        assert_eq!(report.status, "ready");
        let evidence_path = report.evidence_path.clone().unwrap();
        assert!(PathBuf::from(&evidence_path).exists());
        let written: ReleaseEvidenceReport =
            serde_json::from_str(&fs::read_to_string(report.evidence_path.unwrap()).unwrap())
                .unwrap();
        assert_eq!(
            written.evidence_path.as_deref(),
            Some(evidence_path.as_str())
        );
    }

    #[test]
    fn release_evidence_requires_provenance_only_when_requested() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("Cargo.lock"), "# lock").unwrap();
        fs::write(temp.path().join("Cargo.toml"), "[workspace]\n").unwrap();
        fs::write(temp.path().join("SKILL.md"), "# skill").unwrap();
        let history = RegressionHistoryManager::new(temp.path()).unwrap();
        history
            .append(&RegressionRunRecord {
                id: "regression_quick_1".into(),
                profile: "quick".into(),
                status: "passed".into(),
                started_at: "2026-06-08T00:00:00Z".into(),
                finished_at: "2999-01-01T00:00:01Z".into(),
                duration_ms: 1000,
                steps: Vec::new(),
            })
            .unwrap();
        fs::create_dir_all(temp.path().join("target").join("release")).unwrap();
        fs::write(
            temp.path()
                .join("target")
                .join("release")
                .join(rayman_exe_name()),
            "binary",
        )
        .unwrap();
        let manager = release_manager(temp.path());

        let advisory = manager.generate("test", false).unwrap();
        assert_eq!(advisory.status, "ready");
        assert_eq!(advisory.provenance_status, "not_git");
        assert!(!advisory.provenance.completeness.external_distribution_ready);
        assert!(
            advisory
                .provenance
                .completeness
                .missing
                .contains(&"git_commit".into())
        );

        let required = manager
            .generate_with_options(ReleaseEvidenceOptions {
                label: "test".into(),
                require_provenance: true,
                ..ReleaseEvidenceOptions::default()
            })
            .unwrap();
        assert_eq!(required.status, "partial");
        assert!(
            required
                .required_actions
                .iter()
                .any(|action| action.contains("Git workspace"))
        );
    }

    #[test]
    fn release_evidence_reports_failed_latest_regression_as_partial() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("Cargo.lock"), "# lock").unwrap();
        fs::write(temp.path().join("Cargo.toml"), "[workspace]\n").unwrap();
        fs::write(temp.path().join("SKILL.md"), "# skill").unwrap();
        fs::create_dir_all(temp.path().join("target").join("release")).unwrap();
        fs::write(
            temp.path()
                .join("target")
                .join("release")
                .join(rayman_exe_name()),
            "binary",
        )
        .unwrap();
        RegressionHistoryManager::new(temp.path())
            .unwrap()
            .append(&RegressionRunRecord {
                id: "regression_full_1".into(),
                profile: "full".into(),
                status: "failed".into(),
                started_at: "2026-06-08T00:00:00Z".into(),
                finished_at: "2026-06-08T00:00:01Z".into(),
                duration_ms: 1000,
                steps: Vec::new(),
            })
            .unwrap();

        let report = release_manager(temp.path())
            .generate("test", false)
            .unwrap();

        assert_eq!(report.status, "partial");
        assert!(
            report.required_actions.iter().any(|action| {
                action.contains("latest regression history record must be passed")
            })
        );
    }

    #[test]
    fn release_evidence_reports_unreadable_regression_history_as_partial() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("Cargo.lock"), "# lock").unwrap();
        fs::write(temp.path().join("Cargo.toml"), "[workspace]\n").unwrap();
        fs::write(temp.path().join("SKILL.md"), "# skill").unwrap();
        fs::create_dir_all(temp.path().join("target").join("release")).unwrap();
        fs::write(
            temp.path()
                .join("target")
                .join("release")
                .join(rayman_exe_name()),
            "binary",
        )
        .unwrap();
        let history = RegressionHistoryManager::new(temp.path()).unwrap();
        fs::create_dir_all(history.history_path().parent().unwrap()).unwrap();
        fs::write(history.history_path(), "not-json\n").unwrap();

        let report = release_manager(temp.path())
            .generate("test", false)
            .unwrap();

        assert_eq!(report.status, "partial");
        assert!(report.latest_regression.is_none());
        assert!(
            report
                .required_actions
                .iter()
                .any(|action| action.contains("regression history unreadable"))
        );
    }

    #[test]
    fn release_evidence_blocks_failed_dependency_policy() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("Cargo.lock"), "# lock").unwrap();
        fs::write(temp.path().join("Cargo.toml"), "[workspace]\n").unwrap();
        fs::write(temp.path().join("SKILL.md"), "# skill").unwrap();
        fs::write(temp.path().join("deny.toml"), "[licenses]\n").unwrap();
        RegressionHistoryManager::new(temp.path())
            .unwrap()
            .append(&RegressionRunRecord {
                id: "regression_full_1".into(),
                profile: "full".into(),
                status: "passed".into(),
                started_at: "2026-06-08T00:00:00Z".into(),
                finished_at: "2999-01-01T00:00:01Z".into(),
                duration_ms: 1000,
                steps: Vec::new(),
            })
            .unwrap();
        fs::create_dir_all(temp.path().join("target").join("release")).unwrap();
        fs::write(
            temp.path()
                .join("target")
                .join("release")
                .join(rayman_exe_name()),
            "binary",
        )
        .unwrap();
        let manager = ReleaseEvidenceManager::with_dependency_policy_runner(
            temp.path(),
            Arc::new(StaticDependencyRunner {
                check_success: false,
            }),
        )
        .unwrap();

        let report = manager.generate("test", false).unwrap();

        assert_eq!(report.status, "partial");
        assert_eq!(report.provenance.dependency_policy.status, "blocked");
        assert!(
            report
                .required_actions
                .iter()
                .any(|action| action.contains("dependency policy"))
        );
    }
}
