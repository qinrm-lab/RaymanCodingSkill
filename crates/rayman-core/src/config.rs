use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::assets::AssetRetirementManager;
use crate::yaml::{self, Mapping, Value};
use crate::{now_iso, sha256_file};

const AUXILIARY_SETTINGS_TIMEOUT_SECONDS: u64 = 120;
pub const DEFAULT_AUXILIARY_PROVIDER_TIMEOUT_SECONDS: u64 = 120;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelRef {
    pub provider: String,
    pub model: String,
}

impl ModelRef {
    pub fn parse(value: &str) -> Result<Self> {
        let (provider, model) = value
            .split_once('/')
            .with_context(|| format!("模型引用必须是 provider/model: {value}"))?;
        Ok(Self {
            provider: provider.to_string(),
            model: model.to_string(),
        })
    }

    pub fn as_string(&self) -> String {
        format!("{}/{}", self.provider, self.model)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelCatalogFinding {
    pub source: String,
    pub model: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderProxyConfig {
    Direct,
    Env,
    Http { url: String },
}

impl ProviderProxyConfig {
    pub fn mode(&self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::Env => "env",
            Self::Http { .. } => "http",
        }
    }

    pub fn url(&self) -> Option<&str> {
        match self {
            Self::Http { url } => Some(url),
            Self::Direct | Self::Env => None,
        }
    }

    pub fn as_json(&self) -> serde_json::Value {
        serde_json::json!({
            "mode": self.mode(),
            "url": self.url(),
        })
    }
}

#[derive(Debug, Clone)]
pub struct ConfigManager {
    pub root: PathBuf,
    pub config_path: PathBuf,
    pub config: Value,
    pub referenced: HashMap<String, Value>,
}

impl ConfigManager {
    pub fn new(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        let local_config = root.join("config").join("default_config.yaml");
        if local_config.exists() {
            return Self::from_path_with_root(local_config, root);
        }
        if let Some(canonical_config) = workspace_canonical_config_path(&root)? {
            return Self::from_path_with_root(canonical_config, root);
        }
        Self::from_path_with_root(local_config, root)
    }

    pub fn from_path(config_path: impl Into<PathBuf>) -> Result<Self> {
        let config_path = config_path.into();
        let root = config_path
            .parent()
            .and_then(Path::parent)
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        Self::from_path_with_root(config_path, root)
    }

    pub fn from_path_with_root(
        config_path: impl Into<PathBuf>,
        root: impl Into<PathBuf>,
    ) -> Result<Self> {
        let config_path = config_path.into();
        let root = root.into();
        assert_current_config_asset(&root, &config_path)?;
        let mut config = load_yaml(&config_path)?;
        upgrade_auxiliary_config_format(&config_path, &mut config)?;
        let mut manager = Self {
            root,
            config_path,
            config,
            referenced: HashMap::new(),
        };
        manager.load_referenced()?;
        Ok(manager)
    }

    pub fn config_dir(&self) -> PathBuf {
        self.config_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.root.join("config"))
    }

    pub fn get(&self, path: &str) -> Option<&Value> {
        get_path(&self.config, path)
    }

    pub fn get_owned(&self, path: &str) -> Option<Value> {
        self.get(path).cloned()
    }

    pub fn set(&mut self, path: &str, value: Value) -> Result<()> {
        set_path(&mut self.config, path, value)
    }

    pub fn save(&self) -> Result<()> {
        save_yaml(&self.config_path, &self.config)
    }

    pub fn model_catalog(&self) -> Option<&Value> {
        self.referenced.get("models")
    }

    pub fn model_catalog_provider(&self, provider: &str) -> Option<&Value> {
        self.model_catalog()
            .and_then(|catalog| mapping_get(catalog, provider))
    }

    pub fn model_catalog_contains(&self, model_ref: &ModelRef) -> bool {
        self.model_catalog_provider(&model_ref.provider)
            .and_then(|provider| mapping_get(provider, "models"))
            .and_then(|models| mapping_get(models, &model_ref.model))
            .is_some()
    }

    pub fn model_route_catalog_findings(&self) -> Vec<ModelCatalogFinding> {
        let mut findings = Vec::new();
        self.push_model_catalog_finding("default_model", &self.default_model(), &mut findings);

        let Some(routes) =
            get_path(&self.config, "model_routing.routes").and_then(Value::as_mapping)
        else {
            return findings;
        };

        for (task_key, route) in routes {
            let task = task_key.as_str().unwrap_or("<unknown>");
            for key in ["manual", "primary"] {
                if let Some(model) = mapping_get(route, key).and_then(Value::as_str) {
                    self.validate_model_ref_string(
                        &format!("model_routing.routes.{task}.{key}"),
                        model,
                        &mut findings,
                    );
                }
            }
            if let Some(fallbacks) = mapping_get(route, "fallback").and_then(Value::as_sequence) {
                for (index, item) in fallbacks.iter().enumerate() {
                    if let Some(model) = item.as_str() {
                        self.validate_model_ref_string(
                            &format!("model_routing.routes.{task}.fallback[{index}]"),
                            model,
                            &mut findings,
                        );
                    }
                }
            }
        }

        findings
    }

    fn validate_model_ref_string(
        &self,
        source: &str,
        model: &str,
        findings: &mut Vec<ModelCatalogFinding>,
    ) {
        match ModelRef::parse(model) {
            Ok(model_ref) => self.push_model_catalog_finding(source, &model_ref, findings),
            Err(error) => findings.push(ModelCatalogFinding {
                source: source.to_string(),
                model: model.to_string(),
                reason: error.to_string(),
            }),
        }
    }

    fn push_model_catalog_finding(
        &self,
        source: &str,
        model_ref: &ModelRef,
        findings: &mut Vec<ModelCatalogFinding>,
    ) {
        if self.model_catalog_contains(model_ref) {
            return;
        }
        let reason = if self.model_catalog_provider(&model_ref.provider).is_some() {
            format!(
                "model `{}` is not listed under provider `{}` in config/models.yaml",
                model_ref.model, model_ref.provider
            )
        } else {
            format!(
                "provider `{}` is not listed in config/models.yaml",
                model_ref.provider
            )
        };
        findings.push(ModelCatalogFinding {
            source: source.to_string(),
            model: model_ref.as_string(),
            reason,
        });
    }

    pub fn auxiliary_config(&self) -> Option<&Value> {
        self.referenced.get("auxiliary_ai")
    }

    pub fn model_config(&self, provider: &str) -> Value {
        mapping_get_path(&self.config, &["models", provider])
            .or_else(|| {
                self.auxiliary_config()
                    .and_then(|value| mapping_get_path(value, &["models", provider]))
            })
            .cloned()
            .unwrap_or(Value::Mapping(Mapping::new()))
    }

    pub fn provider_adapter(&self, provider: &str) -> String {
        get_path(&self.model_config(provider), "adapter")
            .and_then(Value::as_str)
            .unwrap_or(provider)
            .to_string()
    }

    pub fn api_key_env(&self, provider: &str) -> Option<String> {
        get_path(&self.model_config(provider), "api_key_env")
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    pub fn api_key_value(&self, provider: &str) -> Option<String> {
        get_path(&self.model_config(provider), "api_key")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .map(str::to_string)
    }

    pub fn auth_required(&self, provider: &str) -> bool {
        get_path(&self.model_config(provider), "auth_required")
            .and_then(Value::as_bool)
            .unwrap_or(true)
    }

    pub fn provider_wire_api(&self, provider: &str) -> String {
        get_path(&self.model_config(provider), "wire_api")
            .and_then(Value::as_str)
            .unwrap_or("chat_completions")
            .to_string()
    }

    pub fn provider_model_reasoning_effort(&self, provider: &str) -> Option<String> {
        get_path(&self.model_config(provider), "model_reasoning_effort")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .map(str::to_string)
    }

    pub fn provider_personality(&self, provider: &str) -> Option<String> {
        get_path(&self.model_config(provider), "personality")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .map(str::to_string)
    }

    pub fn provider_supports_websockets(&self, provider: &str) -> bool {
        get_path(&self.model_config(provider), "supports_websockets")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    }

    pub fn provider_requires_openai_auth(&self, provider: &str) -> bool {
        get_path(&self.model_config(provider), "requires_openai_auth")
            .and_then(Value::as_bool)
            .unwrap_or(true)
    }

    pub fn base_url(&self, provider: &str) -> Option<String> {
        let env_name = format!("{}_BASE_URL", provider.to_ascii_uppercase());
        if let Ok(value) = std::env::var(env_name)
            && !value.trim().is_empty()
        {
            return Some(value);
        }
        get_path(&self.model_config(provider), "base_url")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| {
                self.model_catalog_provider(provider)
                    .and_then(|v| mapping_get(v, "base_url"))
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
    }

    pub fn timeout_seconds(&self, provider: &str) -> u64 {
        get_path(&self.model_config(provider), "timeout")
            .and_then(Value::as_u64)
            .unwrap_or(60)
    }

    pub fn auxiliary_default_timeout_seconds(&self) -> u64 {
        self.auxiliary_value("default_timeout")
            .and_then(Value::as_u64)
            .unwrap_or(DEFAULT_AUXILIARY_PROVIDER_TIMEOUT_SECONDS)
    }

    pub fn auxiliary_timeout_seconds(&self, provider: &str) -> u64 {
        get_path(&self.model_config(provider), "timeout")
            .and_then(Value::as_u64)
            .unwrap_or_else(|| self.auxiliary_default_timeout_seconds())
    }

    pub fn provider_proxy_config(&self, provider: &str) -> Result<ProviderProxyConfig> {
        let config = self.model_config(provider);
        let Some(proxy) = get_path(&config, "proxy") else {
            return Ok(ProviderProxyConfig::Direct);
        };
        let Some(mode) = get_path(proxy, "mode").and_then(Value::as_str) else {
            bail!("provider {provider} proxy 配置必须显式声明 mode");
        };
        match mode {
            "direct" => Ok(ProviderProxyConfig::Direct),
            "env" => Ok(ProviderProxyConfig::Env),
            "http" => {
                let url = get_path(proxy, "url")
                    .and_then(Value::as_str)
                    .filter(|url| !url.trim().is_empty())
                    .with_context(|| format!("provider {provider} proxy.mode=http 必须配置 url"))?;
                Ok(ProviderProxyConfig::Http {
                    url: url.to_string(),
                })
            }
            other => bail!("provider {provider} proxy.mode 不支持: {other}"),
        }
    }

    pub fn provider_trust_level(&self, provider: &str) -> String {
        let config = self.model_config(provider);
        get_path(&config, "trust_level")
            .or_else(|| get_path(&config, "data_policy.trust_level"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| {
                if self
                    .base_url(provider)
                    .map(|url| is_loopback_url(&url))
                    .unwrap_or(false)
                {
                    "local_loopback".into()
                } else {
                    "untrusted".into()
                }
            })
    }

    pub fn provider_allows_workspace_data(&self, provider: &str) -> bool {
        let config = self.model_config(provider);
        get_path(&config, "allow_workspace_data")
            .or_else(|| get_path(&config, "data_policy.allow_workspace_data"))
            .and_then(Value::as_bool)
            .unwrap_or_else(|| {
                self.base_url(provider)
                    .map(|url| is_loopback_url(&url))
                    .unwrap_or(false)
            })
    }

    pub fn provider_target_report(&self, provider: &str) -> serde_json::Value {
        let base_url = self.base_url(provider);
        let proxy = self
            .provider_proxy_config(provider)
            .map(|proxy| proxy.as_json())
            .unwrap_or_else(|error| serde_json::json!({"error": error.to_string()}));
        serde_json::json!({
            "provider": provider,
            "base_url": base_url,
            "adapter": self.provider_adapter(provider),
            "wire_api": self.provider_wire_api(provider),
            "auth_required": self.auth_required(provider),
            "requires_openai_auth": self.provider_requires_openai_auth(provider),
            "timeout_seconds": self.auxiliary_timeout_seconds(provider),
            "proxy": proxy,
            "trust_level": self.provider_trust_level(provider),
            "allow_workspace_data": self.provider_allows_workspace_data(provider),
            "supports_websockets": self.provider_supports_websockets(provider),
            "model_reasoning_effort": self.provider_model_reasoning_effort(provider),
            "personality": self.provider_personality(provider),
            "loopback": base_url.as_deref().map(is_loopback_url).unwrap_or(false),
            "policy": "Auxiliary AI may receive workspace-derived prompts only when allow_workspace_data=true or the target is loopback.",
        })
    }

    pub fn default_model(&self) -> ModelRef {
        let provider = get_path(&self.config, "default_model.type")
            .and_then(Value::as_str)
            .unwrap_or("openai");
        let model = get_path(&self.config, "default_model.name")
            .and_then(Value::as_str)
            .unwrap_or("gpt-4o");
        ModelRef {
            provider: provider.to_string(),
            model: model.to_string(),
        }
    }

    pub fn auxiliary_ai_enabled(&self) -> bool {
        self.auxiliary_value("enabled")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    }

    pub fn auxiliary_ai_fail_open(&self) -> bool {
        self.auxiliary_value("fail_open")
            .and_then(Value::as_bool)
            .unwrap_or(true)
    }

    pub fn auxiliary_required_when_available(&self) -> bool {
        self.auxiliary_value("required_when_available")
            .and_then(Value::as_bool)
            .unwrap_or(true)
    }

    pub fn auxiliary_record_skip_reason(&self) -> bool {
        self.auxiliary_value("record_skip_reason")
            .and_then(Value::as_bool)
            .unwrap_or(true)
    }

    pub fn auxiliary_model(&self) -> Option<ModelRef> {
        self.auxiliary_providers().ok()?.into_iter().next()
    }

    pub fn auxiliary_async_enabled(&self) -> bool {
        self.auxiliary_value("async")
            .and_then(Value::as_bool)
            .unwrap_or(true)
    }

    pub fn auxiliary_providers(&self) -> Result<Vec<ModelRef>> {
        let auxiliary = self.auxiliary_config().unwrap_or(&self.config);
        if let Some(providers) =
            mapping_get_path(auxiliary, &["auxiliary_ai", "providers"]).and_then(Value::as_sequence)
        {
            let mut out = Vec::new();
            for item in providers {
                let enabled = mapping_get(item, "enabled")
                    .and_then(Value::as_bool)
                    .unwrap_or(true);
                if !enabled {
                    continue;
                }
                if let Some(model_ref) = item.as_str().and_then(|value| ModelRef::parse(value).ok())
                {
                    out.push(model_ref);
                    continue;
                }
                let provider = mapping_get(item, "provider")
                    .and_then(Value::as_str)
                    .context("auxiliary_ai.providers 条目缺少 provider")?;
                let model = mapping_get(item, "model")
                    .and_then(Value::as_str)
                    .context("auxiliary_ai.providers 条目缺少 model")?;
                out.push(ModelRef {
                    provider: provider.to_string(),
                    model: model.to_string(),
                });
            }
            return Ok(out);
        }
        let Some(provider) = self.auxiliary_value("provider").and_then(Value::as_str) else {
            return Ok(Vec::new());
        };
        let Some(model) = self.auxiliary_value("model").and_then(Value::as_str) else {
            return Ok(Vec::new());
        };
        Ok(vec![ModelRef {
            provider: provider.to_string(),
            model: model.to_string(),
        }])
    }

    pub fn auxiliary_target_report(&self) -> serde_json::Value {
        let providers = self.auxiliary_providers().unwrap_or_default();
        let model = providers.first().cloned();
        serde_json::json!({
            "enabled": self.auxiliary_ai_enabled(),
            "async": self.auxiliary_async_enabled(),
            "task_required_when_available": self.auxiliary_required_when_available(),
            "record_skip_reason": self.auxiliary_record_skip_reason(),
            "default_timeout_seconds": self.auxiliary_default_timeout_seconds(),
            "model": model.as_ref().map(ModelRef::as_string),
            "providers": providers
                .iter()
                .map(|model| serde_json::json!({
                    "model": model.as_string(),
                    "target": self.provider_target_report(&model.provider),
                }))
                .collect::<Vec<_>>(),
            "target": model
                .as_ref()
                .map(|model| self.provider_target_report(&model.provider)),
            "source": self.auxiliary_source(),
        })
    }

    pub fn auxiliary_task_enabled(&self, task: Option<&str>) -> bool {
        let Some(task) = task else {
            return true;
        };
        self.auxiliary_value("tasks")
            .and_then(Value::as_sequence)
            .map(|tasks| {
                tasks
                    .iter()
                    .filter_map(Value::as_str)
                    .any(|item| item == task)
            })
            .unwrap_or(true)
    }

    pub fn auxiliary_source(&self) -> Option<&Value> {
        self.auxiliary_config()
            .and_then(|value| mapping_get(value, "source"))
    }

    fn auxiliary_value(&self, key: &str) -> Option<&Value> {
        self.auxiliary_config()
            .and_then(|value| mapping_get_path(value, &["auxiliary_ai", key]))
            .or_else(|| get_path(&self.config, &format!("auxiliary_ai.{key}")))
    }

    pub fn route_candidates(
        &self,
        task: Option<&str>,
        route_mode: Option<&str>,
        explicit: Option<ModelRef>,
        no_fallback: bool,
    ) -> Vec<ModelRef> {
        if let Some(explicit) = explicit {
            return vec![explicit];
        }
        let routing = get_path(&self.config, "model_routing");
        let enabled = routing
            .and_then(|v| mapping_get(v, "enabled"))
            .and_then(Value::as_bool)
            .unwrap_or(true);
        if !enabled {
            return self.auto_routable_candidates(vec![self.default_model()]);
        }
        let mode = route_mode
            .or_else(|| {
                routing
                    .and_then(|v| mapping_get(v, "mode"))
                    .and_then(Value::as_str)
            })
            .unwrap_or("manual");
        let task = task.unwrap_or("default");
        let routes = routing.and_then(|v| mapping_get(v, "routes"));
        let route = routes
            .and_then(|v| mapping_get(v, task))
            .or_else(|| routes.and_then(|v| mapping_get(v, "default")));
        let mut out = Vec::new();
        if let Some(route) = route {
            let key = if mode == "auto" { "primary" } else { "manual" };
            if let Some(model) = mapping_get(route, key).and_then(Value::as_str)
                && let Ok(model_ref) = ModelRef::parse(model)
            {
                out.push(model_ref);
            }
            let fallback_on_failure = routing
                .and_then(|v| mapping_get(v, "fallback_on_failure"))
                .and_then(Value::as_bool)
                .unwrap_or(true);
            if mode == "auto"
                && fallback_on_failure
                && !no_fallback
                && let Some(fallbacks) = mapping_get(route, "fallback").and_then(Value::as_sequence)
            {
                for item in fallbacks {
                    if let Some(model) = item.as_str()
                        && let Ok(model_ref) = ModelRef::parse(model)
                    {
                        out.push(model_ref);
                    }
                }
            }
        }
        if mode == "auto" {
            out = self.auto_routable_candidates(out);
        }
        if out.is_empty() && mode != "auto" {
            out.push(self.default_model());
        }
        out
    }

    fn auto_routable_candidates(&self, models: Vec<ModelRef>) -> Vec<ModelRef> {
        if self.model_catalog().is_none() {
            return models;
        }
        models
            .into_iter()
            .filter(|model| self.model_is_auto_routable(model))
            .collect()
    }

    fn model_is_auto_routable(&self, model: &ModelRef) -> bool {
        let Some(provider) = self.model_catalog_provider(&model.provider) else {
            return false;
        };
        let Some(models) = mapping_get(provider, "models") else {
            return false;
        };
        let Some(entry) = mapping_get(models, &model.model) else {
            return false;
        };
        !mapping_get(entry, "catalog_status")
            .and_then(Value::as_str)
            .is_some_and(|status| status.eq_ignore_ascii_case("deprecated"))
    }

    fn load_referenced(&mut self) -> Result<()> {
        let Some(files) = get_path(&self.config, "config_files").and_then(Value::as_mapping) else {
            return Ok(());
        };
        for (name, relative) in files {
            let (Some(name), Some(relative)) = (name.as_str(), relative.as_str()) else {
                continue;
            };
            let path = self.config_dir().join(relative);
            if path.exists() {
                let mut value = load_yaml(&path)?;
                if name == "auxiliary_ai" {
                    upgrade_auxiliary_config_format(&path, &mut value)?;
                }
                self.referenced.insert(name.to_string(), value);
            }
        }
        Ok(())
    }

    pub fn refresh_auxiliary_ai_from_settings(
        &self,
        force: bool,
    ) -> Result<Option<serde_json::Value>> {
        let Some(auxiliary_path) = self.referenced_file_path("auxiliary_ai") else {
            return Ok(None);
        };
        if !auxiliary_path.exists() {
            return Ok(None);
        }
        let mut auxiliary = load_yaml(&auxiliary_path)?;
        let auto_update = get_path(&auxiliary, "source.auto_update_from_settings")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if !force && !auto_update {
            return Ok(None);
        }
        let Some(settings_url) = get_path(&auxiliary, "source.settings_url")
            .and_then(Value::as_str)
            .map(str::to_string)
        else {
            return Ok(None);
        };
        let settings = fetch_auxiliary_settings_json(settings_url.clone())?;
        upgrade_auxiliary_config_format(&auxiliary_path, &mut auxiliary)?;
        let provider = get_path(&auxiliary, "auxiliary_ai.providers")
            .and_then(Value::as_sequence)
            .and_then(|providers| providers.first())
            .and_then(|provider| mapping_get(provider, "provider"))
            .and_then(Value::as_str)
            .or_else(|| get_path(&auxiliary, "auxiliary_ai.provider").and_then(Value::as_str))
            .unwrap_or("ai_ubuntu_8888")
            .to_string();
        let preferred_port = get_path(&auxiliary, "source.preferred_port")
            .and_then(Value::as_u64)
            .unwrap_or(8888) as u16;
        let published_base_url = settings
            .get("base_url")
            .or_else(|| settings.get("baseUrl"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("http://ai-ubuntu.local:8000/v1");
        let preferred_base_url = replace_url_port(published_base_url, preferred_port);
        let model = settings
            .get("model")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("auto");

        set_path(
            &mut auxiliary,
            "source.published_base_url",
            Value::String(published_base_url.to_string()),
        )?;
        set_path(
            &mut auxiliary,
            "source.preferred_base_url",
            Value::String(preferred_base_url.clone()),
        )?;
        set_path(
            &mut auxiliary,
            "source.last_checked",
            Value::String(now_iso()),
        )?;
        update_first_auxiliary_provider_model(&mut auxiliary, model)?;
        set_path(
            &mut auxiliary,
            &format!("models.{provider}.base_url"),
            Value::String(preferred_base_url.clone()),
        )?;
        save_yaml(&auxiliary_path, &auxiliary)?;

        Ok(Some(serde_json::json!({
            "settings_url": settings_url,
            "published_base_url": published_base_url,
            "preferred_base_url": preferred_base_url,
            "provider": provider,
            "model": model,
            "updated_file": auxiliary_path.display().to_string(),
        })))
    }

    fn referenced_file_path(&self, name: &str) -> Option<PathBuf> {
        let relative = get_path(&self.config, "config_files")
            .and_then(Value::as_mapping)?
            .get(Value::String(name.to_string()))?
            .as_str()?;
        Some(self.config_dir().join(relative))
    }
}

fn workspace_canonical_config_path(root: &Path) -> Result<Option<PathBuf>> {
    let state_path = root.join(".RaymanCodingSkill").join("workspace_skill.yaml");
    if !state_path.exists() {
        return Ok(None);
    }
    let state = load_yaml(&state_path)?;
    let enabled = get_path(&state, "enabled")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if !enabled {
        return Ok(None);
    }
    let Some(skill_file) = get_path(&state, "skill_file").and_then(Value::as_str) else {
        return Ok(None);
    };
    let skill_path = Path::new(skill_file);
    let recorded_hash = get_path(&state, "skill_sha256")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .with_context(|| {
            format!(
                "workspace skill state is missing skill_sha256; run rayman workspace-skill mark-used before consuming canonical config: {}",
                state_path.display()
            )
        })?;
    let current_hash = sha256_file(skill_path).with_context(|| {
        format!(
            "unable to hash workspace skill file before consuming canonical config: {}",
            skill_path.display()
        )
    })?;
    if current_hash != recorded_hash {
        bail!(
            "workspace skill state is stale for {}; run rayman workspace-skill mark-used before consuming canonical config",
            skill_path.display()
        );
    }
    let Some(skill_root) = skill_path.parent() else {
        return Ok(None);
    };
    let config_path = skill_root.join("config").join("default_config.yaml");
    if config_path.exists() {
        assert_current_config_asset(skill_root, &config_path)?;
    }
    Ok(config_path.exists().then_some(config_path))
}

fn assert_current_config_asset(root: &Path, config_path: &Path) -> Result<()> {
    if !config_path.exists() {
        return Ok(());
    }
    let workspace = root
        .canonicalize()
        .with_context(|| format!("unable to resolve config workspace: {}", root.display()))?;
    let config_path = config_path
        .canonicalize()
        .with_context(|| format!("unable to resolve config path: {}", config_path.display()))?;
    if !config_path.starts_with(&workspace) {
        return Ok(());
    }
    let relative = config_path
        .strip_prefix(&workspace)
        .unwrap_or(&config_path)
        .to_string_lossy()
        .replace('\\', "/");
    let report = AssetRetirementManager::new(&workspace)?.status()?;
    if !report.is_current_behavior_path(&relative) {
        bail!("config asset is recorded as non-current and cannot be consumed: {relative}");
    }
    Ok(())
}

fn upgrade_auxiliary_config_format(path: &Path, value: &mut Value) -> Result<()> {
    if mapping_get(value, "auxiliary_ai").is_none() {
        return Ok(());
    }
    let mut changed = false;
    if mapping_get_path(value, &["auxiliary_ai", "default_timeout"]).is_none() {
        set_path(
            value,
            "auxiliary_ai.default_timeout",
            Value::Number(DEFAULT_AUXILIARY_PROVIDER_TIMEOUT_SECONDS.into()),
        )?;
        changed = true;
    }
    if mapping_get_path(value, &["auxiliary_ai", "async"]).is_none() {
        set_path(value, "auxiliary_ai.async", Value::Bool(true))?;
        changed = true;
    }
    if mapping_get_path(value, &["auxiliary_ai", "providers"]).is_none() {
        let provider = mapping_get_path(value, &["auxiliary_ai", "provider"])
            .and_then(Value::as_str)
            .map(str::to_string);
        let model = mapping_get_path(value, &["auxiliary_ai", "model"])
            .and_then(Value::as_str)
            .map(str::to_string);
        if let (Some(provider), Some(model)) = (provider, model) {
            let mut provider_entry = Mapping::new();
            provider_entry.insert(Value::String("provider".into()), Value::String(provider));
            provider_entry.insert(Value::String("model".into()), Value::String(model));
            provider_entry.insert(Value::String("enabled".into()), Value::Bool(true));
            set_path(
                value,
                "auxiliary_ai.providers",
                Value::Sequence(vec![Value::Mapping(provider_entry)]),
            )?;
            changed = true;
        }
    }
    if let Some(auxiliary) = mapping_get_mut(value, "auxiliary_ai")
        && let Some(mapping) = auxiliary.as_mapping_mut()
    {
        changed |= mapping.remove(Value::String("provider".into())).is_some();
        changed |= mapping.remove(Value::String("model".into())).is_some();
    }
    changed |= ensure_ai_ubuntu_env_proxy(value)?;
    if changed {
        save_yaml(path, value)?;
    }
    Ok(())
}

fn ensure_ai_ubuntu_env_proxy(value: &mut Value) -> Result<bool> {
    if mapping_get_path(value, &["models", "ai_ubuntu_8888"]).is_none()
        || mapping_get_path(value, &["models", "ai_ubuntu_8888", "proxy"]).is_some()
    {
        return Ok(false);
    }
    set_path(
        value,
        "models.ai_ubuntu_8888.proxy.mode",
        Value::String("env".into()),
    )?;
    Ok(true)
}

fn update_first_auxiliary_provider_model(value: &mut Value, model: &str) -> Result<()> {
    if let Some(providers) =
        mapping_get_path_mut(value, &["auxiliary_ai", "providers"]).and_then(Value::as_sequence_mut)
        && let Some(first) = providers.first_mut()
        && let Some(mapping) = first.as_mapping_mut()
    {
        mapping.insert(
            Value::String("model".into()),
            Value::String(model.to_string()),
        );
        return Ok(());
    }
    set_path(
        value,
        "auxiliary_ai.providers",
        Value::Sequence(vec![Value::Mapping({
            let mut provider = Mapping::new();
            provider.insert(
                Value::String("provider".into()),
                Value::String("ai_ubuntu_8888".into()),
            );
            provider.insert(
                Value::String("model".into()),
                Value::String(model.to_string()),
            );
            provider.insert(Value::String("enabled".into()), Value::Bool(true));
            provider
        })]),
    )
}

pub fn load_yaml(path: impl AsRef<Path>) -> Result<Value> {
    yaml::load_value(path.as_ref())
}

pub fn save_yaml(path: impl AsRef<Path>, value: &Value) -> Result<()> {
    yaml::save_value(path.as_ref(), value)
}

pub fn mapping_get<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
    value.as_mapping()?.get(Value::String(key.to_string()))
}

pub fn mapping_get_mut<'a>(value: &'a mut Value, key: &str) -> Option<&'a mut Value> {
    value
        .as_mapping_mut()?
        .get_mut(Value::String(key.to_string()))
}

pub fn mapping_get_path<'a>(value: &'a Value, parts: &[&str]) -> Option<&'a Value> {
    let mut current = value;
    for part in parts {
        current = mapping_get(current, part)?;
    }
    Some(current)
}

pub fn mapping_get_path_mut<'a>(value: &'a mut Value, parts: &[&str]) -> Option<&'a mut Value> {
    let mut current = value;
    for part in parts {
        current = mapping_get_mut(current, part)?;
    }
    Some(current)
}

pub fn get_path<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    let parts: Vec<_> = path.split('.').filter(|part| !part.is_empty()).collect();
    mapping_get_path(value, &parts)
}

pub fn set_path(value: &mut Value, path: &str, new_value: Value) -> Result<()> {
    let parts: Vec<_> = path.split('.').filter(|part| !part.is_empty()).collect();
    if parts.is_empty() {
        bail!("配置路径不能为空");
    }
    let mut current = value;
    for part in &parts[..parts.len() - 1] {
        let mapping = ensure_mapping_mut(current)?;
        current = mapping
            .entry(Value::String((*part).to_string()))
            .or_insert_with(|| Value::Mapping(Mapping::new()));
    }
    ensure_mapping_mut(current)?
        .insert(Value::String(parts[parts.len() - 1].to_string()), new_value);
    Ok(())
}

fn ensure_mapping_mut(value: &mut Value) -> Result<&mut Mapping> {
    if !value.is_mapping() {
        *value = Value::Mapping(Mapping::new());
    }
    value.as_mapping_mut().context("配置路径节点必须是映射")
}

pub fn parse_scalar(value: &str) -> Value {
    yaml::parse_scalar(value)
}

fn replace_url_port(url: &str, port: u16) -> String {
    let Some(scheme_end) = url.find("://") else {
        return url.to_string();
    };
    let authority_start = scheme_end + 3;
    let path_start = url[authority_start..]
        .find('/')
        .map(|index| authority_start + index)
        .unwrap_or(url.len());
    let authority = &url[authority_start..path_start];
    let host = authority
        .split_once(':')
        .map(|(host, _)| host)
        .unwrap_or(authority);
    format!(
        "{}://{}:{}{}",
        &url[..scheme_end],
        host,
        port,
        &url[path_start..]
    )
}

fn fetch_auxiliary_settings_json(settings_url: String) -> Result<serde_json::Value> {
    std::thread::spawn(move || {
        reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(
                AUXILIARY_SETTINGS_TIMEOUT_SECONDS,
            ))
            .build()
            .context("无法创建辅助 AI 设置 HTTP 客户端")?
            .get(settings_url)
            .send()
            .context("无法读取辅助 AI 设置页")?
            .error_for_status()
            .context("辅助 AI 设置页返回错误状态")?
            .json()
            .context("辅助 AI 设置页响应不是 JSON")
    })
    .join()
    .map_err(|_| anyhow::anyhow!("辅助 AI 设置刷新线程崩溃"))?
}

fn is_loopback_url(url: &str) -> bool {
    let host = url
        .trim()
        .strip_prefix("http://")
        .or_else(|| url.trim().strip_prefix("https://"))
        .unwrap_or(url.trim())
        .split('/')
        .next()
        .unwrap_or("")
        .trim_matches(['[', ']'])
        .split(':')
        .next()
        .unwrap_or("");
    host.eq_ignore_ascii_case("localhost")
        || host == "::1"
        || host.starts_with("127.")
        || host == "0.0.0.0"
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::fs;
    use std::path::PathBuf;

    #[test]
    fn dot_path_get_set_round_trip() {
        let mut value = serde_yaml::from_str("a:\n  b: 1\n").unwrap();
        assert_eq!(get_path(&value, "a.b").and_then(Value::as_i64), Some(1));
        set_path(&mut value, "a.c.d", Value::String("x".into())).unwrap();
        assert_eq!(get_path(&value, "a.c.d").and_then(Value::as_str), Some("x"));
    }

    #[test]
    fn dot_path_set_replaces_scalar_nodes_without_panic() {
        let mut value = Value::String("scalar root".into());

        set_path(&mut value, "a.b", Value::String("x".into())).unwrap();

        assert_eq!(get_path(&value, "a.b").and_then(Value::as_str), Some("x"));
    }

    #[test]
    fn model_ref_requires_provider_and_model() {
        assert!(ModelRef::parse("openai/gpt-4o").is_ok());
        assert!(ModelRef::parse("gpt-4o").is_err());
    }

    #[test]
    fn model_routing_consumes_enabled_mode_fallback_and_no_fallback() {
        let config: Value = serde_yaml::from_str(
            r#"
default_model:
  type: default
  name: default-model
model_routing:
  enabled: true
  mode: auto
  fallback_on_failure: true
  switch_cooldown_seconds: 999
  routes:
    default:
      manual: manual/manual-model
      primary: primary/primary-model
      fallback:
        - fallback/fallback-model
"#,
        )
        .unwrap();
        let manager = ConfigManager {
            root: PathBuf::from("."),
            config_path: PathBuf::from("config/default_config.yaml"),
            config,
            referenced: HashMap::new(),
        };

        let auto = manager.route_candidates(Some("unknown_task"), None, None, false);
        assert_eq!(
            auto.iter().map(ModelRef::as_string).collect::<Vec<_>>(),
            vec!["primary/primary-model", "fallback/fallback-model"]
        );

        let no_fallback = manager.route_candidates(Some("unknown_task"), None, None, true);
        assert_eq!(
            no_fallback
                .iter()
                .map(ModelRef::as_string)
                .collect::<Vec<_>>(),
            vec!["primary/primary-model"]
        );

        let manual = manager.route_candidates(Some("unknown_task"), Some("manual"), None, false);
        assert_eq!(
            manual.iter().map(ModelRef::as_string).collect::<Vec<_>>(),
            vec!["manual/manual-model"]
        );

        let explicit = manager.route_candidates(
            Some("unknown_task"),
            Some("manual"),
            Some(ModelRef {
                provider: "explicit".into(),
                model: "explicit-model".into(),
            }),
            false,
        );
        assert_eq!(
            explicit.iter().map(ModelRef::as_string).collect::<Vec<_>>(),
            vec!["explicit/explicit-model"]
        );

        let mut disabled_config = manager.config.clone();
        set_path(
            &mut disabled_config,
            "model_routing.enabled",
            Value::Bool(false),
        )
        .unwrap();
        let disabled = ConfigManager {
            root: PathBuf::from("."),
            config_path: PathBuf::from("config/default_config.yaml"),
            config: disabled_config,
            referenced: HashMap::new(),
        };
        assert_eq!(
            disabled
                .route_candidates(Some("unknown_task"), Some("auto"), None, false)
                .iter()
                .map(ModelRef::as_string)
                .collect::<Vec<_>>(),
            vec!["default/default-model"]
        );
    }

    #[test]
    fn auto_model_routing_filters_unknown_and_deprecated_catalog_entries() {
        let config: Value = serde_yaml::from_str(
            r#"
default_model:
  type: openai
  name: old-model
model_routing:
  enabled: true
  mode: auto
  fallback_on_failure: true
  routes:
    default:
      primary: openai/old-model
      fallback:
        - openai/missing-model
        - openai/current-model
"#,
        )
        .unwrap();
        let catalog: Value = serde_yaml::from_str(
            r#"
openai:
  models:
    old-model:
      catalog_status: deprecated
    current-model:
      catalog_status: active
"#,
        )
        .unwrap();
        let manager = ConfigManager {
            root: PathBuf::from("."),
            config_path: PathBuf::from("config/default_config.yaml"),
            config,
            referenced: HashMap::from([("models".to_string(), catalog)]),
        };

        let routes = manager.route_candidates(Some("default"), Some("auto"), None, false);

        assert_eq!(
            routes.iter().map(ModelRef::as_string).collect::<Vec<_>>(),
            vec!["openai/current-model"]
        );
    }

    #[test]
    fn model_route_catalog_findings_report_missing_provider_or_model() {
        let config: Value = serde_yaml::from_str(
            r#"
default_model:
  type: openai
  name: gpt-4o
model_routing:
  enabled: true
  routes:
    research:
      manual: openai/missing-model
      primary: missing_provider/research-model
      fallback:
        - openai/gpt-4o
"#,
        )
        .unwrap();
        let catalog: Value = serde_yaml::from_str(
            r#"
openai:
  models:
    gpt-4o: {}
"#,
        )
        .unwrap();
        let manager = ConfigManager {
            root: PathBuf::from("."),
            config_path: PathBuf::from("config/default_config.yaml"),
            config,
            referenced: HashMap::from([("models".to_string(), catalog)]),
        };

        let findings = manager.model_route_catalog_findings();

        assert_eq!(findings.len(), 2);
        assert_eq!(findings[0].source, "model_routing.routes.research.manual");
        assert_eq!(findings[0].model, "openai/missing-model");
        assert!(findings[0].reason.contains("not listed under provider"));
        assert_eq!(findings[1].source, "model_routing.routes.research.primary");
        assert_eq!(findings[1].model, "missing_provider/research-model");
        assert!(findings[1].reason.contains("provider `missing_provider`"));
    }

    #[test]
    fn canonical_model_routes_exist_in_catalog() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .unwrap()
            .to_path_buf();
        let config = ConfigManager::new(root).unwrap();

        assert_eq!(config.model_route_catalog_findings(), Vec::new());
    }

    #[test]
    fn auth_required_defaults_true_and_can_be_disabled() {
        let value: Value = serde_yaml::from_str(
            "models:\n  public:\n    auth_required: false\n  private:\n    api_key_env: KEY\n",
        )
        .unwrap();
        let manager = ConfigManager {
            root: PathBuf::from("."),
            config_path: PathBuf::from("config/default_config.yaml"),
            config: value,
            referenced: HashMap::new(),
        };

        assert!(!manager.auth_required("public"));
        assert!(manager.auth_required("private"));
        assert!(manager.auth_required("missing"));
    }

    #[test]
    fn provider_api_key_can_be_stored_inline_or_environment_named() {
        let value: Value = serde_yaml::from_str(
            "models:\n  inline:\n    api_key: sk-inline\n  env:\n    api_key_env: KEY\n",
        )
        .unwrap();
        let manager = ConfigManager {
            root: PathBuf::from("."),
            config_path: PathBuf::from("config/default_config.yaml"),
            config: value,
            referenced: HashMap::new(),
        };

        assert_eq!(
            manager.api_key_value("inline").as_deref(),
            Some("sk-inline")
        );
        assert_eq!(manager.api_key_env("env").as_deref(), Some("KEY"));
        assert_eq!(manager.api_key_value("missing"), None);
    }

    #[test]
    fn auxiliary_config_file_supplies_provider_and_model() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join("config")).unwrap();
        fs::write(
            temp.path().join("config").join("default_config.yaml"),
            "config_files:\n  auxiliary_ai: \"auxiliary_ai.yaml\"\n",
        )
        .unwrap();
        fs::write(
            temp.path().join("config").join("auxiliary_ai.yaml"),
            "auxiliary_ai:\n  enabled: true\n  provider: aux\n  model: auto\nmodels:\n  aux:\n    adapter: openai_compatible\n    auth_required: false\n    base_url: http://127.0.0.1:8888/v1\n",
        )
        .unwrap();

        let manager = ConfigManager::new(temp.path()).unwrap();

        assert!(manager.auxiliary_ai_enabled());
        assert_eq!(manager.auxiliary_model().unwrap().as_string(), "aux/auto");
        let upgraded = load_yaml(temp.path().join("config").join("auxiliary_ai.yaml")).unwrap();
        assert!(get_path(&upgraded, "auxiliary_ai.provider").is_none());
        assert_eq!(
            get_path(&upgraded, "auxiliary_ai.providers")
                .and_then(Value::as_sequence)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            manager.base_url("aux").as_deref(),
            Some("http://127.0.0.1:8888/v1")
        );
        assert!(!manager.auth_required("aux"));
        assert!(manager.auxiliary_required_when_available());
        assert!(manager.auxiliary_record_skip_reason());
    }

    #[test]
    fn inline_legacy_auxiliary_config_is_upgraded_to_providers() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join("config")).unwrap();
        let config_path = temp.path().join("config").join("default_config.yaml");
        fs::write(
            &config_path,
            r#"config_files: {}
auxiliary_ai:
  enabled: true
  provider: aux
  model: auto
models:
  aux:
    adapter: openai_compatible
    auth_required: false
    base_url: http://127.0.0.1:8888/v1
"#,
        )
        .unwrap();

        let manager = ConfigManager::new(temp.path()).unwrap();

        assert_eq!(manager.auxiliary_model().unwrap().as_string(), "aux/auto");
        let upgraded = load_yaml(&config_path).unwrap();
        assert!(get_path(&upgraded, "auxiliary_ai.provider").is_none());
        assert_eq!(
            get_path(&upgraded, "auxiliary_ai.providers")
                .and_then(Value::as_sequence)
                .unwrap()[0]["provider"]
                .as_str(),
            Some("aux")
        );
        assert_eq!(
            get_path(&upgraded, "auxiliary_ai.default_timeout").and_then(Value::as_u64),
            Some(DEFAULT_AUXILIARY_PROVIDER_TIMEOUT_SECONDS)
        );
        assert_eq!(
            get_path(&upgraded, "auxiliary_ai.async").and_then(Value::as_bool),
            Some(true)
        );
    }

    #[test]
    fn legacy_ai_ubuntu_auxiliary_config_is_upgraded_to_env_proxy() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join("config")).unwrap();
        fs::write(
            temp.path().join("config").join("default_config.yaml"),
            "config_files:\n  auxiliary_ai: \"auxiliary_ai.yaml\"\n",
        )
        .unwrap();
        fs::write(
            temp.path().join("config").join("auxiliary_ai.yaml"),
            r#"auxiliary_ai:
  enabled: true
  provider: ai_ubuntu_8888
  model: auto
models:
  ai_ubuntu_8888:
    adapter: openai_compatible
    auth_required: false
    base_url: http://192.168.15.204:8888/v1
"#,
        )
        .unwrap();

        let manager = ConfigManager::new(temp.path()).unwrap();

        assert_eq!(
            manager.provider_proxy_config("ai_ubuntu_8888").unwrap(),
            ProviderProxyConfig::Env
        );
        let upgraded = load_yaml(temp.path().join("config").join("auxiliary_ai.yaml")).unwrap();
        assert_eq!(
            get_path(&upgraded, "models.ai_ubuntu_8888.proxy.mode").and_then(Value::as_str),
            Some("env")
        );
    }

    #[test]
    fn auxiliary_providers_preserve_yaml_order_and_timeout_defaults() {
        let value: Value = serde_yaml::from_str(
            r#"auxiliary_ai:
  enabled: true
  default_timeout: 120
  providers:
    - provider: first
      model: auto
    - provider: second
      model: verifier
models:
  first:
    adapter: openai_compatible
    auth_required: false
    base_url: http://127.0.0.1:8888/v1
  second:
    adapter: openai_compatible
    auth_required: false
    base_url: http://127.0.0.1:9999/v1
    timeout: 7
"#,
        )
        .unwrap();
        let manager = ConfigManager {
            root: PathBuf::from("."),
            config_path: PathBuf::from("config/default_config.yaml"),
            config: Value::Mapping(Mapping::new()),
            referenced: HashMap::from([("auxiliary_ai".to_string(), value)]),
        };

        let providers = manager.auxiliary_providers().unwrap();

        assert_eq!(providers[0].as_string(), "first/auto");
        assert_eq!(providers[1].as_string(), "second/verifier");
        assert_eq!(manager.auxiliary_timeout_seconds("first"), 120);
        assert_eq!(manager.auxiliary_timeout_seconds("second"), 7);
    }

    #[test]
    fn auxiliary_proxy_requires_explicit_mode_and_parses_direct_http() {
        let value: Value = serde_yaml::from_str(
            r#"models:
  direct:
    proxy:
      mode: direct
  http:
    proxy:
      mode: http
      url: http://127.0.0.1:7890
  env:
    proxy:
      mode: env
  invalid:
    proxy:
      url: http://127.0.0.1:7890
"#,
        )
        .unwrap();
        let manager = ConfigManager {
            root: PathBuf::from("."),
            config_path: PathBuf::from("config/default_config.yaml"),
            config: value,
            referenced: HashMap::new(),
        };

        assert_eq!(
            manager.provider_proxy_config("missing").unwrap(),
            ProviderProxyConfig::Direct
        );
        assert_eq!(
            manager.provider_proxy_config("direct").unwrap(),
            ProviderProxyConfig::Direct
        );
        assert_eq!(
            manager.provider_proxy_config("http").unwrap(),
            ProviderProxyConfig::Http {
                url: "http://127.0.0.1:7890".into()
            }
        );
        assert_eq!(
            manager.provider_proxy_config("env").unwrap(),
            ProviderProxyConfig::Env
        );
        assert!(manager.provider_proxy_config("invalid").is_err());
    }

    #[test]
    fn auxiliary_workspace_data_requires_loopback_or_explicit_trust() {
        let value: Value = serde_yaml::from_str(
            r#"models:
  loopback:
    base_url: http://127.0.0.1:8888/v1
    auth_required: false
  lan:
    base_url: http://192.168.15.204:8888/v1
    auth_required: false
  trusted:
    base_url: http://192.168.15.204:8888/v1
    allow_workspace_data: true
    trust_level: trusted_lan
    auth_required: false
"#,
        )
        .unwrap();
        let manager = ConfigManager {
            root: PathBuf::from("."),
            config_path: PathBuf::from("config/default_config.yaml"),
            config: value,
            referenced: HashMap::new(),
        };

        assert!(manager.provider_allows_workspace_data("loopback"));
        assert!(!manager.provider_allows_workspace_data("lan"));
        assert!(manager.provider_allows_workspace_data("trusted"));
        assert_eq!(manager.provider_trust_level("trusted"), "trusted_lan");
    }

    #[test]
    fn opted_in_customer_workspace_uses_canonical_config_with_customer_root() {
        let customer = tempfile::tempdir().unwrap();
        let canonical = tempfile::tempdir().unwrap();
        fs::create_dir_all(canonical.path().join("config")).unwrap();
        fs::write(canonical.path().join("SKILL.md"), "# skill").unwrap();
        fs::write(
            canonical.path().join("config").join("default_config.yaml"),
            "config_files:\n  auxiliary_ai: \"auxiliary_ai.yaml\"\ndefault_model:\n  type: primary\n  name: model\n",
        )
        .unwrap();
        fs::write(
            canonical.path().join("config").join("auxiliary_ai.yaml"),
            "auxiliary_ai:\n  enabled: true\n  provider: aux\n  model: auto\n",
        )
        .unwrap();
        fs::create_dir_all(customer.path().join(".RaymanCodingSkill")).unwrap();
        let mut state = Mapping::new();
        state.insert(Value::String("enabled".into()), Value::Bool(true));
        state.insert(
            Value::String("skill_file".into()),
            Value::String(display_path_for_test(&canonical.path().join("SKILL.md"))),
        );
        state.insert(
            Value::String("skill_sha256".into()),
            Value::String(sha256_file(&canonical.path().join("SKILL.md")).unwrap()),
        );
        fs::write(
            customer
                .path()
                .join(".RaymanCodingSkill")
                .join("workspace_skill.yaml"),
            serde_yaml::to_string(&Value::Mapping(state)).unwrap(),
        )
        .unwrap();

        let manager = ConfigManager::new(customer.path()).unwrap();

        assert_eq!(manager.root, customer.path());
        assert_eq!(
            manager.config_path,
            canonical.path().join("config").join("default_config.yaml")
        );
        assert!(manager.auxiliary_ai_enabled());
    }

    #[test]
    fn opted_in_customer_workspace_rejects_stale_skill_hash_before_canonical_config() {
        let customer = tempfile::tempdir().unwrap();
        let canonical = tempfile::tempdir().unwrap();
        fs::create_dir_all(canonical.path().join("config")).unwrap();
        fs::write(canonical.path().join("SKILL.md"), "# skill v1").unwrap();
        fs::write(
            canonical.path().join("config").join("default_config.yaml"),
            "default_model:\n  type: primary\n  name: model\n",
        )
        .unwrap();
        fs::create_dir_all(customer.path().join(".RaymanCodingSkill")).unwrap();
        let mut state = Mapping::new();
        state.insert(Value::String("enabled".into()), Value::Bool(true));
        state.insert(
            Value::String("skill_file".into()),
            Value::String(display_path_for_test(&canonical.path().join("SKILL.md"))),
        );
        state.insert(
            Value::String("skill_sha256".into()),
            Value::String("old-hash".into()),
        );
        fs::write(
            customer
                .path()
                .join(".RaymanCodingSkill")
                .join("workspace_skill.yaml"),
            serde_yaml::to_string(&Value::Mapping(state)).unwrap(),
        )
        .unwrap();

        let error = ConfigManager::new(customer.path()).unwrap_err().to_string();

        assert!(error.contains("workspace skill state is stale"));
    }

    #[test]
    fn local_config_recorded_non_current_is_rejected() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join("config")).unwrap();
        fs::write(
            temp.path().join("config").join("default_config.yaml"),
            "default_model:\n  type: primary\n  name: model\n",
        )
        .unwrap();
        crate::assets::AssetRetirementManager::new(temp.path())
            .unwrap()
            .exempt(crate::assets::AssetExemptRequest {
                path: PathBuf::from("config/default_config.yaml"),
                retention_reason: "audit retention".into(),
                expires_at: "2999-01-01".into(),
            })
            .unwrap();

        let error = ConfigManager::new(temp.path()).unwrap_err().to_string();

        assert!(error.contains("config/default_config.yaml"));
        assert!(error.contains("non-current"));
    }

    #[test]
    fn opted_in_customer_workspace_rejects_non_current_canonical_config() {
        let customer = tempfile::tempdir().unwrap();
        let canonical = tempfile::tempdir().unwrap();
        fs::create_dir_all(canonical.path().join("config")).unwrap();
        fs::write(canonical.path().join("SKILL.md"), "# skill").unwrap();
        fs::write(
            canonical.path().join("config").join("default_config.yaml"),
            "default_model:\n  type: primary\n  name: model\n",
        )
        .unwrap();
        crate::assets::AssetRetirementManager::new(canonical.path())
            .unwrap()
            .exempt(crate::assets::AssetExemptRequest {
                path: PathBuf::from("config/default_config.yaml"),
                retention_reason: "audit retention".into(),
                expires_at: "2999-01-01".into(),
            })
            .unwrap();
        fs::create_dir_all(customer.path().join(".RaymanCodingSkill")).unwrap();
        let mut state = Mapping::new();
        state.insert(Value::String("enabled".into()), Value::Bool(true));
        state.insert(
            Value::String("skill_file".into()),
            Value::String(display_path_for_test(&canonical.path().join("SKILL.md"))),
        );
        state.insert(
            Value::String("skill_sha256".into()),
            Value::String(sha256_file(&canonical.path().join("SKILL.md")).unwrap()),
        );
        fs::write(
            customer
                .path()
                .join(".RaymanCodingSkill")
                .join("workspace_skill.yaml"),
            serde_yaml::to_string(&Value::Mapping(state)).unwrap(),
        )
        .unwrap();

        let error = ConfigManager::new(customer.path()).unwrap_err().to_string();

        assert!(error.contains("config/default_config.yaml"));
        assert!(error.contains("non-current"));
    }

    #[test]
    fn replace_url_port_preserves_path() {
        assert_eq!(
            replace_url_port("http://ai-ubuntu.local:8000/v1", 8888),
            "http://ai-ubuntu.local:8888/v1"
        );
    }

    #[test]
    fn auxiliary_settings_accept_snake_case_base_url() {
        let settings = serde_json::json!({
            "base_url": "http://192.168.15.204:8000/v1"
        });
        let published_base_url = settings
            .get("base_url")
            .or_else(|| settings.get("baseUrl"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("http://ai-ubuntu.local:8000/v1");

        assert_eq!(
            replace_url_port(published_base_url, 8888),
            "http://192.168.15.204:8888/v1"
        );
    }

    fn display_path_for_test(path: &Path) -> String {
        path.display().to_string()
    }

    #[test]
    fn core_skill_tasks_have_model_routes() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .unwrap()
            .to_path_buf();
        let config = ConfigManager::new(root).unwrap();
        for task in [
            "code_generation",
            "code_review",
            "implementation_validation",
            "test_generation",
            "code_refactor",
            "code_explain",
            "obsolete_code_pruning",
        ] {
            assert!(
                mapping_get_path(&config.config, &["model_routing", "routes", task]).is_some(),
                "missing model route for {task}"
            );
        }
    }

    #[test]
    fn obsolete_pruning_config_and_prompts_cover_asset_retirement() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .unwrap()
            .to_path_buf();
        let skills = fs::read_to_string(root.join("config").join("skills.yaml")).unwrap();
        let prompts = fs::read_to_string(root.join("config").join("prompts.yaml")).unwrap();

        assert!(skills.contains("supported_assets:"));
        assert!(skills.contains("require_asset_inventory: true"));
        assert!(skills.contains("require_docs_config_tests_sync: true"));
        assert!(skills.contains("remove_only_identified_obsolete_code: true"));
        assert!(prompts.contains("Obsolete assets across code, tests, docs, config"));
        assert!(prompts.contains("过时资产包括代码、测试、文档、配置"));
    }

    #[test]
    fn auxiliary_tasks_cover_model_backed_work() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .unwrap()
            .to_path_buf();
        let config = ConfigManager::new(root).unwrap();
        for task in [
            "planning",
            "code_generation",
            "code_review",
            "implementation_validation",
            "test_generation",
            "code_refactor",
            "code_explain",
            "obsolete_code_pruning",
            "doc_sync",
            "regression_test",
            "instruction_lifecycle",
            "conflict_detection",
            "review_gate",
            "workflow_summary",
        ] {
            assert!(
                config.auxiliary_task_enabled(Some(task)),
                "missing auxiliary task coverage for {task}"
            );
        }
    }

    #[test]
    fn default_auxiliary_provider_timeout_is_120_seconds() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .unwrap()
            .to_path_buf();
        let config = ConfigManager::new(root).unwrap();

        assert_eq!(config.timeout_seconds("ai_ubuntu_8888"), 120);
    }

    #[test]
    fn canonical_ai_ubuntu_auxiliary_provider_uses_environment_proxy() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .unwrap()
            .to_path_buf();
        let config = ConfigManager::new(root).unwrap();

        assert_eq!(
            config.provider_proxy_config("ai_ubuntu_8888").unwrap(),
            ProviderProxyConfig::Env
        );
        assert_eq!(
            config.provider_target_report("ai_ubuntu_8888")["proxy"]["mode"].as_str(),
            Some("env")
        );
    }
}
