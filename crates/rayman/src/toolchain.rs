//! Which external programs this process can actually reach.
//!
//! Rayman shells out to `git`, `cargo`, and (on Windows) `schtasks`. Each call
//! site used to invent its own policy for "the program is not there", ranging
//! from a silent degrade to blocking the entire workspace, and none of them
//! told the operator up front. The most common real failure is not a broken
//! repository at all: it is a process whose `PATH` never inherited the
//! toolchain — an installer updates the persistent PATH, which an already
//! running shell does not pick up.
//!
//! This module only reports reachability. It never decides policy: a caller
//! that needs a program still fails closed, it just says something actionable.

use std::path::{Path, PathBuf};

use serde::Serialize;

/// Reachability of one external program, as this process would resolve it.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ToolProbe {
    pub name: &'static str,
    /// What rayman loses when this program cannot be run.
    pub required_for: &'static str,
    /// Whether the workspace actually needs it (a repo with no Cargo manifest
    /// does not need `cargo`), so a missing tool is not reported as a problem
    /// nobody has.
    pub relevant: bool,
    pub found: bool,
    pub path: Option<String>,
}

/// Resolve a program the way the OS would for this process.
///
/// Honors `PATHEXT` on Windows so `cargo.exe`/`git.exe` are found; a bare name
/// lookup silently reports "missing" on every Windows host.
pub fn resolve_program(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let candidates = program_file_names(name);
    std::env::split_paths(&path).find_map(|dir| {
        candidates
            .iter()
            .map(|candidate| dir.join(candidate))
            .find(|candidate| candidate.is_file())
    })
}

#[cfg(windows)]
fn program_file_names(name: &str) -> Vec<String> {
    let extensions = std::env::var_os("PATHEXT")
        .map(|raw| {
            raw.to_string_lossy()
                .split(';')
                .map(str::trim)
                .filter(|extension| !extension.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .filter(|extensions| !extensions.is_empty())
        .unwrap_or_else(|| vec![".COM".into(), ".EXE".into(), ".BAT".into(), ".CMD".into()]);
    extensions
        .iter()
        .map(|extension| format!("{name}{extension}"))
        .collect()
}

#[cfg(not(windows))]
fn program_file_names(name: &str) -> Vec<String> {
    vec![name.to_string()]
}

/// Report every external program rayman may need in this workspace.
pub fn toolchain_probe(root: &Path) -> Vec<ToolProbe> {
    let cargo_relevant = root.join("Cargo.toml").is_file()
        || root.join("crates").is_dir()
        || root.join("Cargo.lock").is_file();
    let mut probes = vec![
        probe("git", "源码状态、跟踪文件枚举与 clean-worktree 判定", true),
        probe(
            "cargo",
            "Cargo 拓扑权威确认（standard/release 就绪的硬前提）",
            cargo_relevant,
        ),
    ];
    if cfg!(windows) {
        probes.push(probe("schtasks", "autosave 计划任务注册与注销", true));
    }
    probes
}

fn probe(name: &'static str, required_for: &'static str, relevant: bool) -> ToolProbe {
    let resolved = resolve_program(name);
    ToolProbe {
        name,
        required_for,
        relevant,
        found: resolved.is_some(),
        path: resolved.map(|path| crate::pathfmt::display_path(&path)),
    }
}

/// The programs this workspace needs that this process cannot reach.
pub fn unreachable_required_tools(root: &Path) -> Vec<ToolProbe> {
    toolchain_probe(root)
        .into_iter()
        .filter(|probe| probe.relevant && !probe.found)
        .collect()
}

/// One actionable line for a program that is needed but unreachable.
pub fn unreachable_tool_advice(name: &str) -> String {
    format!(
        "{name} 不在本进程 PATH 中：安装器/工具链只改持久化 PATH，已经开着的终端不会继承；新开一个终端，或先把它的安装目录加进本进程 PATH"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_program_that_cannot_exist_is_reported_missing() {
        assert!(resolve_program("rayman-no-such-program-xyz").is_none());
    }

    #[test]
    fn cargo_is_only_relevant_to_a_cargo_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let plain = toolchain_probe(dir.path());
        let cargo = plain.iter().find(|probe| probe.name == "cargo").unwrap();
        assert!(
            !cargo.relevant,
            "a workspace with no manifest must not be told to install cargo"
        );

        std::fs::write(dir.path().join("Cargo.toml"), "[workspace]\n").unwrap();
        let rusty = toolchain_probe(dir.path());
        let cargo = rusty.iter().find(|probe| probe.name == "cargo").unwrap();
        assert!(cargo.relevant);
    }

    #[test]
    fn every_probe_names_what_it_is_needed_for() {
        let dir = tempfile::tempdir().unwrap();
        for probe in toolchain_probe(dir.path()) {
            assert!(!probe.required_for.trim().is_empty(), "{}", probe.name);
            assert_eq!(probe.found, probe.path.is_some());
        }
    }
}
