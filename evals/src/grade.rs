//! 客观评分：在完成后的工作区里跑隐藏命令，退出 0 记为成功。

use std::path::Path;
use std::process::Command;

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct GradeResult {
    pub passed: bool,
    pub exit: i32,
    pub stdout: String,
    pub stderr: String,
}

/// 通过平台 shell 运行一条命令行；成本地捕获输出。
pub fn run_shell(workspace: &Path, command: &str) -> GradeResult {
    let output = shell(command).current_dir(workspace).output();
    match output {
        Ok(output) => GradeResult {
            passed: output.status.success(),
            exit: output.status.code().unwrap_or(-1),
            stdout: truncate(&String::from_utf8_lossy(&output.stdout)),
            stderr: truncate(&String::from_utf8_lossy(&output.stderr)),
        },
        Err(error) => GradeResult {
            passed: false,
            exit: -1,
            stdout: String::new(),
            stderr: format!("无法执行命令 `{command}`: {error}"),
        },
    }
}

fn shell(command: &str) -> Command {
    if cfg!(windows) {
        let mut cmd = Command::new("cmd");
        cmd.arg("/C").arg(command);
        cmd
    } else {
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(command);
        cmd
    }
}

fn truncate(text: &str) -> String {
    const MAX: usize = 4000;
    if text.len() <= MAX {
        return text.to_string();
    }
    let tail: String = text
        .chars()
        .rev()
        .take(MAX)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("…(truncated)\n{tail}")
}
