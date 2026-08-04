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
    /// Whether **this process can spawn it**. Every call site uses bare
    /// `Command::new(name)`, so this — not mere presence on `PATH` — is what
    /// decides whether the tool works.
    pub found: bool,
    pub path: Option<String>,
    /// A `PATH` entry that exists but cannot be spawned by `Command::new`
    /// (a `.bat`/`.cmd` shim on Windows). Reporting it separately keeps
    /// "you have no cargo" distinct from "your cargo is a shim this process
    /// cannot launch", which need completely different repairs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unspawnable_shim: Option<String>,
}

/// Resolve a program exactly the way [`std::process::Command`] would for this
/// process — the only resolution that predicts whether a spawn succeeds.
///
/// On Windows that means the literal name and `name.exe`: `CreateProcessW`
/// appends `.exe` and does **not** consult `PATHEXT`. Probing `PATHEXT` here
/// instead reported a `.bat`/`.cmd` shim as reachable while every real spawn
/// failed with NotFound, and doctor then contradicted the gate that blocked.
///
/// This is *not* the right resolver for "what does typing this name in a shell
/// give the user" — see [`resolve_shell_command`].
pub fn resolve_spawnable_program(name: &str) -> Option<PathBuf> {
    resolve_with_extensions(name, &spawnable_file_names(name))
}

/// Resolve a name the way the user's **shell** would, honoring `PATHEXT` on
/// Windows.
///
/// Identity checks ask a different question from spawn checks: `doctor` must
/// see the `rayman.cmd` wrapper that shadows the real binary for anyone typing
/// `rayman`, even though this process could never spawn that wrapper itself.
pub fn resolve_shell_command(name: &str) -> Option<PathBuf> {
    resolve_with_extensions(name, &shell_file_names(name))
}

/// A `PATH` hit that is *not* spawnable by this process: used only to explain
/// the failure, never to claim reachability.
pub fn resolve_unspawnable_shim(name: &str) -> Option<PathBuf> {
    if !cfg!(windows) || resolve_spawnable_program(name).is_some() {
        return None;
    }
    resolve_with_extensions(name, &shim_file_names(name))
}

fn resolve_with_extensions(_name: &str, candidates: &[String]) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    resolve_in(&path, candidates)
}

/// The search itself, over an explicit `PATH` value.
///
/// Split out so the resolution rules can be tested without mutating the
/// process-global `PATH`: cargo runs a binary's tests as threads of one
/// process, so a test that rewrites `PATH` races every other test that spawns
/// or resolves a program, and a mutex shared by only the PATH-writing tests
/// does not exclude those.
fn resolve_in(path: &std::ffi::OsStr, candidates: &[String]) -> Option<PathBuf> {
    std::env::split_paths(path).find_map(|dir| {
        candidates
            .iter()
            .map(|candidate| dir.join(candidate))
            .find(|candidate| candidate.is_file())
    })
}

#[cfg(windows)]
fn spawnable_file_names(name: &str) -> Vec<String> {
    // CreateProcessW appends `.exe` to a program name that carries no extension
    // and never tries the bare name inside a PATH directory. Admitting the bare
    // name here reproduced the doctor-vs-gate contradiction this split exists to
    // remove: an extensionless `cargo` on PATH probed as reachable while every
    // spawn failed. A name that already has an extension is used as written.
    if Path::new(name).extension().is_some() {
        vec![name.to_string()]
    } else {
        vec![format!("{name}.exe")]
    }
}

/// `PATHEXT` extensions, in the order the shell tries them.
#[cfg(windows)]
fn pathext_extensions() -> Vec<String> {
    std::env::var_os("PATHEXT")
        .map(|raw| {
            raw.to_string_lossy()
                .split(';')
                .map(str::trim)
                .filter(|extension| !extension.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .filter(|extensions| !extensions.is_empty())
        .unwrap_or_else(|| vec![".COM".into(), ".EXE".into(), ".BAT".into(), ".CMD".into()])
}

#[cfg(windows)]
fn shell_file_names(name: &str) -> Vec<String> {
    pathext_extensions()
        .iter()
        .map(|extension| format!("{name}{extension}"))
        .collect()
}

#[cfg(windows)]
fn shim_file_names(name: &str) -> Vec<String> {
    pathext_extensions()
        .iter()
        .filter(|extension| !extension.eq_ignore_ascii_case(".EXE"))
        .map(|extension| format!("{name}{extension}"))
        .collect()
}

#[cfg(not(windows))]
fn spawnable_file_names(name: &str) -> Vec<String> {
    vec![name.to_string()]
}

#[cfg(not(windows))]
fn shell_file_names(name: &str) -> Vec<String> {
    vec![name.to_string()]
}

#[cfg(not(windows))]
fn shim_file_names(_name: &str) -> Vec<String> {
    Vec::new()
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
    let resolved = resolve_spawnable_program(name);
    let shim = resolved
        .is_none()
        .then(|| resolve_unspawnable_shim(name))
        .flatten();
    ToolProbe {
        name,
        required_for,
        relevant,
        found: resolved.is_some(),
        path: resolved.map(|path| crate::pathfmt::display_path(&path)),
        unspawnable_shim: shim.map(|path| crate::pathfmt::display_path(&path)),
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
///
/// A `.bat`/`.cmd` shim on `PATH` needs the opposite repair from a missing
/// tool: opening a new terminal cannot help, because this process cannot spawn
/// that file kind at all. Naming the shim is the whole diagnosis.
pub fn unreachable_tool_advice(name: &str) -> String {
    match resolve_unspawnable_shim(name) {
        Some(shim) => format!(
            "{name} 在 PATH 上只有本进程无法启动的 {} —— rayman 用 `Command::new` 直接创建进程，Windows 只会补 `.exe`，不解析 PATHEXT；请把真正的 {name}.exe 所在目录加进 PATH（或改用提供 .exe 的安装方式）",
            crate::pathfmt::display_path(&shim)
        ),
        None => format!(
            "{name} 不在本进程 PATH 中：安装器/工具链只改持久化 PATH，已经开着的终端不会继承；新开一个终端，或先把它的安装目录加进本进程 PATH"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_program_that_cannot_exist_is_reported_missing() {
        assert!(resolve_spawnable_program("rayman-no-such-program-xyz").is_none());
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

    /// `Command::new` 在 Windows 给无扩展名的程序补 `.exe`，**不会**在 PATH
    /// 目录里尝试裸名。探测若接受裸名，就复现了这个拆分本要消除的矛盾：
    /// doctor 说可达，门禁 spawn 却 NotFound。
    #[cfg(windows)]
    #[test]
    fn an_extensionless_file_on_path_is_not_reported_spawnable() {
        let dir = tempfile::tempdir().unwrap();
        let name = "rayman-probe-bare-xyz";
        std::fs::write(dir.path().join(name), "not an executable").unwrap();
        let path = std::ffi::OsString::from(dir.path());

        assert!(
            resolve_in(&path, &spawnable_file_names(name)).is_none(),
            "探测不得声称裸名可达"
        );
        // 前提：Command::new 也确实启动不了它（不改全局 PATH，用绝对路径等价验证）。
        assert!(
            std::process::Command::new(dir.path().join(name))
                .output()
                .is_err(),
            "无扩展名文件本就不可执行"
        );
    }

    /// 探测必须与真实 spawn 语义一致：`Command::new` 在 Windows 只补 `.exe`，
    /// 不解析 PATHEXT。此前按 PATHEXT 探测，于是 doctor 报告 .bat/.cmd shim
    /// "可达"，而门禁 spawn 失败并给出"新开终端"这类完全无效的修复建议。
    #[cfg(windows)]
    #[test]
    fn a_bat_shim_is_not_spawnable_but_is_still_visible_to_the_shell() {
        let dir = tempfile::tempdir().unwrap();
        let name = "rayman-probe-shim-xyz";
        std::fs::write(dir.path().join(format!("{name}.bat")), "@echo off\n").unwrap();
        let path = std::ffi::OsString::from(dir.path());

        assert!(
            resolve_in(&path, &spawnable_file_names(name)).is_none(),
            "探测不得声称 .bat shim 可达"
        );
        let shim = resolve_in(&path, &shim_file_names(name));
        assert!(shim.is_some(), "但必须能指出它就在 PATH 上");
        assert!(
            resolve_in(&path, &shell_file_names(name)).is_some(),
            "shell 语义（doctor 的身份检查）仍要看见它"
        );
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
