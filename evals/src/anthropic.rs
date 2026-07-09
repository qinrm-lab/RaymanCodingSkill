//! 真实 Anthropic Messages API 后端（阻塞式 agent 循环）。
//!
//! 端点 POST /v1/messages；请求体 {model, max_tokens, system, messages, tools}。
//! 针对 Opus 4.8 有意**不**发送 temperature/top_p/top_k/thinking（这些会 400）。
//! stop_reason=tool_use 时由上层 run_agent 循环执行工具并回灌 tool_result；refusal 时 content 可能为空。

use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::agent::{Assistant, Model, ToolCall};

/// 默认模型：除非用户显式指定，始终用最新最强的 Opus。
pub const DEFAULT_MODEL: &str = "claude-opus-4-8";

const ENDPOINT: &str = "https://api.anthropic.com/v1/messages";
const ANTHROPIC_VERSION: &str = "2023-06-01";
const MAX_TOKENS: u32 = 16000; // 非流式请求的稳妥上限（不触发 SDK/HTTP 超时）
const MAX_RETRIES: u32 = 3;

pub struct AnthropicModel {
    client: reqwest::blocking::Client,
    api_key: String,
    model: String,
}

impl AnthropicModel {
    pub fn from_env(model: &str) -> Result<Self> {
        let api_key = std::env::var("ANTHROPIC_API_KEY")
            .context("未设置 ANTHROPIC_API_KEY，无法使用 anthropic 后端")?;
        if api_key.trim().is_empty() {
            bail!("ANTHROPIC_API_KEY 为空");
        }
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(300))
            .build()
            .context("无法创建 HTTP 客户端")?;
        Ok(Self {
            client,
            api_key,
            model: model.to_string(),
        })
    }

    fn post(&self, body: &Value) -> Result<Value> {
        let mut attempt = 0;
        loop {
            attempt += 1;
            let response = self
                .client
                .post(ENDPOINT)
                .header("x-api-key", &self.api_key)
                .header("anthropic-version", ANTHROPIC_VERSION)
                .header("content-type", "application/json")
                .json(body)
                .send();

            match response {
                Ok(resp) => {
                    let status = resp.status();
                    if status.is_success() {
                        return resp.json::<Value>().context("Anthropic 响应不是 JSON");
                    }
                    let retryable = status.as_u16() == 429 || status.as_u16() >= 500;
                    let text = resp.text().unwrap_or_default();
                    if retryable && attempt < MAX_RETRIES {
                        std::thread::sleep(Duration::from_secs(2 * attempt as u64));
                        continue;
                    }
                    bail!("Anthropic 返回错误状态 {status}: {text}");
                }
                Err(error) => {
                    if attempt < MAX_RETRIES {
                        std::thread::sleep(Duration::from_secs(2 * attempt as u64));
                        continue;
                    }
                    return Err(error).context("Anthropic 请求失败");
                }
            }
        }
    }
}

impl Model for AnthropicModel {
    fn respond(&self, system: &str, messages: &[Value], tools: &Value) -> Result<Assistant> {
        // 有意省略 temperature/top_p/top_k/thinking：Opus 4.8 会对它们返回 400。
        let body = json!({
            "model": self.model,
            "max_tokens": MAX_TOKENS,
            "system": system,
            "messages": messages,
            "tools": tools,
        });
        let response = self.post(&body)?;

        // refusal 时 content 可能为空——先取出再判断，不要假设有内容。
        let content = response
            .get("content")
            .cloned()
            .unwrap_or_else(|| Value::Array(Vec::new()));

        let mut tool_calls = Vec::new();
        if let Some(blocks) = content.as_array() {
            for block in blocks {
                if block.get("type").and_then(Value::as_str) == Some("tool_use") {
                    tool_calls.push(ToolCall {
                        id: block
                            .get("id")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        name: block
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        input: block.get("input").cloned().unwrap_or_else(|| json!({})),
                    });
                }
            }
        }

        Ok(Assistant {
            content,
            tool_calls,
        })
    }

    fn label(&self) -> String {
        format!("anthropic({})", self.model)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn label_includes_selected_model() {
        let model = AnthropicModel {
            client: reqwest::blocking::Client::new(),
            api_key: "test-key".into(),
            model: "claude-test".into(),
        };

        assert_eq!(model.label(), "anthropic(claude-test)");
    }
}
