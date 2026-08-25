use std::env;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

const REPOSITORY_GATE: &str = "repository-gate";

fn main() -> ExitCode {
    match run(env::args_os().skip(1).collect()) {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            eprintln!("xtask: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: Vec<OsString>) -> Result<u8, String> {
    match args.as_slice() {
        [argument] if argument == REPOSITORY_GATE => run_repository_gate(),
        [argument] if argument == "-h" || argument == "--help" => {
            print_help();
            Ok(0)
        }
        [] => Err(format!(
            "missing subcommand; run `cargo xtask {REPOSITORY_GATE}`"
        )),
        _ => Err(format!(
            "unsupported arguments; authority accepts only `{REPOSITORY_GATE}`"
        )),
    }
}

fn print_help() {
    println!(
        "RaymanCodingSkill source tasks\n\nUsage:\n  cargo xtask {REPOSITORY_GATE}\n\n\
         `{REPOSITORY_GATE}` delegates to the reviewed repository gate. The alias is\n\
         convenience only; authority uses the explicit cargo-run argv."
    );
}

fn repository_root() -> Result<PathBuf, String> {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| "xtask manifest has no repository parent".to_string())
}

fn resolve_native_application(name: &str) -> Result<PathBuf, String> {
    let path = env::var_os("PATH").ok_or_else(|| "PATH is unavailable".to_string())?;
    let candidates: &[&str] = if cfg!(windows) {
        &["pwsh.exe"]
    } else {
        &[name]
    };
    for directory in env::split_paths(&path) {
        for candidate in candidates {
            let application = directory.join(candidate);
            if application.is_file() {
                return application.canonicalize().map_err(|error| {
                    format!("cannot canonicalize {}: {error}", application.display())
                });
            }
        }
    }
    Err(format!(
        "{name} must resolve directly to a native application on PATH"
    ))
}

fn run_repository_gate() -> Result<u8, String> {
    let root = repository_root()?;
    let gate = root.join("scripts").join("check-repo.ps1");
    if !gate.is_file() {
        return Err(format!("repository gate is missing: {}", gate.display()));
    }
    let pwsh = resolve_native_application("pwsh")?;
    let status = Command::new(&pwsh)
        .args(["-NoProfile", "-File"])
        .arg(&gate)
        .current_dir(&root)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map_err(|error| format!("failed to start {}: {error}", pwsh.display()))?;
    match status.code() {
        Some(code) if (0..=u8::MAX as i32).contains(&code) => Ok(code as u8),
        Some(code) => Err(format!(
            "repository gate returned unsupported exit code {code}"
        )),
        None => Err("repository gate terminated without an exit code".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repository_gate_is_the_only_operational_subcommand() {
        assert!(run(Vec::new()).is_err());
        assert!(run(vec!["unknown".into()]).is_err());
        assert!(run(vec![REPOSITORY_GATE.into(), "extra".into()]).is_err());
    }

    #[test]
    fn repository_root_contains_the_reviewed_gate() {
        let root = repository_root().unwrap();
        assert!(root.join("Cargo.toml").is_file());
        assert!(root.join("scripts/check-repo.ps1").is_file());
    }
}
