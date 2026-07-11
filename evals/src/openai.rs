//! OpenAI 兼容后端：支持两种线协议
//! - `wire: "openai"`（默认）：`/chat/completions`（DeepSeek 官方、OpenRouter、Ollama 等）
//! - `wire: "responses"`：OpenAI Responses API `/responses`（如 yunyi.cfd 这类 Codex 中转）
//!
//! 端点/模型/密钥放在 gitignore 的 JSON 配置文件里（API 设置常变，改配置不用改代码）；
//! 密钥可用 `api_key`（内联，仅本地）或 `api_key_env`（环境变量名）。内部把共享的
//! Anthropic 形状对话历史转成对应线协议，再把响应转回 Anthropic 形状，让 run_agent 无感知复用。

use std::collections::BTreeMap;
use std::net::IpAddr;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use reqwest::Url;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::agent::{Assistant, Model, ToolCall, Truncated};

const DEFAULT_MAX_TOKENS: u32 = 400000;
const MAX_RETRIES: u32 = 3;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackendsConfig {
    /// 允许示例配置保留说明文字，同时拒绝其他未知顶层字段。
    #[serde(default, rename = "_comment")]
    _comment: Option<String>,
    pub backends: BTreeMap<String, BackendCfg>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackendCfg {
    /// OpenAI 兼容基址，例如 https://api.deepseek.com/v1 或 https://yunyi.cfd/codex
    pub base_url: String,
    /// 模型名，例如 deepseek-chat、gpt-5.5。
    pub model: String,
    /// 线协议：`openai`（默认，/chat/completions）或 `responses`（/responses）。
    #[serde(default)]
    pub wire: Option<String>,
    /// 内联密钥默认拒绝；只有显式 `--unsafe-inline-api-key` 才可使用。
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
    pub fn new(
        name: &str,
        cfg: &BackendCfg,
        model_override: Option<&str>,
        allow_inline_api_key: bool,
    ) -> Result<Self> {
        let wire = match cfg.wire.as_deref().unwrap_or("openai") {
            "openai" | "chat" | "chat_completions" => Wire::Chat,
            "responses" => Wire::Responses,
            other => bail!("后端 {name} 的 wire 不支持: {other}（用 openai 或 responses）"),
        };
        let api_key = resolve_key(name, cfg, allow_inline_api_key)?;
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(300))
            .build()
            .context("无法创建 HTTP 客户端")?;
        let model = model_override.unwrap_or(&cfg.model).trim();
        if model.is_empty() {
            bail!("后端 {name} 的 model 不能为空");
        }
        let base = validate_base_url(name, &cfg.base_url)?;
        let max_tokens = cfg.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS);
        if max_tokens == 0 {
            bail!("后端 {name} 的 max_tokens 必须大于 0");
        }
        let url = match wire {
            Wire::Chat => format!("{base}/chat/completions"),
            Wire::Responses => format!("{base}/responses"),
        };
        Ok(Self {
            label: format!("{name}({model})"),
            client,
            url,
            model: model.to_string(),
            api_key,
            max_tokens,
            wire,
        })
    }

    fn post(&self, body: &Value) -> Result<String> {
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
                        return resp.text().context("无法读取响应体");
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

fn resolve_key(name: &str, cfg: &BackendCfg, allow_inline_api_key: bool) -> Result<Option<String>> {
    if cfg.api_key.is_some() && cfg.api_key_env.is_some() {
        bail!("后端 {name} 不能同时设置 api_key 与 api_key_env");
    }
    if let Some(inline) = &cfg.api_key {
        if inline.trim().is_empty() {
            bail!("后端 {name} 的 api_key 不能为空");
        }
        if !allow_inline_api_key {
            bail!(
                "后端 {name} 使用了内联 api_key；默认拒绝。改用批准的 api_key_env，或显式传 --unsafe-inline-api-key"
            );
        }
        return Ok(Some(inline.clone()));
    }
    match &cfg.api_key_env {
        Some(env_name) if !env_name.trim().is_empty() => {
            validate_key_env(name, env_name)?;
            let key = std::env::var(env_name)
                .with_context(|| format!("后端 {name} 需要环境变量 {env_name}，但未设置"))?;
            if key.trim().is_empty() {
                bail!("环境变量 {env_name} 为空");
            }
            Ok(Some(key))
        }
        Some(_) => bail!("后端 {name} 的 api_key_env 不能为空"),
        _ => Ok(None),
    }
}

/// 只允许明示为 API key 的少量变量名，避免把任意宿主机密钥按配置转发给远程端点。
fn validate_key_env(backend: &str, env_name: &str) -> Result<()> {
    const APPROVED: &[&str] = &[
        "ANTHROPIC_API_KEY",
        "DEEPSEEK_API_KEY",
        "GEMINI_API_KEY",
        "MISTRAL_API_KEY",
        "OPENAI_API_KEY",
        "OPENROUTER_API_KEY",
        "RELAY_API_KEY",
        "TOGETHER_API_KEY",
    ];
    if APPROVED.contains(&env_name) {
        Ok(())
    } else {
        bail!(
            "后端 {backend} 的 api_key_env `{env_name}` 不在批准列表；只允许专用 *_API_KEY 名称: {}",
            APPROVED.join(", ")
        );
    }
}

/// 远程后端只能走 HTTPS；HTTP 只允许 loopback，供本地 Ollama 等开发服务使用。
fn validate_base_url(name: &str, raw: &str) -> Result<String> {
    let url = Url::parse(raw.trim())
        .with_context(|| format!("后端 {name} 的 base_url 不是有效绝对 URL"))?;
    if !url.username().is_empty() || url.password().is_some() {
        bail!("后端 {name} 的 base_url 不得包含用户名或密码");
    }
    if url.query().is_some() || url.fragment().is_some() {
        bail!("后端 {name} 的 base_url 不得包含 query 或 fragment");
    }
    let host = url
        .host_str()
        .ok_or_else(|| anyhow::anyhow!("后端 {name} 的 base_url 缺少 host"))?;
    match url.scheme() {
        "https" => {}
        "http" if is_loopback_host(host) => {}
        "http" => {
            bail!("后端 {name} 的 HTTP base_url 仅允许 localhost/loopback；远程端点必须使用 HTTPS")
        }
        scheme => bail!("后端 {name} 的 base_url 仅支持 https（或 loopback http），收到 {scheme}"),
    }
    Ok(url.as_str().trim_end_matches('/').to_string())
}

fn is_loopback_host(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback())
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
                let text = self.post(&body)?;
                let value: Value = serde_json::from_str(&text)
                    .with_context(|| format!("chat/completions 响应不是 JSON: {}", head(&text)))?;
                parse_chat(&value)
            }
            Wire::Responses => {
                // Codex 类中转的 /responses 只回流式 SSE，故 stream:true + 解析事件流。
                let body = json!({
                    "model": self.model,
                    "max_output_tokens": self.max_tokens,
                    "instructions": system,
                    "input": messages_to_responses(messages),
                    "tools": tools_to_responses(tools),
                    "tool_choice": "auto",
                    "store": false,
                    "stream": true,
                });
                read_responses(&self.post(&body)?)
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
    let choice = &response["choices"][0];
    // finish_reason=length：输出被 max_tokens 截断，工具调用可能不完整，不能当干净收尾。
    if choice.get("finish_reason").and_then(Value::as_str) == Some("length") {
        return Err(anyhow::Error::new(Truncated));
    }
    let message = &choice["message"];
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
            let (input, input_error) = parse_tool_args(args);
            blocks.push(json!({"type": "tool_use", "id": id, "name": name, "input": input}));
            tool_calls.push(ToolCall {
                id,
                name,
                input,
                input_error,
            });
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

/// 读取 /responses 的响应体：非流式则整体是 JSON；流式则从 SSE 里取终态事件。
fn read_responses(text: &str) -> Result<Assistant> {
    // 非流式：整体就是一个 response 对象。
    if let Ok(value) = serde_json::from_str::<Value>(text) {
        return parse_response_snapshot(&value, text);
    }
    // 流式 SSE：只认 response.completed / response.failed 终态；
    // 中间快照（created/in_progress）带着空 output，绝不能当成功结果。
    let mut completed = None;
    for line in text.lines() {
        let Some(data) = line.trim_start().strip_prefix("data:") else {
            continue;
        };
        let data = data.trim();
        if data.is_empty() || data == "[DONE]" {
            continue;
        }
        let Ok(event) = serde_json::from_str::<Value>(data) else {
            continue;
        };
        match event.get("type").and_then(Value::as_str) {
            Some("response.completed") => {
                if let Some(response) = event.get("response") {
                    completed = Some(response.clone());
                }
            }
            Some("response.failed") => {
                let message = event
                    .pointer("/response/error/message")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .unwrap_or_else(|| event.to_string());
                bail!("responses 端点失败: {message}");
            }
            Some("response.incomplete") => return Err(anyhow::Error::new(Truncated)),
            _ => {}
        }
        if let Some(error) = event.get("error") {
            bail!("responses 流内错误: {error}");
        }
    }
    match completed {
        Some(response) => {
            let assistant = parse_responses(&response)?;
            debug_empty(&response, &assistant);
            Ok(assistant)
        }
        None => bail!("responses 流没有 response.completed 终态: {}", head(text)),
    }
}

/// 非流式 response 对象：完成态解析输出；failed/incomplete 显式报错，不把空 output 当成功。
fn parse_response_snapshot(value: &Value, raw: &str) -> Result<Assistant> {
    match value.get("status").and_then(Value::as_str) {
        Some("failed") => {
            let message = value
                .pointer("/error/message")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| head(raw));
            bail!("responses 端点失败: {message}");
        }
        Some("incomplete") => Err(anyhow::Error::new(Truncated)),
        _ => {
            if let Some(error) = value.get("error").filter(|e| !e.is_null()) {
                bail!("responses 端点错误: {error}");
            }
            if value.get("output").is_some() {
                return parse_responses(value);
            }
            bail!("无法从 responses 响应中提取结果: {}", head(raw));
        }
    }
}

/// EVAL_DEBUG 下，当一条 responses 解析为空（无工具调用、无文本）时，打印其 status /
/// incomplete_details / output 各项 type，用来判定是“reasoning-only 截断”还是解析漏抓。
fn debug_empty(response: &Value, assistant: &Assistant) {
    if std::env::var_os("EVAL_DEBUG").is_none() {
        return;
    }
    let has_text = assistant
        .content
        .as_array()
        .map(|blocks| {
            blocks
                .iter()
                .any(|b| b.get("type").and_then(Value::as_str) == Some("text"))
        })
        .unwrap_or(false);
    if assistant.tool_calls.is_empty() && !has_text {
        let types: Vec<&str> = response
            .get("output")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .map(|i| i.get("type").and_then(Value::as_str).unwrap_or("?"))
                    .collect()
            })
            .unwrap_or_default();
        eprintln!(
            "    [responses EMPTY] status={} incomplete={} output_types={:?}",
            response
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("?"),
            response
                .get("incomplete_details")
                .map(|v| v.to_string())
                .unwrap_or_else(|| "null".into()),
            types
        );
    }
}

/// 报错时截取响应体开头，便于诊断。
fn head(text: &str) -> String {
    text.chars().take(400).collect()
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
                    let (input, input_error) = parse_tool_args(args);
                    blocks
                        .push(json!({"type": "tool_use", "id": id, "name": name, "input": input}));
                    tool_calls.push(ToolCall {
                        id,
                        name,
                        input,
                        input_error,
                    });
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

/// 解析工具 arguments JSON；失败时不静默替换 `{}`，把原因带给执行层作为错误 tool_result 回给模型。
fn parse_tool_args(args: &str) -> (Value, Option<String>) {
    match serde_json::from_str(args) {
        Ok(input) => (input, None),
        Err(error) => (
            json!({}),
            Some(format!("{error}；原始片段: {}", head(args))),
        ),
    }
}

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

    #[test]
    fn read_responses_handles_sse_stream() {
        let sse = "event: response.output_text.delta\n\
                   data: {\"type\":\"response.output_text.delta\",\"delta\":\"h\"}\n\n\
                   event: response.completed\n\
                   data: {\"type\":\"response.completed\",\"response\":{\"output\":[\
                   {\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"done\"}]},\
                   {\"type\":\"function_call\",\"call_id\":\"c9\",\"name\":\"run\",\"arguments\":\"{}\"}]}}\n\n\
                   data: [DONE]\n\n";
        let assistant = read_responses(sse).unwrap();
        assert_eq!(assistant.tool_calls.len(), 1);
        assert_eq!(assistant.tool_calls[0].id, "c9");
    }

    #[test]
    fn read_responses_handles_plain_json() {
        let json_body = "{\"output\":[{\"type\":\"function_call\",\"call_id\":\"c1\",\"name\":\"run\",\"arguments\":\"{}\"}]}";
        let assistant = read_responses(json_body).unwrap();
        assert_eq!(assistant.tool_calls[0].id, "c1");
    }

    #[test]
    fn read_responses_reports_failed_terminal_event() {
        let sse = "data: {\"type\":\"response.failed\",\"response\":{\"status\":\"failed\",\"error\":{\"message\":\"boom\"}}}\n\n";
        let error = read_responses(sse).unwrap_err();
        assert!(error.to_string().contains("boom"), "{error}");
    }

    #[test]
    fn read_responses_rejects_intermediate_snapshot_without_completed() {
        // in_progress 快照带空 output，不能被当成功结果。
        let sse = "data: {\"type\":\"response.in_progress\",\"response\":{\"output\":[]}}\n\n\
                   data: [DONE]\n\n";
        assert!(read_responses(sse).is_err());
    }

    #[test]
    fn read_responses_maps_incomplete_to_truncated() {
        let sse = "data: {\"type\":\"response.incomplete\",\"response\":{\"output\":[]}}\n\n";
        let error = read_responses(sse).unwrap_err();
        assert!(error.is::<Truncated>());
    }

    #[test]
    fn parse_chat_maps_length_finish_reason_to_truncated() {
        let response = json!({
            "choices": [{"finish_reason": "length", "message": {"content": "partial"}}]
        });
        let error = parse_chat(&response).unwrap_err();
        assert!(error.is::<Truncated>());
    }

    #[test]
    fn invalid_tool_arguments_surface_as_input_error() {
        let response = json!({
            "choices": [{"message": {"tool_calls": [
                {"id": "c1", "function": {"name": "run", "arguments": "{\"command\": \"trunc"}}
            ]}}]
        });
        let assistant = parse_chat(&response).unwrap();
        assert_eq!(assistant.tool_calls.len(), 1);
        assert!(assistant.tool_calls[0].input_error.is_some());
        assert_eq!(assistant.tool_calls[0].input, json!({}));
    }

    #[test]
    fn backend_schema_rejects_unknown_fields() {
        let unknown_top = r#"{"backends": {}, "surprise": true}"#;
        assert!(serde_json::from_str::<BackendsConfig>(unknown_top).is_err());

        let unknown_backend = r#"{
            "backends": {
                "example": {
                    "base_url": "https://api.example.test/v1",
                    "model": "model",
                    "not_allowed": true
                }
            }
        }"#;
        assert!(serde_json::from_str::<BackendsConfig>(unknown_backend).is_err());
    }

    #[test]
    fn base_url_requires_https_except_loopback_http() {
        assert!(validate_base_url("x", "https://api.example.test/v1").is_ok());
        assert!(validate_base_url("x", "http://localhost:11434/v1").is_ok());
        assert!(validate_base_url("x", "http://127.0.0.1:11434/v1").is_ok());
        assert!(validate_base_url("x", "http://api.example.test/v1").is_err());
        assert!(validate_base_url("x", "ftp://localhost/files").is_err());
        assert!(validate_base_url("x", "https://user:pass@example.test/v1").is_err());
    }

    #[test]
    fn key_resolution_requires_approved_env_or_explicit_inline_ack() {
        let inline = BackendCfg {
            base_url: "https://api.example.test/v1".into(),
            model: "model".into(),
            wire: None,
            api_key: Some("secret".into()),
            api_key_env: None,
            max_tokens: None,
        };
        let error = resolve_key("x", &inline, false).unwrap_err().to_string();
        assert!(error.contains("--unsafe-inline-api-key"), "{error}");
        assert!(!error.contains("--unsafe-inline-key"), "{error}");
        assert_eq!(
            resolve_key("x", &inline, true).unwrap(),
            Some("secret".into())
        );

        let arbitrary_env = BackendCfg {
            api_key: None,
            api_key_env: Some("AWS_SECRET_ACCESS_KEY".into()),
            ..inline
        };
        assert!(resolve_key("x", &arbitrary_env, false).is_err());
        assert!(validate_key_env("x", "OPENAI_API_KEY").is_ok());

        let ambiguous = BackendCfg {
            api_key: Some("inline".into()),
            api_key_env: Some("OPENAI_API_KEY".into()),
            ..arbitrary_env
        };
        assert!(resolve_key("x", &ambiguous, true).is_err());
    }
}
