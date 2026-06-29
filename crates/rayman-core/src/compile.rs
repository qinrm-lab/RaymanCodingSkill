use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::{display_path, ensure_within};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoCompileResult {
    pub status: String,
    pub reason: Option<String>,
    pub language: String,
    pub source_path: Option<String>,
    pub executable_path: Option<String>,
    pub command: Vec<String>,
    pub cwd: Option<String>,
    pub exit_code: Option<i32>,
    pub stdout: Option<String>,
    pub stderr: Option<String>,
}

impl AutoCompileResult {
    pub fn skipped(language: &str, reason: impl Into<String>) -> Self {
        Self {
            status: "skipped".into(),
            reason: Some(reason.into()),
            language: language.into(),
            source_path: None,
            executable_path: None,
            command: Vec::new(),
            cwd: None,
            exit_code: None,
            stdout: None,
            stderr: None,
        }
    }

    pub fn failed(&self) -> bool {
        self.status == "failed"
    }
}

pub fn auto_compile_generated(
    workspace: impl AsRef<Path>,
    language: &str,
    output_path: Option<&Path>,
) -> Result<AutoCompileResult> {
    let Some(output_path) = output_path else {
        return Ok(AutoCompileResult::skipped(language, "no_output_path"));
    };
    let workspace = workspace.as_ref();
    let source_path = resolve_workspace_path(workspace, output_path)?;
    if !source_path.exists() {
        return Ok(AutoCompileResult::skipped(
            language,
            "output_path_does_not_exist",
        ));
    }
    match normalized_language(language).as_str() {
        "rust" => compile_rust(workspace, &source_path),
        _ => Ok(AutoCompileResult::skipped(language, "unsupported_language")),
    }
}

fn compile_rust(workspace: &Path, source_path: &Path) -> Result<AutoCompileResult> {
    if source_path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
        return Ok(AutoCompileResult::skipped("rust", "unsupported_rust_path"));
    }
    if let Some(cargo_root) = nearest_cargo_target_root(source_path, workspace) {
        return run_compile_command(
            "rust",
            source_path,
            None,
            "cargo",
            &[OsString::from("build")],
            &["build".into()],
            &cargo_root,
        );
    }
    let executable_path = standalone_executable_path(source_path);
    run_compile_command(
        "rust",
        source_path,
        Some(&executable_path),
        "rustc",
        &[
            source_path.as_os_str().to_os_string(),
            OsString::from("-o"),
            executable_path.as_os_str().to_os_string(),
        ],
        &[
            display_path(source_path),
            "-o".into(),
            display_path(&executable_path),
        ],
        workspace,
    )
}

fn run_compile_command(
    language: &str,
    source_path: &Path,
    executable_path: Option<&Path>,
    program: &str,
    args: &[OsString],
    display_args: &[String],
    cwd: &Path,
) -> Result<AutoCompileResult> {
    let command = std::iter::once(program.to_string())
        .chain(display_args.iter().cloned())
        .collect::<Vec<_>>();
    let output = Command::new(program).args(args).current_dir(cwd).output();
    let output = match output {
        Ok(output) => output,
        Err(error) => {
            return Ok(AutoCompileResult {
                status: "failed".into(),
                reason: Some(error.to_string()),
                language: language.into(),
                source_path: Some(display_path(source_path)),
                executable_path: executable_path.map(display_path),
                command,
                cwd: Some(display_path(cwd)),
                exit_code: None,
                stdout: None,
                stderr: None,
            });
        }
    };
    Ok(AutoCompileResult {
        status: if output.status.success() {
            "compiled".into()
        } else {
            "failed".into()
        },
        reason: None,
        language: language.into(),
        source_path: Some(display_path(source_path)),
        executable_path: executable_path.map(display_path),
        command,
        cwd: Some(display_path(cwd)),
        exit_code: output.status.code(),
        stdout: non_empty_output(&output.stdout),
        stderr: non_empty_output(&output.stderr),
    })
}

fn resolve_workspace_path(workspace: &Path, path: &Path) -> Result<PathBuf> {
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        workspace.join(path)
    };
    ensure_within(&candidate, workspace, "自动编译源文件必须位于工作区内")
}

fn normalized_language(language: &str) -> String {
    match language.trim().to_ascii_lowercase().as_str() {
        "rs" => "rust".into(),
        other => other.into(),
    }
}

fn standalone_executable_path(source_path: &Path) -> PathBuf {
    let mut executable = source_path.with_extension("");
    if cfg!(windows) {
        executable.set_extension("exe");
    }
    executable
}

fn nearest_cargo_target_root(source_path: &Path, workspace: &Path) -> Option<PathBuf> {
    let mut current = source_path.parent()?;
    loop {
        if !current.starts_with(workspace) {
            return None;
        }
        if current.join("Cargo.toml").exists() && is_cargo_target_source(source_path, current) {
            return Some(current.to_path_buf());
        }
        current = current.parent()?;
    }
}

fn is_cargo_target_source(source_path: &Path, cargo_root: &Path) -> bool {
    let Ok(relative) = source_path.strip_prefix(cargo_root) else {
        return false;
    };
    let parts = relative
        .components()
        .map(|component| component.as_os_str().to_string_lossy().to_string())
        .collect::<Vec<_>>();
    matches!(parts.as_slice(), [src, main] if src == "src" && main == "main.rs")
        || matches!(parts.as_slice(), [src, lib] if src == "src" && lib == "lib.rs")
        || matches!(parts.as_slice(), [src, bin, file] if src == "src" && bin == "bin" && file.ends_with(".rs"))
}

fn non_empty_output(bytes: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(bytes).trim().to_string();
    (!text.is_empty()).then_some(text)
}

pub fn compile_result_summary(result: &AutoCompileResult) -> String {
    match result.status.as_str() {
        "compiled" => {
            if let Some(executable) = &result.executable_path {
                format!("compiled executable: {executable}")
            } else {
                format!("compiled with: {}", result.command.join(" "))
            }
        }
        "skipped" => format!(
            "skipped: {}",
            result.reason.as_deref().unwrap_or("unknown_reason")
        ),
        "failed" => format!(
            "failed: {}",
            result
                .stderr
                .as_deref()
                .or(result.reason.as_deref())
                .unwrap_or("compile command failed")
        ),
        status => status.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn skips_when_generation_was_not_written_to_file() {
        let temp = tempfile::tempdir().unwrap();
        let result = auto_compile_generated(temp.path(), "rust", None).unwrap();

        assert_eq!(result.status, "skipped");
        assert_eq!(result.reason.as_deref(), Some("no_output_path"));
    }

    #[test]
    fn compiles_standalone_rust_file() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("hello.rs");
        fs::write(&source, "fn main() { println!(\"hello\"); }\n").unwrap();

        let result = auto_compile_generated(temp.path(), "rust", Some(&source)).unwrap();

        assert_eq!(result.status, "compiled");
        assert!(
            result
                .executable_path
                .as_ref()
                .map(Path::new)
                .is_some_and(Path::exists)
        );
    }

    #[test]
    fn compiles_standalone_rust_file_in_chinese_directory() {
        let temp = tempfile::tempdir().unwrap();
        let source_dir = temp.path().join("中文目录").join("子目录");
        fs::create_dir_all(&source_dir).unwrap();
        let source = source_dir.join("hello.rs");
        fs::write(&source, "fn main() { println!(\"中文输出\"); }\n").unwrap();

        let result = auto_compile_generated(temp.path(), "rust", Some(&source)).unwrap();

        assert_eq!(result.status, "compiled", "{result:#?}");
        assert!(
            result
                .source_path
                .as_deref()
                .is_some_and(|path| path.contains("中文目录"))
        );
        assert!(
            result
                .executable_path
                .as_ref()
                .map(Path::new)
                .is_some_and(Path::exists)
        );
        assert!(result.command.iter().any(|arg| arg.contains("中文目录")));
    }
}
