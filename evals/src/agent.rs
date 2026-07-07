//! 共享的最小 agent 循环 + 工具执行 + 可插拔模型后端。
//!
//! 循环与工具是共享的；后端（mock / anthropic）只负责“给定对话，产出下一条 assistant 消息”。
//! 这样 A/B 两组之间**唯一的自变量**就是 system 提示里有没有技能文本 + `rayman` 是否可用。

use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::Serialize;
use serde_json::{Value, json};

use crate::grade::run_shell;

pub const MAX_STEPS: usize = 24;

/// 工具定义（Anthropic tools schema）。mock 后端忽略它。
pub fn tool_defs() -> Value {
    json!([
        {
            "name": "list_files",
            "description": "List all files in the workspace (relative paths).",
            "input_schema": {"type": "object", "properties": {}}
        },
        {
            "name": "read_file",
            "description": "Read a text file in the workspace.",
            "input_schema": {
                "type": "object",
                "properties": {"path": {"type": "string"}},
                "required": ["path"]
            }
        },
        {
            "name": "write_file",
            "description": "Create or overwrite a text file in the workspace.",
            "input_schema": {
                "type": "object",
                "properties": {"path": {"type": "string"}, "content": {"type": "string"}},
                "required": ["path", "content"]
            }
        },
        {
            "name": "run",
            "description": "Run a shell command in the workspace root and see its stdout/stderr/exit code.",
            "input_schema": {
                "type": "object",
                "properties": {"command": {"type": "string"}},
                "required": ["command"]
            }
        }
    ])
}

#[derive(Debug, Clone)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub input: Value,
}

/// 后端返回的一条 assistant 回复。
pub struct Assistant {
    /// 原始 content 数组，直接作为 assistant 消息追加进对话。
    pub content: Value,
    pub tool_calls: Vec<ToolCall>,
}

/// 可插拔模型后端。
pub trait Model {
    fn respond(&self, system: &str, messages: &[Value], tools: &Value) -> Result<Assistant>;
    fn label(&self) -> String;
}

pub struct AgentConfig {
    pub system_base: String,
    /// WithSkill 组为 Some（注入 SKILL.md）；Control 组为 None。
    pub skill_text: Option<String>,
    pub task_prompt: String,
    pub max_steps: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct AttemptLog {
    pub steps: usize,
    pub tool_calls: usize,
    /// 模型是否自行结束（true）而非撞到 max_steps（false）。
    pub finished: bool,
    pub error: Option<String>,
    /// 用了几次 rayman 命令（用于观察技能是否真的被采纳）。
    pub rayman_invocations: usize,
}

fn system_prompt(cfg: &AgentConfig) -> String {
    let mut system = cfg.system_base.clone();
    if let Some(skill) = &cfg.skill_text {
        system.push_str(
            "\n\n---\nYou have the RaymanCodingSkill available in this workspace, and its `rayman` \
             CLI is on PATH. Its SKILL.md follows verbatim; use it as guidance.\n\n",
        );
        system.push_str(skill);
    }
    system
}

/// 跑一次 agent 尝试，直到模型自行结束或撞到 max_steps。工具在 `workspace` 内执行。
pub fn run_agent(model: &dyn Model, workspace: &Path, cfg: &AgentConfig) -> AttemptLog {
    let system = system_prompt(cfg);
    let tools = tool_defs();
    let mut messages: Vec<Value> = vec![json!({
        "role": "user",
        "content": cfg.task_prompt,
    })];

    let mut steps = 0usize;
    let mut tool_calls = 0usize;
    let mut rayman_invocations = 0usize;

    while steps < cfg.max_steps {
        steps += 1;
        let assistant = match model.respond(&system, &messages, &tools) {
            Ok(assistant) => assistant,
            Err(error) => {
                return AttemptLog {
                    steps,
                    tool_calls,
                    finished: false,
                    error: Some(format!("{error:#}")),
                    rayman_invocations,
                };
            }
        };
        messages.push(json!({"role": "assistant", "content": assistant.content}));

        if assistant.tool_calls.is_empty() {
            return AttemptLog {
                steps,
                tool_calls,
                finished: true,
                error: None,
                rayman_invocations,
            };
        }

        let mut results = Vec::new();
        for call in &assistant.tool_calls {
            tool_calls += 1;
            if call.name == "run"
                && call
                    .input
                    .get("command")
                    .and_then(Value::as_str)
                    .map(|command| command.contains("rayman"))
                    .unwrap_or(false)
            {
                rayman_invocations += 1;
            }
            let (content, is_error) = exec_tool(workspace, call);
            results.push(json!({
                "type": "tool_result",
                "tool_use_id": call.id,
                "content": content,
                "is_error": is_error,
            }));
        }
        messages.push(json!({"role": "user", "content": results}));
    }

    AttemptLog {
        steps,
        tool_calls,
        finished: false,
        error: None,
        rayman_invocations,
    }
}

/// 执行单个工具，返回 (内容, 是否错误)。读/写限制在工作区内。
fn exec_tool(workspace: &Path, call: &ToolCall) -> (String, bool) {
    match call.name.as_str() {
        "list_files" => (list_files(workspace), false),
        "read_file" => match safe_path(workspace, str_arg(&call.input, "path")) {
            Ok(path) => match std::fs::read_to_string(&path) {
                Ok(text) => (text, false),
                Err(error) => (format!("read_file 失败: {error}"), true),
            },
            Err(error) => (error, true),
        },
        "write_file" => match safe_path(workspace, str_arg(&call.input, "path")) {
            Ok(path) => {
                if let Some(parent) = path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                match std::fs::write(&path, str_arg(&call.input, "content")) {
                    Ok(()) => ("ok".to_string(), false),
                    Err(error) => (format!("write_file 失败: {error}"), true),
                }
            }
            Err(error) => (error, true),
        },
        "run" => {
            let result = run_shell(workspace, str_arg(&call.input, "command"));
            let content = format!(
                "exit={}\n--- stdout ---\n{}\n--- stderr ---\n{}",
                result.exit, result.stdout, result.stderr
            );
            (content, !result.passed)
        }
        other => (format!("未知工具: {other}"), true),
    }
}

fn str_arg<'a>(input: &'a Value, key: &str) -> &'a str {
    input.get(key).and_then(Value::as_str).unwrap_or("")
}

/// 把相对路径限制在工作区内，拒绝 `..` 逃逸。
fn safe_path(workspace: &Path, rel: &str) -> Result<PathBuf, String> {
    if rel.is_empty() {
        return Err("path 为空".into());
    }
    let candidate = workspace.join(rel);
    let normalized = normalize(&candidate);
    let base = normalize(workspace);
    if !normalized.starts_with(&base) {
        return Err(format!("拒绝越权路径: {rel}"));
    }
    Ok(normalized)
}

/// 词法归一化（不触碰文件系统），消解 `.`/`..`。
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn list_files(workspace: &Path) -> String {
    let mut files = Vec::new();
    collect(workspace, workspace, &mut files);
    files.sort();
    if files.is_empty() {
        "(empty workspace)".into()
    } else {
        files.join("\n")
    }
}

fn collect(root: &Path, dir: &Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if matches!(name.as_ref(), "target" | ".git" | ".RaymanCodingSkill") {
            continue;
        }
        if path.is_dir() {
            collect(root, &path, out);
        } else if let Ok(rel) = path.strip_prefix(root) {
            out.push(rel.to_string_lossy().replace('\\', "/"));
        }
    }
}

// ---- Mock 后端：按脚本回放工具调用，用来在不花钱的情况下验证整套编排与评分。----

pub struct MockModel {
    /// 每一轮要发出的工具调用；用尽后回一句文本表示结束。
    turns: std::cell::RefCell<std::collections::VecDeque<Vec<(String, Value)>>>,
    label: String,
}

impl MockModel {
    pub fn new(label: &str, turns: Vec<Vec<(String, Value)>>) -> Self {
        Self {
            turns: std::cell::RefCell::new(turns.into_iter().collect()),
            label: label.into(),
        }
    }
}

impl Model for MockModel {
    fn respond(&self, _system: &str, _messages: &[Value], _tools: &Value) -> Result<Assistant> {
        let mut turns = self.turns.borrow_mut();
        match turns.pop_front() {
            Some(calls) => {
                let mut content = Vec::new();
                let mut tool_calls = Vec::new();
                for (index, (name, input)) in calls.into_iter().enumerate() {
                    let id = format!("mock_{index}");
                    content
                        .push(json!({"type": "tool_use", "id": id, "name": name, "input": input}));
                    tool_calls.push(ToolCall { id, name, input });
                }
                Ok(Assistant {
                    content: Value::Array(content),
                    tool_calls,
                })
            }
            None => Ok(Assistant {
                content: json!([{"type": "text", "text": "done"}]),
                tool_calls: Vec::new(),
            }),
        }
    }

    fn label(&self) -> String {
        self.label.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_loop_executes_scripted_tools_and_finishes() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path();

        // 脚本：写一个文件 → 跑命令读回 → 结束。
        let model = MockModel::new(
            "mock",
            vec![vec![(
                "write_file".to_string(),
                json!({"path": "hello.txt", "content": "hi"}),
            )]],
        );
        let cfg = AgentConfig {
            system_base: "test".into(),
            skill_text: None,
            task_prompt: "do it".into(),
            max_steps: MAX_STEPS,
        };
        let log = run_agent(&model, workspace, &cfg);
        assert!(log.finished);
        assert_eq!(log.tool_calls, 1);
        assert_eq!(
            std::fs::read_to_string(workspace.join("hello.txt")).unwrap(),
            "hi"
        );
    }

    #[test]
    fn safe_path_rejects_escape() {
        let dir = tempfile::tempdir().unwrap();
        assert!(safe_path(dir.path(), "../evil.txt").is_err());
        assert!(safe_path(dir.path(), "sub/ok.txt").is_ok());
    }

    #[test]
    fn skill_condition_injects_skill_text_into_system_prompt() {
        let with = AgentConfig {
            system_base: "BASE".into(),
            skill_text: Some("SKILL-BODY".into()),
            task_prompt: "t".into(),
            max_steps: 1,
        };
        let control = AgentConfig {
            skill_text: None,
            ..AgentConfig {
                system_base: "BASE".into(),
                skill_text: None,
                task_prompt: "t".into(),
                max_steps: 1,
            }
        };
        assert!(system_prompt(&with).contains("SKILL-BODY"));
        assert!(!system_prompt(&control).contains("SKILL-BODY"));
    }
}
