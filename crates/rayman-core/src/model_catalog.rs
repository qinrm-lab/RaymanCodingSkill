use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::config::{ConfigManager, ModelRef, mapping_get};
use crate::{display_path, ensure_within, now_iso, sha256_file, write_text, yaml};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelCatalogStatus {
    pub workspace_path: String,
    pub cache_path: String,
    pub status: String,
    pub cache_present: bool,
    pub cache_valid: bool,
    pub cache_stale: bool,
    pub cache_generated_at: Option<String>,
    pub unknown_routes: Vec<String>,
    pub deprecated_routes: Vec<String>,
    pub required_actions: Vec<String>,
}

pub struct ModelCatalogManager {
    workspace: PathBuf,
    cache_path: PathBuf,
}

impl ModelCatalogManager {
    pub fn new(workspace: impl Into<PathBuf>) -> Result<Self> {
        let workspace = workspace.into().canonicalize()?;
        let cache_path = ensure_within(
            &workspace
                .join(".RaymanCodingSkill")
                .join("models")
                .join("catalog_cache.json"),
            &workspace,
            "model catalog cache must stay inside workspace",
        )?;
        Ok(Self {
            workspace,
            cache_path,
        })
    }

    pub fn refresh(&self, apply: bool) -> Result<Value> {
        let config = ConfigManager::new(&self.workspace)?;
        let snapshot = json!({
            "generated_at": now_iso(),
            "source": "local_config_snapshot",
            "source_hashes": self.source_hashes()?,
            "providers": config.model_catalog().cloned().unwrap_or(yaml::Value::Null),
            "route_findings": config.model_route_catalog_findings(),
            "source_policy": "Local model catalog cache records current config metadata only; provider pricing/capability data must be refreshed from provider docs before cost-sensitive routing."
        });
        if apply {
            if let Some(parent) = self.cache_path.parent() {
                fs::create_dir_all(parent)?;
            }
            write_text(&self.cache_path, &serde_json::to_string_pretty(&snapshot)?)?;
        }
        Ok(json!({
            "status": if apply { "applied" } else { "dry_run" },
            "cache_path": display_path(&self.cache_path),
            "snapshot": snapshot
        }))
    }

    pub fn status(&self) -> Result<ModelCatalogStatus> {
        let cache_required = self.cache_required();
        if !cache_required {
            let cache_present = self.cache_path.exists();
            let mut required_actions = Vec::new();
            let mut cache_valid = false;
            let mut cache_stale = false;
            let mut cache_generated_at = None;
            if cache_present {
                match self.read_cache() {
                    Ok(cache) => {
                        cache_valid = true;
                        cache_generated_at = cache
                            .get("generated_at")
                            .and_then(Value::as_str)
                            .map(str::to_string);
                        let cached_hashes =
                            cache.get("source_hashes").cloned().unwrap_or(Value::Null);
                        let current_hashes = self.source_hashes()?;
                        if cached_hashes != current_hashes {
                            cache_stale = true;
                            required_actions.push(
                                "rerun rayman models refresh --apply after model route config changes"
                                    .into(),
                            );
                        }
                    }
                    Err(error) => {
                        required_actions.push(format!("repair model catalog cache JSON: {error}"));
                    }
                }
            }
            return Ok(ModelCatalogStatus {
                workspace_path: display_path(&self.workspace),
                cache_path: display_path(&self.cache_path),
                status: if required_actions.is_empty() {
                    "passed".into()
                } else {
                    "blocked".into()
                },
                cache_present,
                cache_valid,
                cache_stale,
                cache_generated_at,
                unknown_routes: Vec::new(),
                deprecated_routes: Vec::new(),
                required_actions,
            });
        }
        let config = ConfigManager::new(&self.workspace)?;
        let unknown_routes = config
            .model_route_catalog_findings()
            .into_iter()
            .map(|finding| {
                format!(
                    "{} -> {} ({})",
                    finding.source, finding.model, finding.reason
                )
            })
            .collect::<Vec<_>>();
        let deprecated_routes = route_models(&config)
            .into_iter()
            .filter(|model| model_is_deprecated(&config, model))
            .map(|model| model.as_string())
            .collect::<Vec<_>>();
        let cache_present = self.cache_path.exists();
        let mut cache_valid = false;
        let mut cache_stale = false;
        let mut cache_generated_at = None;
        let mut required_actions = Vec::new();
        if cache_present {
            match self.read_cache() {
                Ok(cache) => {
                    cache_valid = true;
                    cache_generated_at = cache
                        .get("generated_at")
                        .and_then(Value::as_str)
                        .map(str::to_string);
                    let cached_hashes = cache.get("source_hashes").cloned().unwrap_or(Value::Null);
                    let current_hashes = self.source_hashes()?;
                    if cached_hashes != current_hashes {
                        cache_stale = true;
                        required_actions.push(
                            "rerun rayman models refresh --apply after model route config changes"
                                .into(),
                        );
                    }
                }
                Err(error) => {
                    required_actions.push(format!("repair model catalog cache JSON: {error}"));
                }
            }
        } else {
            required_actions.push(
                "run rayman models refresh --apply before trusting dynamic model catalog metadata"
                    .into(),
            );
        }
        for item in &unknown_routes {
            required_actions.push(format!("define route model in config/models.yaml: {item}"));
        }
        for item in &deprecated_routes {
            required_actions.push(format!(
                "remove deprecated model from automatic route/default: {item}"
            ));
        }
        Ok(ModelCatalogStatus {
            workspace_path: display_path(&self.workspace),
            cache_path: display_path(&self.cache_path),
            status: if required_actions.is_empty() {
                "passed".into()
            } else {
                "blocked".into()
            },
            cache_present,
            cache_valid,
            cache_stale,
            cache_generated_at,
            unknown_routes,
            deprecated_routes,
            required_actions,
        })
    }

    pub fn assert_passed(&self) -> Result<ModelCatalogStatus> {
        let status = self.status()?;
        if status.status != "passed" {
            bail!(
                "model catalog status blocked: {}",
                status.required_actions.join("; ")
            );
        }
        Ok(status)
    }

    fn cache_required(&self) -> bool {
        self.workspace.join("config").join("models.yaml").exists()
            || self
                .workspace
                .join("config")
                .join("default_config.yaml")
                .exists()
    }

    fn source_hashes(&self) -> Result<Value> {
        let mut hashes = serde_json::Map::new();
        for path in ["config/models.yaml", "config/default_config.yaml"] {
            let full_path = self.workspace.join(path);
            if full_path.exists() {
                hashes.insert(path.into(), Value::String(sha256_file(&full_path)?));
            }
        }
        Ok(Value::Object(hashes))
    }

    fn read_cache(&self) -> Result<Value> {
        let text = fs::read_to_string(&self.cache_path)
            .with_context(|| format!("无法读取模型目录缓存: {}", self.cache_path.display()))?;
        serde_json::from_str(&text)
            .with_context(|| format!("无法解析模型目录缓存: {}", self.cache_path.display()))
    }
}

fn route_models(config: &ConfigManager) -> Vec<ModelRef> {
    let mut out = vec![config.default_model()];
    for task in [
        "default",
        "code_generation",
        "code_review",
        "test_generation",
        "documentation",
        "research_planner",
        "research_scientist",
    ] {
        out.extend(config.route_candidates(Some(task), Some("auto"), None, false));
        out.extend(config.route_candidates(Some(task), Some("manual"), None, false));
    }
    out.sort_by_key(ModelRef::as_string);
    out.dedup_by_key(|model| model.as_string());
    out
}

fn model_is_deprecated(config: &ConfigManager, model: &ModelRef) -> bool {
    let Some(provider) = config.model_catalog_provider(&model.provider) else {
        return false;
    };
    let Some(models) = mapping_get(provider, "models") else {
        return false;
    };
    let Some(entry) = mapping_get(models, &model.model) else {
        return false;
    };
    mapping_get(entry, "catalog_status")
        .and_then(yaml::Value::as_str)
        .is_some_and(|status| status.eq_ignore_ascii_case("deprecated"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_status_blocks_unknown_route_model() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join("config")).unwrap();
        fs::write(
            temp.path().join("config").join("default_config.yaml"),
            r#"
config_files:
  models: "models.yaml"
default_model:
  type: openai
  name: missing
"#,
        )
        .unwrap();
        fs::write(
            temp.path().join("config").join("models.yaml"),
            "openai:\n  models: {}\n",
        )
        .unwrap();

        let status = ModelCatalogManager::new(temp.path())
            .unwrap()
            .status()
            .unwrap();

        assert_eq!(status.status, "blocked");
        assert!(!status.unknown_routes.is_empty());
    }

    #[test]
    fn model_status_blocks_deprecated_default_model() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join("config")).unwrap();
        fs::write(
            temp.path().join("config").join("default_config.yaml"),
            r#"
config_files:
  models: "models.yaml"
default_model:
  type: openai
  name: old-model
"#,
        )
        .unwrap();
        fs::write(
            temp.path().join("config").join("models.yaml"),
            r#"
openai:
  models:
    old-model:
      catalog_status: deprecated
"#,
        )
        .unwrap();
        let manager = ModelCatalogManager::new(temp.path()).unwrap();
        manager.refresh(true).unwrap();

        let status = manager.status().unwrap();

        assert_eq!(status.status, "blocked");
        assert_eq!(status.deprecated_routes, vec!["openai/old-model"]);
    }

    #[test]
    fn model_status_blocks_stale_catalog_cache() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join("config")).unwrap();
        fs::write(
            temp.path().join("config").join("default_config.yaml"),
            r#"
config_files:
  models: "models.yaml"
default_model:
  type: local
  name: codellama
"#,
        )
        .unwrap();
        fs::write(
            temp.path().join("config").join("models.yaml"),
            r#"
local:
  models:
    codellama:
      name: Code Llama
"#,
        )
        .unwrap();
        let manager = ModelCatalogManager::new(temp.path()).unwrap();
        manager.refresh(true).unwrap();
        fs::write(
            temp.path().join("config").join("models.yaml"),
            r#"
local:
  models:
    codellama:
      name: Code Llama Changed
"#,
        )
        .unwrap();

        let status = manager.status().unwrap();

        assert_eq!(status.status, "blocked");
        assert!(status.cache_stale);
        assert!(status.cache_valid);
    }
}
