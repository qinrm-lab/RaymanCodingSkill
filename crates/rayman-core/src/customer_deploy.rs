use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::yaml::{self, Value as YamlValue};
use crate::{display_path, ensure_within, now_iso, write_text};

const CONFIG_RELATIVE_PATH: &str = ".RaymanCodingSkill/customer_deploy.yaml";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct CredentialRef {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_ref: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct CustomerDeployConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_command: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub test_commands: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deploy_command: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifact_paths: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_alias: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rollback_command: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub credential_refs: Vec<CredentialRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, YamlValue>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeployConfigValidationReport {
    pub status: String,
    pub config_path: String,
    pub present: bool,
    pub missing_required: Vec<String>,
    pub warnings: Vec<String>,
    pub sanitized_config: Value,
}

#[derive(Debug, Clone, Default)]
pub struct CustomerDeployUpdate {
    pub environment: Option<String>,
    pub build_command: Option<String>,
    pub test_commands: Vec<String>,
    pub deploy_command: Option<String>,
    pub artifact_paths: Vec<String>,
    pub target_alias: Option<String>,
    pub rollback_command: Option<String>,
    pub credential_refs: Vec<CredentialRef>,
    pub notes: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CustomerDeployManager {
    pub workspace: PathBuf,
    pub config_path: PathBuf,
}

impl CustomerDeployManager {
    pub fn new(workspace: impl Into<PathBuf>) -> Result<Self> {
        let workspace = workspace
            .into()
            .canonicalize()
            .context("无法解析工作区路径")?;
        let config_path = ensure_within(
            &workspace.join(CONFIG_RELATIVE_PATH),
            &workspace,
            "客户发布配置必须位于工作区内",
        )?;
        Ok(Self {
            workspace,
            config_path,
        })
    }

    pub fn read(&self) -> Result<Option<CustomerDeployConfig>> {
        if !self.config_path.exists() {
            return Ok(None);
        }
        let text = fs::read_to_string(&self.config_path)
            .with_context(|| format!("无法读取客户发布配置: {}", self.config_path.display()))?;
        let config: CustomerDeployConfig = yaml::from_str(&text).context("无法解析客户发布配置")?;
        validate_config(&config)?;
        Ok(Some(config))
    }

    pub fn set(&self, update: CustomerDeployUpdate) -> Result<CustomerDeployConfig> {
        validate_update(&update)?;
        let mut config = self.read()?.unwrap_or_default();
        apply_string(&mut config.environment, update.environment);
        apply_string(&mut config.build_command, update.build_command);
        if !update.test_commands.is_empty() {
            config.test_commands = update.test_commands;
        }
        apply_string(&mut config.deploy_command, update.deploy_command);
        if !update.artifact_paths.is_empty() {
            config.artifact_paths = update.artifact_paths;
        }
        apply_string(&mut config.target_alias, update.target_alias);
        apply_string(&mut config.rollback_command, update.rollback_command);
        if !update.credential_refs.is_empty() {
            config.credential_refs = update.credential_refs;
        }
        apply_string(&mut config.notes, update.notes);
        config.updated_at = Some(now_iso());
        self.write(&config)?;
        Ok(config)
    }

    pub fn unset(&self, key: &str) -> Result<CustomerDeployConfig> {
        let mut config = self.read()?.unwrap_or_default();
        match key {
            "environment" | "env" => config.environment = None,
            "build_command" | "build" => config.build_command = None,
            "test_commands" | "test" => config.test_commands.clear(),
            "deploy_command" | "deploy" => config.deploy_command = None,
            "artifact_paths" | "artifact" => config.artifact_paths.clear(),
            "target_alias" | "target" => config.target_alias = None,
            "rollback_command" | "rollback" => config.rollback_command = None,
            "credential_refs" | "credential" | "credentials" => config.credential_refs.clear(),
            "notes" => config.notes = None,
            other => bail!("不支持的客户发布配置键: {other}"),
        }
        config.updated_at = Some(now_iso());
        self.write(&config)?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<DeployConfigValidationReport> {
        let config = self.read()?;
        Ok(validation_report(
            &self.config_path,
            config.as_ref(),
            config.is_some(),
        ))
    }

    pub fn status(&self) -> Result<Value> {
        let report = self.validate()?;
        Ok(json!({
            "workspace_path": display_path(&self.workspace),
            "config_path": report.config_path,
            "present": report.present,
            "status": report.status,
            "missing_required": report.missing_required,
            "warnings": report.warnings,
            "config": report.sanitized_config,
        }))
    }

    pub fn goal_metadata(&self, goal: &str) -> Result<Option<Value>> {
        if !is_deploy_goal(goal) {
            return Ok(None);
        }
        let report = self.validate()?;
        Ok(Some(json!({
            "customer_deploy": {
                "detected": true,
                "status": report.status,
                "config_path": report.config_path,
                "present": report.present,
                "missing_required": report.missing_required,
                "warnings": report.warnings,
                "config": report.sanitized_config,
            }
        })))
    }

    fn write(&self, config: &CustomerDeployConfig) -> Result<()> {
        write_text(&self.config_path, &yaml::to_string(config)?)
    }
}

pub fn is_deploy_goal(goal: &str) -> bool {
    let lower = goal.to_ascii_lowercase();
    ["deploy", "deployment", "release", "publish"]
        .iter()
        .any(|term| lower.contains(term))
        || goal.contains("发布")
        || goal.contains("部署")
        || goal.contains("上线")
}

pub fn validation_report(
    config_path: &std::path::Path,
    config: Option<&CustomerDeployConfig>,
    present: bool,
) -> DeployConfigValidationReport {
    let mut missing_required = Vec::new();
    let mut warnings = Vec::new();
    if let Some(config) = config {
        if blank(&config.environment) {
            missing_required.push("environment".into());
        }
        if blank(&config.build_command) {
            missing_required.push("build_command".into());
        }
        if config
            .test_commands
            .iter()
            .all(|item| item.trim().is_empty())
        {
            missing_required.push("test_commands".into());
        }
        if blank(&config.deploy_command) {
            missing_required.push("deploy_command".into());
        }
        if config.credential_refs.is_empty() {
            warnings.push(
                "credential_refs is empty; deployment may still need external credentials".into(),
            );
        }
    } else {
        missing_required.extend([
            "environment".into(),
            "build_command".into(),
            "test_commands".into(),
            "deploy_command".into(),
        ]);
    }
    let status = if missing_required.is_empty() {
        "ready"
    } else if present {
        "attention"
    } else {
        "missing"
    };
    DeployConfigValidationReport {
        status: status.into(),
        config_path: display_path(config_path),
        present,
        missing_required,
        warnings,
        sanitized_config: config.map(sanitize_config).unwrap_or(Value::Null),
    }
}

fn sanitize_config(config: &CustomerDeployConfig) -> Value {
    json!({
        "environment": sanitize_optional_string(&config.environment),
        "build_command": sanitize_optional_string(&config.build_command),
        "test_commands": sanitize_string_vec(&config.test_commands),
        "deploy_command": sanitize_optional_string(&config.deploy_command),
        "artifact_paths": sanitize_string_vec(&config.artifact_paths),
        "target_alias": sanitize_optional_string(&config.target_alias),
        "rollback_command": sanitize_optional_string(&config.rollback_command),
        "credential_refs": config.credential_refs.iter().map(|item| json!({
            "env": item.env.as_ref().map(|value| mask_ref(value)),
            "credential_ref": item.credential_ref.as_ref().map(|value| mask_ref(value)),
        })).collect::<Vec<_>>(),
        "notes": sanitize_optional_string(&config.notes),
        "updated_at": config.updated_at,
    })
}

fn sanitize_optional_string(value: &Option<String>) -> Option<String> {
    value.as_deref().map(sanitize_string_value)
}

fn sanitize_string_vec(values: &[String]) -> Vec<String> {
    values
        .iter()
        .map(|value| sanitize_string_value(value))
        .collect()
}

fn sanitize_string_value(value: &str) -> String {
    if looks_like_secret_value(value) {
        "<redacted-secret-like-value>".into()
    } else {
        value.to_string()
    }
}

fn mask_ref(value: &str) -> String {
    if value.len() <= 4 {
        return "****".into();
    }
    format!("{}****", &value[..value.len().min(4)])
}

fn apply_string(target: &mut Option<String>, value: Option<String>) {
    if let Some(value) = value
        && !value.trim().is_empty()
    {
        *target = Some(value);
    }
}

fn validate_config(config: &CustomerDeployConfig) -> Result<()> {
    for value in config
        .environment
        .iter()
        .chain(config.build_command.iter())
        .chain(config.deploy_command.iter())
        .chain(config.target_alias.iter())
        .chain(config.rollback_command.iter())
        .chain(config.notes.iter())
        .chain(config.test_commands.iter())
        .chain(config.artifact_paths.iter())
    {
        reject_secret_value(value)?;
    }
    for credential in &config.credential_refs {
        let has_ref = credential
            .env
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
            || credential
                .credential_ref
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty());
        if !has_ref {
            bail!("密钥引用必须包含 env 或 credential_ref");
        }
        if let Some(env) = &credential.env {
            validate_reference_name(env, "env")?;
        }
        if let Some(reference) = &credential.credential_ref {
            validate_reference_name(reference, "credential_ref")?;
        }
    }
    for value in config.extra.values() {
        reject_yaml_secret_values(value)?;
    }
    Ok(())
}

fn validate_update(update: &CustomerDeployUpdate) -> Result<()> {
    for value in update
        .environment
        .iter()
        .chain(update.build_command.iter())
        .chain(update.deploy_command.iter())
        .chain(update.target_alias.iter())
        .chain(update.rollback_command.iter())
        .chain(update.notes.iter())
        .chain(update.test_commands.iter())
        .chain(update.artifact_paths.iter())
    {
        reject_secret_value(value)?;
    }
    for credential in &update.credential_refs {
        let has_ref = credential
            .env
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
            || credential
                .credential_ref
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty());
        if !has_ref {
            bail!("密钥引用必须包含 env 或 credential_ref");
        }
        if let Some(env) = &credential.env {
            validate_reference_name(env, "env")?;
        }
        if let Some(reference) = &credential.credential_ref {
            validate_reference_name(reference, "credential_ref")?;
        }
    }
    Ok(())
}

fn validate_reference_name(value: &str, field: &str) -> Result<()> {
    reject_secret_value(value)?;
    let ok = value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | ':'));
    if !ok || value.trim().is_empty() {
        bail!("{field} 只能保存凭据引用名，不能保存真实密钥值");
    }
    Ok(())
}

fn reject_secret_value(value: &str) -> Result<()> {
    if looks_like_secret_value(value) {
        bail!("客户发布配置拒绝保存疑似真实密钥；请改用 env 或 credential_ref 引用");
    }
    Ok(())
}

fn looks_like_secret_value(value: &str) -> bool {
    let trimmed = value.trim();
    let lower = trimmed.to_ascii_lowercase();
    lower.starts_with("sk-")
        || lower.starts_with("ghp_")
        || lower.starts_with("xoxb-")
        || lower.contains("password=")
        || lower.contains("token=")
        || lower.contains("secret=")
        || lower.contains("apikey=")
        || (trimmed.len() >= 32
            && trimmed
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.')))
}

fn reject_yaml_secret_values(value: &YamlValue) -> Result<()> {
    match value {
        YamlValue::String(text) => reject_secret_value(text),
        YamlValue::Sequence(items) => {
            for item in items {
                reject_yaml_secret_values(item)?;
            }
            Ok(())
        }
        YamlValue::Mapping(mapping) => {
            for (key, value) in mapping {
                reject_yaml_secret_values(key)?;
                reject_yaml_secret_values(value)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn blank(value: &Option<String>) -> bool {
    value
        .as_deref()
        .map(str::trim)
        .unwrap_or_default()
        .is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_round_trips_and_preserves_unknown_fields() {
        let temp = tempfile::tempdir().unwrap();
        let manager = CustomerDeployManager::new(temp.path()).unwrap();
        fs::create_dir_all(manager.config_path.parent().unwrap()).unwrap();
        fs::write(
            &manager.config_path,
            "custom_field: keep\nenvironment: old\n",
        )
        .unwrap();

        let config = manager
            .set(CustomerDeployUpdate {
                environment: Some("prod".into()),
                build_command: Some("cargo build --release".into()),
                test_commands: vec!["cargo test".into()],
                deploy_command: Some("scripts/deploy.ps1".into()),
                credential_refs: vec![CredentialRef {
                    env: Some("PROD_TOKEN".into()),
                    credential_ref: None,
                }],
                ..Default::default()
            })
            .unwrap();

        assert_eq!(config.environment.as_deref(), Some("prod"));
        assert_eq!(
            config.extra.get("custom_field").and_then(YamlValue::as_str),
            Some("keep")
        );
        let saved = manager.read().unwrap().unwrap();
        assert_eq!(saved.test_commands, vec!["cargo test"]);
    }

    #[test]
    fn rejects_probable_secret_values_but_allows_refs() {
        let temp = tempfile::tempdir().unwrap();
        let manager = CustomerDeployManager::new(temp.path()).unwrap();
        let bad = manager.set(CustomerDeployUpdate {
            credential_refs: vec![CredentialRef {
                env: Some("sk-abcdefghijklmnopqrstuvwxyz".into()),
                credential_ref: None,
            }],
            ..Default::default()
        });
        assert!(bad.is_err());

        manager
            .set(CustomerDeployUpdate {
                credential_refs: vec![CredentialRef {
                    env: Some("PROD_TOKEN".into()),
                    credential_ref: Some("aliyun-prod".into()),
                }],
                ..Default::default()
            })
            .unwrap();
    }

    #[test]
    fn hand_written_config_rejects_secret_values_without_leaking_them() {
        let temp = tempfile::tempdir().unwrap();
        let manager = CustomerDeployManager::new(temp.path()).unwrap();
        fs::create_dir_all(manager.config_path.parent().unwrap()).unwrap();
        fs::write(
            &manager.config_path,
            "environment: prod\ndeploy_command: deploy --token=super-secret-token\nbuild_command: cargo build --release\ntest_commands:\n  - cargo test\n",
        )
        .unwrap();

        let error = manager.validate().unwrap_err().to_string();

        assert!(error.contains("疑似真实密钥"));
        assert!(!error.contains("super-secret-token"));
    }

    #[test]
    fn sanitize_config_redacts_secret_like_inline_fields() {
        let config = CustomerDeployConfig {
            deploy_command: Some("deploy --token=super-secret-token".into()),
            test_commands: vec!["cargo test".into()],
            ..Default::default()
        };

        let value = sanitize_config(&config);

        assert_eq!(
            value["deploy_command"].as_str(),
            Some("<redacted-secret-like-value>")
        );
        assert_eq!(value["test_commands"][0].as_str(), Some("cargo test"));
    }

    #[test]
    fn validation_reports_missing_required_fields() {
        let temp = tempfile::tempdir().unwrap();
        let manager = CustomerDeployManager::new(temp.path()).unwrap();
        let report = manager.validate().unwrap();

        assert_eq!(report.status, "missing");
        assert!(report.missing_required.contains(&"build_command".into()));
    }

    #[test]
    fn deploy_goal_detection_does_not_treat_generic_ship_feature_as_deploy() {
        assert!(!is_deploy_goal("ship feature"));
        assert!(is_deploy_goal("deploy customer project"));
        assert!(is_deploy_goal("发布客户项目"));
    }
}
