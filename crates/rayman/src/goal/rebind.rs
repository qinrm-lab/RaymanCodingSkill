use std::fs;
use std::path::{Component, Path, PathBuf};

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use super::validation::{is_sha256, parse_validation_command};
use super::{GoalDecisionContext, ParsedValidationCommand};
use crate::file_io::is_link_or_reparse;

pub const MAINTENANCE_CYCLE_REBIND_SCHEMA: &str = "rayman.maintenance-cycle-authority-rebind.v1";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReplacementAuthorityCommandRebind {
    pub schema: String,
    pub flag: String,
    pub archived_value: String,
    pub current_value: String,
    pub current_sha256: String,
}

fn maintenance_cycle_argument_index(command: &ParsedValidationCommand) -> Result<usize> {
    let matches = command
        .args
        .iter()
        .enumerate()
        .filter(|(_, argument)| argument.eq_ignore_ascii_case("-MaintenanceOrchestrationCycle"))
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    if matches.len() != 1 {
        bail!(
            "maintenance cycle rebind 要求 archived command 恰好包含一个 -MaintenanceOrchestrationCycle"
        );
    }
    let index = matches[0];
    if index + 1 >= command.args.len() || command.args[index + 1].starts_with('-') {
        bail!("-MaintenanceOrchestrationCycle 缺少独立路径参数");
    }
    Ok(index)
}

fn normalized_relative_path(value: &str) -> Result<&Path> {
    if value.is_empty() || value.contains('\\') {
        bail!("maintenance cycle rebind 路径必须是使用 / 的非空 workspace-relative 路径");
    }
    let relative = Path::new(value);
    if relative.is_absolute()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        bail!("maintenance cycle rebind 路径禁止 absolute、.、..、prefix 或 root component");
    }
    let normalized = relative
        .components()
        .filter_map(|component| match component {
            Component::Normal(segment) => Some(segment.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/");
    if normalized != value {
        bail!("maintenance cycle rebind 路径必须是唯一规范文本形式");
    }
    if !value.ends_with("-maintenance-review-cycle.json") {
        bail!("maintenance cycle rebind 只接受 cycle-qualified maintenance-review-cycle.json");
    }
    Ok(relative)
}

fn verified_workspace_relative_file(root: &Path, value: &str) -> Result<PathBuf> {
    let relative = normalized_relative_path(value)?;
    let canonical_root = root.canonicalize()?;
    let mut current = canonical_root.clone();
    for component in relative.components() {
        let Component::Normal(segment) = component else {
            unreachable!("components were validated above")
        };
        current.push(segment);
        let metadata = fs::symlink_metadata(&current)?;
        if is_link_or_reparse(&metadata) {
            bail!(
                "maintenance cycle rebind 路径不得经过 symlink/junction/reparse: {}",
                current.display()
            );
        }
    }
    let metadata = fs::symlink_metadata(&current)?;
    if !metadata.is_file() {
        bail!(
            "maintenance cycle rebind 目标不是普通文件: {}",
            current.display()
        );
    }
    let canonical = current.canonicalize()?;
    canonical.strip_prefix(&canonical_root).map_err(|_| {
        anyhow::anyhow!(
            "maintenance cycle rebind 目标逃逸 workspace: {}",
            canonical.display()
        )
    })?;
    Ok(canonical)
}

pub(crate) fn verified_maintenance_cycle_rebind_path(
    root: &Path,
    rebind: &ReplacementAuthorityCommandRebind,
) -> Result<PathBuf> {
    if rebind.schema != MAINTENANCE_CYCLE_REBIND_SCHEMA
        || !is_sha256(&rebind.current_sha256)
        || rebind.current_sha256 != rebind.current_sha256.to_ascii_lowercase()
    {
        bail!("maintenance cycle rebind 结构无效");
    }
    verified_workspace_relative_file(root, &rebind.current_value)
}

pub fn prepare_maintenance_cycle_rebind(
    root: &Path,
    command: &str,
    current_value: &str,
) -> Result<(ParsedValidationCommand, ReplacementAuthorityCommandRebind)> {
    let parsed = parse_validation_command(command)?;
    let index = maintenance_cycle_argument_index(&parsed)?;
    let archived_value = parsed.args[index + 1].clone();
    if !archived_value.ends_with("-maintenance-review-cycle.json") {
        bail!("archived -MaintenanceOrchestrationCycle 不是 cycle-qualified JSON 路径");
    }
    let current_path = verified_workspace_relative_file(root, current_value)?;
    let rebind = ReplacementAuthorityCommandRebind {
        schema: MAINTENANCE_CYCLE_REBIND_SCHEMA.into(),
        flag: parsed.args[index].clone(),
        archived_value,
        current_value: current_value.into(),
        current_sha256: crate::hash::sha256_file(&current_path)?,
    };
    let effective = replacement_authority_effective_command(command, Some(&rebind))?;
    Ok((effective, rebind))
}

pub fn replacement_authority_effective_command(
    command: &str,
    rebind: Option<&ReplacementAuthorityCommandRebind>,
) -> Result<ParsedValidationCommand> {
    let mut parsed = parse_validation_command(command)?;
    let Some(rebind) = rebind else {
        return Ok(parsed);
    };
    if rebind.schema != MAINTENANCE_CYCLE_REBIND_SCHEMA
        || !rebind
            .flag
            .eq_ignore_ascii_case("-MaintenanceOrchestrationCycle")
        || !is_sha256(&rebind.current_sha256)
        || rebind.current_sha256 != rebind.current_sha256.to_ascii_lowercase()
        || !rebind
            .archived_value
            .ends_with("-maintenance-review-cycle.json")
    {
        bail!("maintenance cycle rebind 结构无效");
    }
    normalized_relative_path(&rebind.current_value)?;
    let index = maintenance_cycle_argument_index(&parsed)?;
    if parsed.args[index] != rebind.flag || parsed.args[index + 1] != rebind.archived_value {
        bail!("maintenance cycle rebind 未精确绑定 archived command 的 flag/value");
    }
    parsed.args[index + 1] = rebind.current_value.clone();
    Ok(parsed)
}

pub fn verify_maintenance_cycle_rebind_artifact(
    root: &Path,
    rebind: &ReplacementAuthorityCommandRebind,
) -> Result<()> {
    let path = verified_maintenance_cycle_rebind_path(root, rebind)?;
    let (bytes, _) =
        crate::file_io::read_handle_bound_file(&path, "maintenance cycle rebind artifact")?;
    let current = crate::hash::sha256_bytes(&bytes);
    if current != rebind.current_sha256 {
        bail!("maintenance cycle rebind 文件 hash 已漂移");
    }
    Ok(())
}

/// Capture-only counterpart for readiness decisions. The capture has already
/// resolved and handle-read the artifact, so reopening the path here would let
/// an A/B mutation influence a decision made about the earlier snapshot.
pub(crate) fn verify_maintenance_cycle_rebind_artifact_with_context(
    decision: &GoalDecisionContext<'_>,
    rebind: &ReplacementAuthorityCommandRebind,
) -> Result<()> {
    if rebind.schema != MAINTENANCE_CYCLE_REBIND_SCHEMA
        || !is_sha256(&rebind.current_sha256)
        || rebind.current_sha256 != rebind.current_sha256.to_ascii_lowercase()
    {
        bail!("maintenance cycle rebind 结构无效");
    }
    normalized_relative_path(&rebind.current_value)?;
    match decision.captured_maintenance_artifact_hash(&rebind.current_value)? {
        Some(hash) if hash == rebind.current_sha256 => Ok(()),
        Some(_) => bail!("captured maintenance cycle rebind 文件 hash 已漂移"),
        None => bail!("captured maintenance cycle rebind artifact 缺失"),
    }
}
