//! OpenAI 兼容后端（DeepSeek / 本地 Ollama / 任意 OpenAI 兼容端点）。
//!
//! 端点/模型放在一个 gitignore 的 JSON 配置文件里（API 设置常变，改配置不用改代码），
//! 密钥放环境变量。内部把共享的 Anthropic 形状对话历史转成 OpenAI `chat/completions` 格式，
//! 再把响应转回 Anthropic 形状的 content，让 run_agent 循环无感知地复用。

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::agent::{Assistant, Model, ToolCall};

const DEFAULT_MAX_TOKENS: u32 = 4096;
const MAX_RETRIES: u32 = 3;

/// 配置文件结构：命名后端 -> 端点/模型/密钥环境变量名。
#[derive(Debug, Deserialize)]
pub struct BackendsConfig {
    pub backends: BTreeMap<String, BackendCfg>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BackendCfg {
    /// OpenAI 兼容基址，例如 https://api.deepseek.com/v1 或 http://localhost:11434/v1
    pub base_url: String,
    /// 模型名，例如 deepseek-chat、qwen2.5-coder。
    pub model: String,
    /// 存放密钥的环境变量名，例如 DEEPSEEK_API_KEY。留空表示本地端点无需密钥。
    #[serde(default)]
    pub api_key_env: Option<String>,
    /// 单次响应上限，默认 4096。
    #[serde(default)]
    pub max_tokens: Option<u32>,
}

impl BackendsConfig {
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path).with_context(|| {
            format!(
                "无法读取后端配置 {}（可从 backends.example.json 复制一份）",
                path.display()
            )
        })?;
        serde_json::from_str(&text)
            .with_context(|| format!("无法解析后端配置 JSON: {}", path.display()))
    }
}

pub struct OpenAiModel {
    client: reqwest::blocking::Client,
    base_url: String,
    model: String,
    api_key: Option<String>,
    max_tokens: u32,
    label: String,
}

impl OpenAiModel {
    /// 从配置项构建；密钥从 `api_key_env` 指定的环境变量读取。`model_override` 非空时覆盖配置模型。
    pub fn new(name: &str, cfg: &BackendCfg, model_override: Option<&str>) -> Result<Self> {
        let api_key = match &cfg.api_key_env {
            Some(env_name) if !env_name.trim().is_empty() => {
                let key = std::env::var(env_name)
                    .with_context(|| format!("后端 {name} 需要环境变量 {env_name}，但未设置"))?;
                if key.trim().is_empty() {
                    bail!("环境变量 {env_name} 为空");
                }
                Some(key)
            }
            _ => None, // 本地端点可无密钥
        };
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(300))
            .build()
            .context("无法创建 HTTP 客户端")?;
        let model = model_override.unwrap_or(&cfg.model).to_string();
        Ok(Self {
            label: format!("{name}({model})"),
            client,
            base_url: cfg.base_url.trim_end_matches('/').to_string(),
            model,
            api_key,
            max_tokens: cfg.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS),
        })
    }

    fn post(&self, body: &Value) -> Result<Value> {
        let url = format!("{}/chat/completions", self.base_url);
        let mut attempt = 0;
        loop {
            attempt += 1;
            let mut request = self
                .client
                .post(&url)
                .header("content-type", "application/json");
            if let Some(key) = &self.api_key {
                request = request.bearer_auth(key);
            }
            match request.json(body).send() {
                Ok(resp) => {
                    let status = resp.status();
                    if status.is_success() {
                        return resp.json::<Value>().context("OpenAI 兼容响应不是 JSON");
                    }
                    let retryable = status.as_u16() == 429 || status.as_u16() >= 500;
                    let text = resp.text().unwrap_or_default();
                    if retryable && attempt < MAX_RETRIES {
                        std::thread::sleep(Duration::from_secs(2 * attempt as u64));
                        continue;
                    }
                    bail!("OpenAI 兼容端点返回错误状态 {status}: {text}");
                }
                Err(error) => {
                    if attempt < MAX_RETRIES {
                        std::thread::sleep(Duration::from_secs(2 * attempt as u64));
                        continue;
                    }
                    return Err(error).context("OpenAI 兼容请求失败");
                }
            }
        }
    }
}

impl Model for OpenAiModel {
    fn respond(&self, system: &str, messages: &[Value], tools: &Value) -> Result<Assistant> {
        let body = json!({
            "model": self.model,
            "max_tokens": self.max_tokens,
            "messages": messages_to_openai(system, messages),
            "tools": tools_to_openai(tools),
            "tool_choice": "auto",
        });
        let response = self.post(&body)?;
        let message = &response["choices"][0]["message"];

        let mut content_blocks = Vec::new();
        if let Some(text) = message.get("content").and_then(Value::as_str)
            && !text.is_empty()
        {
            content_blocks.push(json!({"type": "text", "text": text}));
        }

        let mut tool_calls = Vec::new();
        if let Some(calls) = message.get("tool_calls").and_then(Value::as_array) {
            for call in calls {
                let id = call
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let function = call.get("function");
                let name = function
                    .and_then(|f| f.get("name"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let args = function
                    .and_then(|f| f.get("arguments"))
                    .and_then(Value::as_str)
                    .unwrap_or("{}");
                let input: Value = serde_json::from_str(args).unwrap_or_else(|_| json!({}));
                content_blocks.push(json!({
                    "type": "tool_use", "id": id, "name": name, "input": input
                }));
                tool_calls.push(ToolCall { id, name, input });
            }
        }

        Ok(Assistant {
            content: Value::Array(content_blocks),
            tool_calls,
        })
    }

    fn label(&self) -> String {
        self.label.clone()
    }
}

/// Anthropic tools schema -> OpenAI function-tools schema。
fn tools_to_openai(tools: &Value) -> Value {
    let converted: Vec<Value> = tools
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|tool| {
            json!({
                "type": "function",
                "function": {
                    "name": tool.get("name"),
                    "description": tool.get("description"),
                    "parameters": tool
                        .get("input_schema")
                        .cloned()
                        .unwrap_or_else(|| json!({"type": "object", "properties": {}})),
                }
            })
        })
        .collect();
    Value::Array(converted)
}

/// 把 run_agent 维护的 Anthropic 形状历史转成 OpenAI messages。
fn messages_to_openai(system: &str, messages: &[Value]) -> Vec<Value> {
    let mut out = vec![json!({"role": "system", "content": system})];
    for message in messages {
        let role = message
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or("user");
        let content = message.get("content");
        match role {
            "user" => {
                if let Some(text) = content.and_then(Value::as_str) {
                    out.push(json!({"role": "user", "content": text}));
                } else if let Some(blocks) = content.and_then(Value::as_array) {
                    // tool_result 块 -> 每个一条 {role: tool}
                    for block in blocks {
                        if block.get("type").and_then(Value::as_str) == Some("tool_result") {
                            out.push(json!({
                                "role": "tool",
                                "tool_call_id": block.get("tool_use_id"),
                                "content": block.get("content").and_then(Value::as_str).unwrap_or(""),
                            }));
                        }
                    }
                }
            }
            "assistant" => {
                let mut text = String::new();
                let mut tool_calls = Vec::new();
                if let Some(blocks) = content.and_then(Value::as_array) {
                    for block in blocks {
                        match block.get("type").and_then(Value::as_str) {
                            Some("text") => {
                                if let Some(part) = block.get("text").and_then(Value::as_str) {
                                    text.push_str(part);
                                }
                            }
                            Some("tool_use") => {
                                let args =
                                    serde_json::to_string(block.get("input").unwrap_or(&json!({})))
                                        .unwrap_or_else(|_| "{}".to_string());
                                tool_calls.push(json!({
                                    "id": block.get("id"),
                                    "type": "function",
                                    "function": {"name": block.get("name"), "arguments": args},
                                }));
                            }
                            _ => {}
                        }
                    }
                }
                let mut assistant = json!({"role": "assistant"});
                assistant["content"] = if text.is_empty() {
                    Value::Null
                } else {
                    Value::String(text)
                };
                if !tool_calls.is_empty() {
                    assistant["tool_calls"] = Value::Array(tool_calls);
                }
                out.push(assistant);
            }
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_anthropic_history_to_openai_shape() {
        let messages = vec![
            json!({"role": "user", "content": "do it"}),
            json!({"role": "assistant", "content": [
                {"type": "text", "text": "calling"},
                {"type": "tool_use", "id": "t1", "name": "run", "input": {"command": "cargo test"}},
            ]}),
            json!({"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "t1", "content": "ok", "is_error": false},
            ]}),
        ];
        let out = messages_to_openai("SYS", &messages);
        assert_eq!(out[0]["role"], "system");
        assert_eq!(out[1]["content"], "do it");
        assert_eq!(out[2]["role"], "assistant");
        assert_eq!(out[2]["tool_calls"][0]["function"]["name"], "run");
        // arguments 必须是 JSON 字符串（OpenAI 规范），不是对象。
        assert!(out[2]["tool_calls"][0]["function"]["arguments"].is_string());
        assert_eq!(out[3]["role"], "tool");
        assert_eq!(out[3]["tool_call_id"], "t1");
    }

    #[test]
    fn converts_tools_to_function_schema() {
        let tools = json!([{
            "name": "read_file",
            "description": "Read a file.",
            "input_schema": {"type": "object", "properties": {"path": {"type": "string"}}}
        }]);
        let out = tools_to_openai(&tools);
        assert_eq!(out[0]["type"], "function");
        assert_eq!(out[0]["function"]["name"], "read_file");
        assert!(out[0]["function"]["parameters"]["properties"]["path"].is_object());
    }
}
