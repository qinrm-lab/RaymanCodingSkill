use rayman_core::config::{ConfigManager, mapping_get};
use serde_json::{Value, json};

pub(crate) fn model_update_status_json(config: &ConfigManager) -> Value {
    let updates = config.referenced.get("model_updates");
    let auto_update = updates.and_then(|value| mapping_get(value, "auto_update"));
    let enabled = auto_update
        .and_then(|value| mapping_get(value, "enabled"))
        .and_then(serde_yaml::Value::as_bool)
        .unwrap_or(false);
    let interval_days = auto_update
        .and_then(|value| mapping_get(value, "interval_days"))
        .and_then(serde_yaml::Value::as_i64)
        .unwrap_or(0);
    let last_update =
        yaml_value_to_json(updates.and_then(|value| mapping_get(value, "last_update")));
    json!({
        "auto_update_enabled": enabled,
        "interval_days": interval_days,
        "last_update": last_update,
        "next_update": Value::Null,
        "should_update": enabled && last_update.is_null()
    })
}

pub(crate) fn model_update_sources_json(config: &ConfigManager) -> Value {
    yaml_value_to_json(
        config
            .referenced
            .get("model_updates")
            .and_then(|value| mapping_get(value, "update_sources")),
    )
}

fn yaml_value_to_json(value: Option<&serde_yaml::Value>) -> Value {
    value
        .and_then(|value| serde_json::to_value(value).ok())
        .unwrap_or(Value::Null)
}
