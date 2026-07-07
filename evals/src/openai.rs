//! OpenAI 兼容后端：支持两种线协议
//! - `wire: "openai"`（默认）：`/chat/completions`（DeepSeek 官方、OpenRouter、Ollama 等）
//! - `wire: "responses"`：OpenAI Responses API `/responses`（如 yunyi.cfd 这类 Codex 中转）
//!
//! 端点/模型/密钥放在 gitignore 的 JSON 配置文件里（API 设置常变，改配置不用改代码）；
//! 密钥可用 `api_key`（内联，仅本地）或 `api_key_env`（环境变量名）。内部把共享的
//! Anthropic 形状对话历史转成对应线协议，再把响应转回 Anthropic 形状，让 run_agent 无感知复用。

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::agent::{Assistant, Model, ToolCall};

const DEFAULT_MAX_TOKENS: u32 = 400000;
const MAX_RETRIES: u32 = 3;

#[derive(Debug, Deserialize)]
pub struct BackendsConfig {
    pub backends: BTreeMap<String, BackendCfg>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BackendCfg {
    /// OpenAI 兼容基址，例如 https://api.deepseek.com/v1 或 https://yunyi.cfd/codex
    pub base_url: String,
    /// 模型名，例如 deepseek-chat、gpt-5.5。
    pub model: String,
    /// 线协议：`openai`（默认，/chat/completions）或 `responses`（/responses）。
    #[serde(default)]
    pub wire: Option<String>,
    /// 内联密钥（仅本地 gitignore 配置用）。优先级高于 api_key_env。
    #[serde(default)]
    pub api_key: Option<String>,
    /// 存放密钥的环境变量名，例如 DEEPSEEK_API_KEY。本地无密钥端点可省略。
    #[serde(default)]
    pub api_key_env: Option<String>,
    /// 单次响应上限，默认 400000。注意：多数端点的实际输出上限远低于此
    /// （如 DeepSeek 约 8192），端点若拒绝就在此调小。
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

#[derive(Clone, Copy, PartialEq)]
enum Wire {
    Chat,
    Responses,
}

pub struct OpenAiModel {
    client: reqwest::blocking::Client,
    url: String,
    model: String,
    api_key: Option<String>,
    max_tokens: u32,
    wire: Wire,
    label: String,
}

impl OpenAiModel {
    /// 从配置项构建；密钥优先取内联 api_key，否则取 api_key_env 指定的环境变量。
    /// `model_override` 非空时覆盖配置模型。
    pub fn new(name: &str, cfg: &BackendCfg, model_override: Option<&str>) -> Result<Self> {
        let wire = match cfg.wire.as_deref().unwrap_or("openai") {
            "openai" | "chat" | "chat_completions" => Wire::Chat,
            "responses" => Wire::Responses,
            other => bail!("后端 {name} 的 wire 不支持: {other}（用 openai 或 responses）"),
        };
        let api_key = resolve_key(name, cfg)?;
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(300))
            .build()
            .context("无法创建 HTTP 客户端")?;
        let model = model_override.unwrap_or(&cfg.model).to_string();
        let base = cfg.base_url.trim_end_matches('/');
        let url = match wire {
            Wire::Chat => format!("{base}/chat/completions"),
            Wire::Responses => format!("{base}/responses"),
        };
        Ok(Self {
            label: format!("{name}({model})"),
            client,
            url,
            model,
            api_key,
            max_tokens: cfg.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS),
            wire,
        })
    }

    fn post(&self, body: &Value) -> Result<Value> {
        let mut attempt = 0;
        loop {
            attempt += 1;
            let mut request = self
                .client
                .post(&self.url)
                .header("content-type", "application/json");
            if let Some(key) = &self.api_key {
                request = request.bearer_auth(key);
            }
            match request.json(body).send() {
                Ok(resp) => {
                    let status = resp.status();
                    if status.is_success() {
                        return resp.json::<Value>().context("端点响应不是 JSON");
                    }
                    let retryable = status.as_u16() == 429 || status.as_u16() >= 500;
                    let text = resp.text().unwrap_or_default();
                    if retryable && attempt < MAX_RETRIES {
                        std::thread::sleep(Duration::from_secs(2 * attempt as u64));
                        continue;
                    }
                    bail!("端点返回错误状态 {status}: {text}");
                }
                Err(error) => {
                    if attempt < MAX_RETRIES {
                        std::thread::sleep(Duration::from_secs(2 * attempt as u64));
                        continue;
                    }
                    return Err(error).context("请求失败");
                }
            }
        }
    }
}

fn resolve_key(name: &str, cfg: &BackendCfg) -> Result<Option<String>> {
    if let Some(inline) = &cfg.api_key
        && !inline.trim().is_empty()
    {
        return Ok(Some(inline.clone()));
    }
    match &cfg.api_key_env {
        Some(env_name) if !env_name.trim().is_empty() => {
            let key = std::env::var(env_name)
                .with_context(|| format!("后端 {name} 需要环境变量 {env_name}，但未设置"))?;
            if key.trim().is_empty() {
                bail!("环境变量 {env_name} 为空");
            }
            Ok(Some(key))
        }
        _ => Ok(None),
    }
}

impl Model for OpenAiModel {
    fn respond(&self, system: &str, messages: &[Value], tools: &Value) -> Result<Assistant> {
        match self.wire {
            Wire::Chat => {
                let body = json!({
                    "model": self.model,
                    "max_tokens": self.max_tokens,
                    "messages": messages_to_chat(system, messages),
                    "tools": tools_to_chat(tools),
                    "tool_choice": "auto",
                });
                parse_chat(&self.post(&body)?)
            }
            Wire::Responses => {
                let body = json!({
                    "model": self.model,
                    "max_output_tokens": self.max_tokens,
                    "instructions": system,
                    "input": messages_to_responses(messages),
                    "tools": tools_to_responses(tools),
                    "tool_choice": "auto",
                    "store": false,
                });
                parse_responses(&self.post(&body)?)
            }
        }
    }

    fn label(&self) -> String {
        self.label.clone()
    }
}

// ---- chat/completions 线协议 ----

fn tools_to_chat(tools: &Value) -> Value {
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

fn messages_to_chat(system: &str, messages: &[Value]) -> Vec<Value> {
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
                let (text, tool_calls) = split_assistant(content);
                let calls: Vec<Value> = tool_calls
                    .into_iter()
                    .map(|(id, name, args)| {
                        json!({
                            "id": id,
                            "type": "function",
                            "function": {"name": name, "arguments": args},
                        })
                    })
                    .collect();
                let mut assistant = json!({"role": "assistant"});
                assistant["content"] = if text.is_empty() {
                    Value::Null
                } else {
                    Value::String(text)
                };
                if !calls.is_empty() {
                    assistant["tool_calls"] = Value::Array(calls);
                }
                out.push(assistant);
            }
            _ => {}
        }
    }
    out
}

fn parse_chat(response: &Value) -> Result<Assistant> {
    let message = &response["choices"][0]["message"];
    let mut blocks = Vec::new();
    if let Some(text) = message.get("content").and_then(Value::as_str)
        && !text.is_empty()
    {
        blocks.push(json!({"type": "text", "text": text}));
    }
    let mut tool_calls = Vec::new();
    if let Some(calls) = message.get("tool_calls").and_then(Value::as_array) {
        for call in calls {
            let id = str_at(call, "id");
            let name = call
                .get("function")
                .map(|f| str_at(f, "name"))
                .unwrap_or_default();
            let args = call
                .get("function")
                .and_then(|f| f.get("arguments"))
                .and_then(Value::as_str)
                .unwrap_or("{}");
            let input: Value = serde_json::from_str(args).unwrap_or_else(|_| json!({}));
            blocks.push(json!({"type": "tool_use", "id": id, "name": name, "input": input}));
            tool_calls.push(ToolCall { id, name, input });
        }
    }
    Ok(Assistant {
        content: Value::Array(blocks),
        tool_calls,
    })
}

// ---- Responses 线协议 ----

fn tools_to_responses(tools: &Value) -> Value {
    let converted: Vec<Value> = tools
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|tool| {
            json!({
                "type": "function",
                "name": tool.get("name"),
                "description": tool.get("description"),
                "parameters": tool
                    .get("input_schema")
                    .cloned()
                    .unwrap_or_else(|| json!({"type": "object", "properties": {}})),
            })
        })
        .collect();
    Value::Array(converted)
}

fn messages_to_responses(messages: &[Value]) -> Vec<Value> {
    let mut input = Vec::new();
    for message in messages {
        let role = message
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or("user");
        let content = message.get("content");
        match role {
            "user" => {
                if let Some(text) = content.and_then(Value::as_str) {
                    input.push(json!({"role": "user", "content": text}));
                } else if let Some(blocks) = content.and_then(Value::as_array) {
                    for block in blocks {
                        if block.get("type").and_then(Value::as_str) == Some("tool_result") {
                            input.push(json!({
                                "type": "function_call_output",
                                "call_id": block.get("tool_use_id"),
                                "output": block.get("content").and_then(Value::as_str).unwrap_or(""),
                            }));
                        }
                    }
                }
            }
            "assistant" => {
                let (text, tool_calls) = split_assistant(content);
                if !text.is_empty() {
                    input.push(json!({
                        "type": "message",
                        "role": "assistant",
                        "content": [{"type": "output_text", "text": text}],
                    }));
                }
                for (id, name, args) in tool_calls {
                    input.push(json!({
                        "type": "function_call",
                        "call_id": id,
                        "name": name,
                        "arguments": args,
                    }));
                }
            }
            _ => {}
        }
    }
    input
}

fn parse_responses(response: &Value) -> Result<Assistant> {
    let mut blocks = Vec::new();
    let mut tool_calls = Vec::new();
    if let Some(output) = response.get("output").and_then(Value::as_array) {
        for item in output {
            match item.get("type").and_then(Value::as_str) {
                Some("message") => {
                    if let Some(parts) = item.get("content").and_then(Value::as_array) {
                        let mut text = String::new();
                        for part in parts {
                            if part.get("type").and_then(Value::as_str) == Some("output_text") {
                                text.push_str(
                                    part.get("text").and_then(Value::as_str).unwrap_or(""),
                                );
                            }
                        }
                        if !text.is_empty() {
                            blocks.push(json!({"type": "text", "text": text}));
                        }
                    }
                }
                Some("function_call") => {
                    // call_id 是回灌 function_call_output 时引用的 id。
                    let id = item
                        .get("call_id")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                        .unwrap_or_else(|| str_at(item, "id"));
                    let name = str_at(item, "name");
                    let args = item
                        .get("arguments")
                        .and_then(Value::as_str)
                        .unwrap_or("{}");
                    let input: Value = serde_json::from_str(args).unwrap_or_else(|_| json!({}));
                    blocks
                        .push(json!({"type": "tool_use", "id": id, "name": name, "input": input}));
                    tool_calls.push(ToolCall { id, name, input });
                }
                _ => {} // reasoning 等忽略
            }
        }
    }
    Ok(Assistant {
        content: Value::Array(blocks),
        tool_calls,
    })
}

// ---- 共享助手 ----

/// 从 Anthropic assistant content 块拆出文本 + (call_id, name, arguments_json_string) 列表。
fn split_assistant(content: Option<&Value>) -> (String, Vec<(String, String, String)>) {
    let mut text = String::new();
    let mut calls = Vec::new();
    if let Some(blocks) = content.and_then(Value::as_array) {
        for block in blocks {
            match block.get("type").and_then(Value::as_str) {
                Some("text") => {
                    if let Some(part) = block.get("text").and_then(Value::as_str) {
                        text.push_str(part);
                    }
                }
                Some("tool_use") => {
                    let id = str_at(block, "id");
                    let name = str_at(block, "name");
                    let args = serde_json::to_string(block.get("input").unwrap_or(&json!({})))
                        .unwrap_or_else(|_| "{}".to_string());
                    calls.push((id, name, args));
                }
                _ => {}
            }
        }
    }
    (text, calls)
}

fn str_at(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_history() -> Vec<Value> {
        vec![
            json!({"role": "user", "content": "do it"}),
            json!({"role": "assistant", "content": [
                {"type": "text", "text": "calling"},
                {"type": "tool_use", "id": "c1", "name": "run", "input": {"command": "cargo test"}},
            ]}),
            json!({"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "c1", "content": "ok", "is_error": false},
            ]}),
        ]
    }

    #[test]
    fn chat_history_and_tools_convert() {
        let out = messages_to_chat("SYS", &sample_history());
        assert_eq!(out[0]["role"], "system");
        assert_eq!(out[1]["content"], "do it");
        assert_eq!(out[2]["tool_calls"][0]["function"]["name"], "run");
        assert!(out[2]["tool_calls"][0]["function"]["arguments"].is_string());
        assert_eq!(out[3]["role"], "tool");
        assert_eq!(out[3]["tool_call_id"], "c1");

        let tools =
            json!([{"name": "read_file", "description": "d", "input_schema": {"type": "object"}}]);
        let t = tools_to_chat(&tools);
        assert_eq!(t[0]["function"]["name"], "read_file");
    }

    #[test]
    fn responses_history_and_tools_convert() {
        let input = messages_to_responses(&sample_history());
        // user 文本 + assistant 文本 message + function_call + function_call_output
        assert_eq!(input[0]["role"], "user");
        assert_eq!(input[1]["type"], "message");
        assert_eq!(input[1]["content"][0]["type"], "output_text");
        assert_eq!(input[2]["type"], "function_call");
        assert_eq!(input[2]["call_id"], "c1");
        assert_eq!(input[3]["type"], "function_call_output");
        assert_eq!(input[3]["call_id"], "c1");

        let tools =
            json!([{"name": "run", "description": "d", "input_schema": {"type": "object"}}]);
        let t = tools_to_responses(&tools);
        assert_eq!(t[0]["type"], "function");
        assert_eq!(t[0]["name"], "run"); // 扁平，不嵌套在 function 下
    }

    #[test]
    fn parse_responses_extracts_text_and_tool_calls() {
        let response = json!({
            "output": [
                {"type": "reasoning", "summary": []},
                {"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "hi"}]},
                {"type": "function_call", "id": "fc_1", "call_id": "call_1", "name": "run", "arguments": "{\"command\":\"ls\"}"},
            ]
        });
        let assistant = parse_responses(&response).unwrap();
        assert_eq!(assistant.tool_calls.len(), 1);
        assert_eq!(assistant.tool_calls[0].id, "call_1");
        assert_eq!(assistant.tool_calls[0].name, "run");
    }
}
