use std::path::Path;

use anyhow::{Context, Result};
use serde::Serialize;
use serde::de::DeserializeOwned;

pub use serde_yaml::{Mapping, Value};

use crate::{read_text, write_text};

pub fn from_str<T: DeserializeOwned>(text: &str) -> Result<T> {
    serde_yaml::from_str(text).context("无法解析 YAML")
}

pub fn to_string<T: Serialize>(value: &T) -> Result<String> {
    serde_yaml::to_string(value).context("无法序列化 YAML")
}

pub fn load_value(path: &Path) -> Result<Value> {
    let text = read_text(path).with_context(|| format!("无法读取 YAML: {}", path.display()))?;
    from_str(&text).with_context(|| format!("无法解析 YAML: {}", path.display()))
}

pub fn save_value(path: &Path, value: &Value) -> Result<()> {
    write_text(path, &to_string(value)?)
        .with_context(|| format!("无法写入 YAML: {}", path.display()))
}

pub fn parse_scalar(value: &str) -> Value {
    serde_yaml::from_str(value).unwrap_or_else(|_| Value::String(value.to_string()))
}
