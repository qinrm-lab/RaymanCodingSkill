use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Serialize;

use crate::file_io::write_atomic;
use crate::hash::sha256_file;
use crate::state_paths;

const ACTIVATION_RELATIVE: &str = "workspace_skill.yaml";
const SKILL_NAME: &str = "raymancodingskill";

#[derive(Debug, Clone, Serialize)]
pub struct WorkspaceActivationReport {
    pub status: String,
    pub active: bool,
    pub state_present: bool,
    pub config_present: bool,
    pub enabled: bool,
    pub skill: Option<String>,
    pub skill_file: Option<String>,
    pub cli_contract: Option<String>,
    pub cli_version: Option<String>,
    pub running_cli_contract: String,
    pub running_cli_version: String,
    pub expected_sha256: Option<String>,
    pub actual_sha256: Option<String>,
    pub issues: Vec<String>,
}

fn scalar(value: &str, line_number: usize) -> Result<String> {
    let value = value.trim();
    if value.is_empty() {
        bail!("workspace_skill.yaml 第 {line_number} 行缺少值");
    }
    let quoted = value.starts_with(['\'', '"']) || value.ends_with(['\'', '"']);
    if quoted {
        let Some(quote) = value.chars().next() else {
            unreachable!("empty activation scalar was rejected")
        };
        if value.len() < 2 || !value.ends_with(quote) {
            bail!("workspace_skill.yaml 第 {line_number} 行包含未闭合或不匹配的引号");
        }
        return Ok(value[1..value.len() - 1].to_string());
    }
    Ok(value.to_string())
}

fn parse_activation(text: &str) -> Result<BTreeMap<String, String>> {
    const ALLOWED_FIELDS: &[&str] = &[
        "skill",
        "enabled",
        "skill_file",
        "skill_sha256",
        "cli_contract",
        "cli_version",
    ];
    let mut fields = BTreeMap::new();
    for (index, raw) in text.lines().enumerate() {
        let line_number = index + 1;
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if raw.trim_start() != raw {
            bail!(
                "workspace_skill.yaml 第 {line_number} 行包含不受支持的缩进；激活合同只接受顶层标量"
            );
        }
        let (key, value) = line.split_once(':').ok_or_else(|| {
            anyhow::anyhow!("workspace_skill.yaml 第 {line_number} 行缺少 key/value 分隔符")
        })?;
        let key = key.trim();
        if !ALLOWED_FIELDS.contains(&key) {
            bail!("workspace_skill.yaml 第 {line_number} 行包含未知字段: {key}");
        }
        if fields.contains_key(key) {
            bail!("workspace_skill.yaml 第 {line_number} 行包含重复字段: {key}");
        }
        fields.insert(key.to_string(), scalar(value, line_number)?);
    }
    Ok(fields)
}
fn resolve_skill_file(root: &Path, value: &str) -> PathBuf {
    let path = Path::new(value);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    }
}
pub fn activation_status(root: &Path) -> Result<WorkspaceActivationReport> {
    let state = state_paths::managed_state_root(root, false)?;
    let state_present = state.is_some();
    let config_path = state_paths::managed_state_file(root, Path::new(ACTIVATION_RELATIVE), false)?;
    let config_present = match fs::symlink_metadata(&config_path) {
        Ok(metadata) if metadata.file_type().is_file() && !metadata.file_type().is_symlink() => {
            true
        }
        Ok(_) => bail!(
            "workspace_skill.yaml 必须是普通非链接文件: {}",
            config_path.display()
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => return Err(error).context("无法检查 workspace_skill.yaml"),
    };
    if !config_present {
        return Ok(WorkspaceActivationReport {
            status: if state_present {
                "orphan_state".into()
            } else {
                "inactive".into()
            },
            active: false,
            state_present,
            config_present: false,
            enabled: false,
            skill: None,
            skill_file: None,
            expected_sha256: None,
            cli_contract: None,
            cli_version: None,
            running_cli_contract: crate::CLI_CONTRACT.into(),
            running_cli_version: crate::CLI_VERSION.into(),
            actual_sha256: None,
            issues: if state_present {
                vec!["受管状态存在，但缺少显式 workspace_skill.yaml 激活合同".into()]
            } else {
                Vec::new()
            },
        });
    }

    let text = fs::read_to_string(&config_path)
        .with_context(|| format!("无法读取工作区激活合同: {}", config_path.display()))?;
    let fields = parse_activation(&text)?;
    let skill = fields.get("skill").cloned();
    let enabled = fields
        .get("enabled")
        .is_some_and(|value| value.eq_ignore_ascii_case("true"));
    let skill_file = fields.get("skill_file").cloned();
    let expected_sha256 = fields.get("skill_sha256").cloned();
    let mut issues = Vec::new();
    let cli_contract = fields.get("cli_contract").cloned();
    let cli_version = fields.get("cli_version").cloned();
    if cli_contract.as_deref() != Some(crate::CLI_CONTRACT) {
        issues.push(format!("cli_contract 必须精确为 {}", crate::CLI_CONTRACT));
    }
    if cli_version.as_deref() != Some(crate::CLI_VERSION) {
        issues.push(format!("cli_version 必须精确为 {}", crate::CLI_VERSION));
    }
    if skill.as_deref() != Some(SKILL_NAME) {
        issues.push(format!("skill 必须精确为 {SKILL_NAME}"));
    }
    if !enabled {
        issues.push("enabled 不是 true".into());
    }
    let actual_sha256 = if let Some(value) = skill_file.as_deref() {
        let path = resolve_skill_file(root, value);
        match fs::symlink_metadata(&path) {
            Ok(metadata)
                if metadata.file_type().is_file() && !metadata.file_type().is_symlink() =>
            {
                match sha256_file(&path) {
                    Ok(hash) => Some(hash),
                    Err(error) => {
                        issues.push(format!("无法哈希 skill_file {}: {error}", path.display()));
                        None
                    }
                }
            }
            Ok(_) => {
                issues.push(format!("skill_file 不是普通非链接文件: {}", path.display()));
                None
            }
            Err(error) => {
                issues.push(format!("skill_file 不可读取 {}: {error}", path.display()));
                None
            }
        }
    } else {
        issues.push("缺少 skill_file".into());
        None
    };
    match (expected_sha256.as_deref(), actual_sha256.as_deref()) {
        (Some(expected), Some(actual))
            if expected.len() == 64
                && expected
                    .chars()
                    .all(|character| character.is_ascii_hexdigit())
                && expected.eq_ignore_ascii_case(actual) => {}
        (Some(_), Some(_)) => issues.push("skill_sha256 与 skill_file 当前内容不一致".into()),
        (None, _) => issues.push("缺少 skill_sha256".into()),
        _ => {}
    }
    let active = issues.is_empty();
    Ok(WorkspaceActivationReport {
        status: if active {
            "active".into()
        } else if !enabled {
            "inactive".into()
        } else {
            "invalid".into()
        },
        active,
        state_present,
        config_present: true,
        enabled,
        skill,
        skill_file,
        expected_sha256,
        actual_sha256,
        cli_contract,
        cli_version,
        running_cli_contract: crate::CLI_CONTRACT.into(),
        running_cli_version: crate::CLI_VERSION.into(),
        issues,
    })
}

pub fn require_active(root: &Path) -> Result<WorkspaceActivationReport> {
    let report = activation_status(root)?;
    if !report.active {
        bail!(
            "RaymanCodingSkill 工作区未显式激活（status={}）：运行 `rayman workspace activate --skill-file <canonical-SKILL.md> --yes`；历史 .RaymanCodingSkill 状态不会自动激活 skill",
            report.status
        );
    }
    Ok(report)
}

pub fn activate(root: &Path, skill_file: &Path) -> Result<WorkspaceActivationReport> {
    let root = root.canonicalize().context("无法规范化工作区根")?;
    let lexical = if skill_file.is_absolute() {
        skill_file.to_path_buf()
    } else {
        root.join(skill_file)
    };
    let metadata = fs::symlink_metadata(&lexical)
        .with_context(|| format!("无法读取 canonical skill: {}", lexical.display()))?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        bail!(
            "canonical skill 必须是普通非链接文件: {}",
            lexical.display()
        );
    }
    let canonical = lexical.canonicalize()?;
    let hash = sha256_file(&canonical)?;
    let recorded_path = canonical
        .strip_prefix(&root)
        .map(|path| path.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| canonical.display().to_string());
    if recorded_path.contains(['\r', '\n']) {
        bail!("skill_file 路径不能包含换行");
    }
    let config = format!(
        "skill: {SKILL_NAME}\nenabled: true\nskill_file: {recorded_path}\nskill_sha256: {hash}\ncli_contract: {}\ncli_version: {}\n",
        crate::CLI_CONTRACT,
        crate::CLI_VERSION
    );
    let path =
        state_paths::managed_state_file(root.as_path(), Path::new(ACTIVATION_RELATIVE), true)?;
    write_atomic(&path, &config)?;
    let report = activation_status(&root)?;
    if !report.active {
        bail!("写入后的工作区激活合同仍无效: {}", report.issues.join("; "));
    }
    Ok(report)
}

pub fn deactivate(root: &Path) -> Result<WorkspaceActivationReport> {
    let path = state_paths::managed_state_file(root, Path::new(ACTIVATION_RELATIVE), true)?;
    let config = format!("skill: {SKILL_NAME}\nenabled: false\n");
    write_atomic(&path, &config)?;
    activation_status(root)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orphan_state_is_not_activation() {
        let root = tempfile::tempdir().unwrap();
        state_paths::managed_state_root(root.path(), true).unwrap();
        let report = activation_status(root.path()).unwrap();
        assert_eq!(report.status, "orphan_state");
        assert!(!report.active);
        assert!(require_active(root.path()).is_err());
    }

    #[test]
    fn activation_is_hash_bound_and_deactivation_is_explicit() {
        let root = tempfile::tempdir().unwrap();
        let skill = root.path().join("SKILL.md");
        fs::write(&skill, "canonical skill\n").unwrap();
        assert!(activate(root.path(), &skill).unwrap().active);

        fs::write(&skill, "changed skill\n").unwrap();
        let drifted = activation_status(root.path()).unwrap();
        assert_eq!(drifted.status, "invalid");
        assert!(!drifted.active);

        let inactive = deactivate(root.path()).unwrap();
        assert_eq!(inactive.status, "inactive");
        assert!(!inactive.active);
    }
    #[test]
    fn activation_contract_rejects_duplicate_unknown_and_malformed_fields() {
        for invalid in [
            "skill: raymancodingskill\nskill: raymancodingskill\nenabled: true\n",
            "skill: raymancodingskill\nenabled: true\nauto_use: true\n",
            "skill: raymancodingskill\n  enabled: true\n",
            "skill: \"raymancodingskill\nenabled: true\n",
            "skill raymancodingskill\nenabled: true\n",
        ] {
            assert!(parse_activation(invalid).is_err(), "accepted: {invalid:?}");
        }
    }

    #[test]
    fn activation_is_bound_to_the_running_cli_contract_and_version() {
        let root = tempfile::tempdir().unwrap();
        let skill = root.path().join("SKILL.md");
        fs::write(&skill, "canonical skill\n").unwrap();
        let hash = sha256_file(&skill).unwrap();
        let state = state_paths::managed_state_root(root.path(), true)
            .unwrap()
            .unwrap();
        fs::write(
            state.join(ACTIVATION_RELATIVE),
            format!(
                "skill: {SKILL_NAME}\nenabled: true\nskill_file: SKILL.md\nskill_sha256: {hash}\ncli_contract: rayman-cli-contract-v5\ncli_version: 2.1.0\n"
            ),
        )
        .unwrap();

        let report = activation_status(root.path()).unwrap();
        assert!(!report.active);
        assert_eq!(
            report.cli_contract.as_deref(),
            Some("rayman-cli-contract-v5")
        );
        assert_eq!(report.running_cli_contract, crate::CLI_CONTRACT);
        assert!(
            report
                .issues
                .iter()
                .any(|issue| issue.contains("cli_contract"))
        );
        assert!(
            report
                .issues
                .iter()
                .any(|issue| issue.contains("cli_version"))
        );
    }
}
