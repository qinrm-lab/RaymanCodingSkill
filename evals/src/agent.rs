//! 共享的最小 agent 循环 + 工具执行 + 可插拔模型后端。
//!
//! 循环与工具是共享的；后端（mock / anthropic）只负责“给定对话，产出下一条 assistant 消息”。
//! 这样 A/B 两组之间**唯一的自变量**就是 system 提示里有没有技能文本 + `rayman` 是否可用。

use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::Result;
use serde::Serialize;
use serde_json::{Value, json};

use crate::grade::{EnvPolicy, RUN_TIMEOUT, run_shell};

pub const MAX_STEPS: usize = 24;
static WRITE_COUNTER: AtomicU64 = AtomicU64::new(0);

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
            "description": "Run a shell command directly on the host in the workspace root and see stdout/stderr/exit code. This is NOT a sandbox: the command can access host resources allowed to this user.",
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
    /// arguments 解析失败时的原因（多为截断）；执行层直接把它作为错误 tool_result 回给模型。
    pub input_error: Option<String>,
}

/// 后端响应被输出上限截断（stop_reason=max_tokens / finish_reason=length）：
/// 不完整的 tool_use 已被丢弃，不能当干净收尾。上层据此把该次尝试记为 error 而非 fail。
#[derive(Debug)]
pub struct Truncated;

impl std::fmt::Display for Truncated {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("truncated: 响应被输出上限截断，工具调用可能不完整")
    }
}

impl std::error::Error for Truncated {}

/// 后端返回的一条 assistant 回复。
#[derive(Debug)]
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
    /// `run` 工具的子进程环境策略：with_skill 组注入 rayman 到 PATH，control 组反向剔除。
    pub env: EnvPolicy,
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
        let assistant = match respond_nonempty(model, &system, &messages, &tools) {
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
        if std::env::var_os("EVAL_DEBUG").is_some() {
            let names: Vec<&str> = assistant
                .tool_calls
                .iter()
                .map(|c| c.name.as_str())
                .collect();
            eprintln!(
                "    · step {steps}: {} tool_call(s) {:?}; content={}",
                assistant.tool_calls.len(),
                names,
                serde_json::to_string(&assistant.content)
                    .unwrap_or_default()
                    .chars()
                    .take(300)
                    .collect::<String>()
            );
        }
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
            if call.name == "run" && is_rayman_command(str_arg(&call.input, "command")) {
                rayman_invocations += 1;
            }
            let (content, is_error) = exec_tool(workspace, call, &cfg.env);
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

/// content 数组里是否有非空文本块（用于区分“正常收尾（有文本）”与“退化空响应”）。
fn has_text(content: &Value) -> bool {
    content
        .as_array()
        .map(|blocks| {
            blocks.iter().any(|block| {
                block.get("type").and_then(Value::as_str) == Some("text")
                    && !block
                        .get("text")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .trim()
                        .is_empty()
            })
        })
        .unwrap_or(false)
}

/// 调后端，重试退化的空响应（无工具调用且无文本——多为 responses/中转的 reasoning-only 抖动）。
/// 连续空到底则报错，让该次尝试显式失败而非被静默记成“干净结束”。
fn respond_nonempty(
    model: &dyn Model,
    system: &str,
    messages: &[Value],
    tools: &Value,
) -> Result<Assistant> {
    // 中转（如 yunyi）在负载高时会成串返回空完成（status=completed/failed, output=[]），
    // 多重试几次以穿过坏窗口；仍空到底才判该次尝试失败。
    const RETRIES: usize = 3;
    let debug = std::env::var_os("EVAL_DEBUG").is_some();
    let mut assistant = model.respond(system, messages, tools)?;
    let mut attempt = 0;
    while assistant.tool_calls.is_empty() && !has_text(&assistant.content) && attempt < RETRIES {
        attempt += 1;
        if debug {
            eprintln!("    · 空响应，重试 {attempt}/{RETRIES}");
        }
        assistant = model.respond(system, messages, tools)?;
    }
    if assistant.tool_calls.is_empty() && !has_text(&assistant.content) {
        anyhow::bail!(
            "后端连续 {} 次返回空响应（无工具调用、无文本）",
            RETRIES + 1
        );
    }
    Ok(assistant)
}

/// 命令首个 token 的 basename（去扩展名）等于 `rayman` 才算一次调用；
/// 只是出现在参数/文本里（如 `echo rayman`、`cargo test rayman_x`）不计。
fn is_rayman_command(command: &str) -> bool {
    let Some(first) = command.split_whitespace().next() else {
        return false;
    };
    let first = first.trim_matches(|c| c == '"' || c == '\'');
    // 手动切分两种分隔符：Unix 下 Path 不认 `\`，不能只靠 Path::file_name。
    let base = first.rsplit(['/', '\\']).next().unwrap_or(first);
    Path::new(base)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .is_some_and(|stem| stem.eq_ignore_ascii_case("rayman"))
}

/// 执行单个工具，返回 (内容, 是否错误)。文件工具拒绝任何已存在的 symlink 组件。
/// `run` 仍是宿主 shell 执行，不能也不会在这里伪装成 sandbox。
fn exec_tool(workspace: &Path, call: &ToolCall, env: &EnvPolicy) -> (String, bool) {
    if let Some(reason) = &call.input_error {
        return (
            format!("工具参数无效（arguments 不是合法 JSON，疑似被截断）: {reason}"),
            true,
        );
    }
    match call.name.as_str() {
        "list_files" => match list_files(workspace) {
            Ok(files) => (files, false),
            Err(error) => (format!("list_files 失败: {error}"), true),
        },
        "read_file" => match safe_path(workspace, str_arg(&call.input, "path"), PathAccess::Read) {
            Ok(path) => match fs::read_to_string(&path) {
                Ok(text) => (text, false),
                Err(error) => (format!("read_file 失败: {error}"), true),
            },
            Err(error) => (error, true),
        },
        "write_file" => match write_file(
            workspace,
            str_arg(&call.input, "path"),
            str_arg(&call.input, "content"),
        ) {
            Ok(()) => ("ok".to_string(), false),
            Err(error) => (format!("write_file 失败: {error}"), true),
        },
        "run" => {
            let result = run_shell(workspace, str_arg(&call.input, "command"), env, RUN_TIMEOUT);
            let content = format!(
                "exit={}\n--- stdout ---\n{}\n--- stderr ---\n{}",
                result.exit, result.stdout, result.stderr
            );
            (content, !result.outcome.passed())
        }
        other => (format!("未知工具: {other}"), true),
    }
}

fn str_arg<'a>(input: &'a Value, key: &str) -> &'a str {
    input.get(key).and_then(Value::as_str).unwrap_or("")
}

#[derive(Clone, Copy)]
enum PathAccess {
    Read,
    Write,
}

/// 把相对路径限制在工作区内，并拒绝任何已存在的 symlink/reparse-point 可见组件。
/// Rust 标准库没有跨平台的 `openat(..., O_NOFOLLOW)` 等价物，因此这里是可移植的
/// best-effort 防护：每次检查/创建后都复查元数据，且写入通过同目录临时文件 + rename。
fn safe_path(workspace: &Path, rel: &str, access: PathAccess) -> Result<PathBuf, String> {
    ensure_real_workspace(workspace)?;
    if rel.trim().is_empty() {
        return Err("path 为空".into());
    }
    let requested = Path::new(rel);
    if requested.is_absolute() {
        return Err(format!("拒绝绝对路径: {rel}"));
    }

    let components: Vec<_> = requested.components().collect();
    if components.is_empty() {
        return Err("path 为空".into());
    }
    let mut candidate = workspace.to_path_buf();
    for (index, component) in components.iter().enumerate() {
        match component {
            Component::CurDir => continue,
            Component::Normal(name) => candidate.push(name),
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(format!("拒绝越权路径: {rel}"));
            }
        }
        match fs::symlink_metadata(&candidate) {
            Ok(metadata) => {
                reject_symlink(&candidate, &metadata)?;
                if index + 1 < components.len() && !metadata.is_dir() {
                    return Err(format!("路径中间组件不是目录: {}", candidate.display()));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if matches!(access, PathAccess::Read) && index + 1 < components.len() {
                    return Err(format!("读取路径的父目录不存在: {}", candidate.display()));
                }
            }
            Err(error) => return Err(format!("无法检查路径 {}: {error}", candidate.display())),
        }
    }
    Ok(candidate)
}

fn ensure_real_workspace(workspace: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(workspace)
        .map_err(|error| format!("无法检查工作区 {}: {error}", workspace.display()))?;
    reject_symlink(workspace, &metadata)?;
    if !metadata.is_dir() {
        return Err(format!("工作区不是目录: {}", workspace.display()));
    }
    // 祖先检查可以防住常见的父级 symlink/junction；标准库无法彻底消除 TOCTOU，故不声称 sandbox。
    for ancestor in workspace.ancestors() {
        if let Ok(metadata) = fs::symlink_metadata(ancestor) {
            reject_symlink(ancestor, &metadata)?;
        }
    }
    Ok(())
}

fn reject_symlink(path: &Path, metadata: &fs::Metadata) -> Result<(), String> {
    if is_link_or_reparse(metadata) {
        Err(format!("拒绝 symlink/reparse 路径组件: {}", path.display()))
    } else {
        Ok(())
    }
}

/// Windows junction/symlink 都是 reparse point；`FileType::is_symlink` 不覆盖所有 junction。
/// 非 Windows 仅用标准库的 symlink 标识，保持这一层文件工具逻辑可移植。
fn is_link_or_reparse(metadata: &fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;

        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    {
        false
    }
}

fn write_file(workspace: &Path, rel: &str, content: &str) -> Result<(), String> {
    let path = safe_path(workspace, rel, PathAccess::Write)?;
    let parent = path
        .parent()
        .ok_or_else(|| format!("写入路径没有父目录: {}", path.display()))?;
    ensure_safe_parent_dirs(workspace, parent)?;
    if let Ok(metadata) = fs::symlink_metadata(&path) {
        reject_symlink(&path, &metadata)?;
        if !metadata.is_file() {
            return Err(format!("拒绝覆盖非普通文件: {}", path.display()));
        }
    }

    let temp = parent.join(format!(
        ".rayman-eval-write-{}-{}",
        std::process::id(),
        WRITE_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| -> Result<(), String> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .map_err(|error| format!("无法创建临时写入文件: {error}"))?;
        file.write_all(content.as_bytes())
            .map_err(|error| format!("无法写入临时文件: {error}"))?;
        file.sync_all()
            .map_err(|error| format!("无法同步临时文件: {error}"))?;
        drop(file);

        // rename 替换的是路径节点而非 symlink 的目标；Windows 若不允许覆盖，再只删除已验证的普通文件。
        match fs::rename(&temp, &path) {
            Ok(()) => Ok(()),
            Err(first_error) => {
                let metadata = fs::symlink_metadata(&path)
                    .map_err(|error| format!("无法在替换前复查目标 {}: {error}", path.display()))?;
                reject_symlink(&path, &metadata)?;
                if !metadata.is_file() {
                    return Err(format!("拒绝替换非普通文件: {}", path.display()));
                }
                fs::remove_file(&path)
                    .map_err(|error| format!("无法替换目标 {}: {error}", path.display()))?;
                fs::rename(&temp, &path).map_err(|error| {
                    format!(
                        "无法完成安全替换 {}（初始错误: {first_error}）: {error}",
                        path.display()
                    )
                })
            }
        }
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

fn ensure_safe_parent_dirs(workspace: &Path, parent: &Path) -> Result<(), String> {
    let relative = parent
        .strip_prefix(workspace)
        .map_err(|_| format!("写入父目录逃出工作区: {}", parent.to_string_lossy()))?;
    let mut current = workspace.to_path_buf();
    for component in relative.components() {
        let Component::Normal(name) = component else {
            return Err(format!("写入父目录包含非法组件: {}", parent.display()));
        };
        current.push(name);
        match fs::symlink_metadata(&current) {
            Ok(metadata) => {
                reject_symlink(&current, &metadata)?;
                if !metadata.is_dir() {
                    return Err(format!("写入父组件不是目录: {}", current.display()));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                match fs::create_dir(&current) {
                    Ok(()) => {}
                    Err(create_error)
                        if create_error.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(create_error) => {
                        return Err(format!(
                            "无法创建父目录 {}: {create_error}",
                            current.display()
                        ));
                    }
                }
                let metadata = fs::symlink_metadata(&current).map_err(|metadata_error| {
                    format!("无法复查新父目录 {}: {metadata_error}", current.display())
                })?;
                reject_symlink(&current, &metadata)?;
                if !metadata.is_dir() {
                    return Err(format!("新父组件不是目录: {}", current.display()));
                }
            }
            Err(error) => return Err(format!("无法检查父目录 {}: {error}", current.display())),
        }
    }
    Ok(())
}

fn list_files(workspace: &Path) -> Result<String, String> {
    ensure_real_workspace(workspace)?;
    let mut files = Vec::new();
    collect(workspace, workspace, &mut files)?;
    files.sort();
    Ok(if files.is_empty() {
        "(empty workspace)".into()
    } else {
        files.join("\n")
    })
}

fn collect(root: &Path, dir: &Path, out: &mut Vec<String>) -> Result<(), String> {
    let entries =
        fs::read_dir(dir).map_err(|error| format!("无法读取目录 {}: {error}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("无法枚举目录 {}: {error}", dir.display()))?;
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if matches!(name.as_ref(), "target" | ".git" | ".RaymanCodingSkill") {
            continue;
        }
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| format!("无法检查目录项 {}: {error}", path.display()))?;
        // list_files 从不跟随 link；静默跳过而不把宿主外部路径暴露给模型。
        if is_link_or_reparse(&metadata) {
            continue;
        }
        if metadata.is_dir() {
            collect(root, &path, out)?;
        } else if metadata.is_file() {
            let rel = path
                .strip_prefix(root)
                .map_err(|_| format!("文件逃出工作区: {}", path.display()))?;
            out.push(rel.to_string_lossy().replace('\\', "/"));
        }
    }
    Ok(())
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
                    tool_calls.push(ToolCall {
                        id,
                        name,
                        input,
                        input_error: None,
                    });
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
            env: EnvPolicy::default(),
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
        assert!(safe_path(dir.path(), "../evil.txt", PathAccess::Read).is_err());
        assert!(safe_path(dir.path(), "sub/ok.txt", PathAccess::Write).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn file_tools_do_not_follow_symlinks() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().join("workspace");
        fs::create_dir(&workspace).unwrap();
        let outside = dir.path().join("outside");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("secret.txt"), "secret").unwrap();
        symlink(&outside, workspace.join("escape")).unwrap();

        assert!(safe_path(&workspace, "escape/secret.txt", PathAccess::Read).is_err());
        assert!(!list_files(&workspace).unwrap().contains("secret.txt"));

        let call = ToolCall {
            id: "write".into(),
            name: "write_file".into(),
            input: json!({"path": "escape/new.txt", "content": "nope"}),
            input_error: None,
        };
        let (_, is_error) = exec_tool(&workspace, &call, &EnvPolicy::default());
        assert!(is_error);
        assert!(!outside.join("new.txt").exists());
    }

    #[test]
    fn write_file_creates_nested_regular_path() {
        let dir = tempfile::tempdir().unwrap();
        write_file(dir.path(), "nested/file.txt", "safe").unwrap();
        assert_eq!(
            fs::read_to_string(dir.path().join("nested/file.txt")).unwrap(),
            "safe"
        );
        write_file(dir.path(), "nested/file.txt", "replaced").unwrap();
        assert_eq!(
            fs::read_to_string(dir.path().join("nested/file.txt")).unwrap(),
            "replaced"
        );
    }

    #[test]
    fn skill_condition_injects_skill_text_into_system_prompt() {
        let with = AgentConfig {
            system_base: "BASE".into(),
            skill_text: Some("SKILL-BODY".into()),
            task_prompt: "t".into(),
            max_steps: 1,
            env: EnvPolicy::default(),
        };
        let control = AgentConfig {
            system_base: "BASE".into(),
            skill_text: None,
            task_prompt: "t".into(),
            max_steps: 1,
            env: EnvPolicy::default(),
        };
        assert!(system_prompt(&with).contains("SKILL-BODY"));
        assert!(!system_prompt(&control).contains("SKILL-BODY"));
    }

    #[test]
    fn rayman_invocation_counts_only_leading_rayman_token() {
        assert!(is_rayman_command("rayman scan --json"));
        assert!(is_rayman_command("./rayman scan"));
        assert!(is_rayman_command(r"C:\bin\rayman.exe map"));
        assert!(is_rayman_command("\"rayman\" goal"));
        assert!(!is_rayman_command("echo rayman"));
        assert!(!is_rayman_command("cargo test rayman_case"));
        assert!(!is_rayman_command("raymanx scan"));
        assert!(!is_rayman_command(""));
    }
}
