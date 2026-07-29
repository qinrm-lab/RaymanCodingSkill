//! Facts about the Codex host environment, independent of any hook behavior.
//!
//! Kept separate from `codex_hook` so the Stop guard does not become a shared
//! dependency of every read-only CLI surface that only wants to report what the
//! host can do.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::Serialize;

use crate::file_io::is_link_or_reparse;

/// Resolve the Codex home directory the same way Codex itself does.
pub fn default_codex_home() -> Result<PathBuf> {
    if let Some(path) = env::var_os("CODEX_HOME").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(path));
    }
    let home = env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .ok_or_else(|| anyhow::anyhow!("cannot resolve Codex home"))?;
    Ok(PathBuf::from(home).join(".codex"))
}

/// Whether this Codex host can run its built-in patch tool at all.
///
/// The Windows `unelevated` restricted-token sandbox cannot express the managed
/// permission profile (read-only `.git`/`.agents`/`.codex` carved out of the
/// writable workspace root), so it refuses before `apply_patch` reads the file.
/// Reporting that once per command is what stops an agent from rediagnosing a
/// host defect on every context window.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct HostPatchProbe {
    /// False when this platform or host cannot exhibit the defect at all.
    pub applicable: bool,
    /// The configured `[windows] sandbox` value, when it could be read.
    pub sandbox_mode: Option<String>,
    /// False only on a positively identified broken configuration.
    pub patch_tool_usable: bool,
    pub reason: Option<String>,
    pub fix: Option<String>,
}

impl HostPatchProbe {
    fn usable(sandbox_mode: Option<String>, applicable: bool) -> Self {
        Self {
            applicable,
            sandbox_mode,
            patch_tool_usable: true,
            reason: None,
            fix: None,
        }
    }
}

/// Probe the Codex host patch tool without touching any host file.
///
/// Deliberately biased to silence: only a positively parsed `unelevated` value
/// reports a defect. An unreadable, absent, or ambiguous config reports usable,
/// because a false "your host is broken" banner on every read-only command is
/// worse than missing one.
pub fn patch_probe(codex_home: Option<&Path>) -> HostPatchProbe {
    if !cfg!(windows) {
        return HostPatchProbe::usable(None, false);
    }
    let Ok(home) = codex_home
        .map(Path::to_path_buf)
        .map_or_else(default_codex_home, Ok)
    else {
        return HostPatchProbe::usable(None, false);
    };
    let config = home.join("config.toml");
    // A linked/reparse config is refused rather than followed, matching how the
    // hooks file is handled; an unreadable one is simply not a finding.
    match fs::symlink_metadata(&config) {
        Ok(metadata) if metadata.file_type().is_file() && !is_link_or_reparse(&metadata) => {}
        _ => return HostPatchProbe::usable(None, true),
    }
    let Ok(text) = fs::read_to_string(&config) else {
        return HostPatchProbe::usable(None, true);
    };
    let mode = windows_sandbox_mode(&text);
    if mode.as_deref() == Some("unelevated") {
        return HostPatchProbe {
            applicable: true,
            sandbox_mode: mode,
            patch_tool_usable: false,
            reason: Some(
                "Codex `[windows] sandbox = \"unelevated\"` cannot enforce split writable roots, split filesystem reads, or deny-read restrictions, so the built-in apply_patch refuses before reading the target".into(),
            ),
            fix: Some(
                "set `[windows] sandbox = \"elevated\"` in the Codex config and restart Codex; until then apply patches with `git apply` from a file".into(),
            ),
        };
    }
    HostPatchProbe::usable(mode, true)
}

/// Read `sandbox` from the `[windows]` table of a TOML document.
///
/// Hand-rolled because the tool ships no TOML dependency. It only recognizes the
/// unambiguous `key = "value"` form inside a plain `[windows]` header and returns
/// `None` for anything it does not fully understand.
fn windows_sandbox_mode(text: &str) -> Option<String> {
    let mut in_windows = false;
    for line in text.lines() {
        let line = match line.split_once('#') {
            Some((before, _)) => before,
            None => line,
        }
        .trim();
        if line.starts_with('[') {
            in_windows = line == "[windows]";
            continue;
        }
        if !in_windows {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim() != "sandbox" {
            continue;
        }
        let value = value.trim();
        return value
            .strip_prefix('"')
            .and_then(|value| value.strip_suffix('"'))
            .map(str::to_string);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_sandbox_mode_reads_only_the_windows_table() {
        assert_eq!(
            windows_sandbox_mode("[windows]\nsandbox = \"unelevated\"\n").as_deref(),
            Some("unelevated")
        );
        // A `sandbox` key belonging to another table must never be attributed
        // to `[windows]`, or an unrelated config reports a host defect.
        assert_eq!(
            windows_sandbox_mode("[other]\nsandbox = \"unelevated\"\n"),
            None
        );
        assert_eq!(
            windows_sandbox_mode(
                "[windows]\nsandbox = \"elevated\"\n[other]\nsandbox = \"unelevated\"\n"
            )
            .as_deref(),
            Some("elevated")
        );
        // Commented-out and unquoted values are not understood, so they must
        // read as unknown rather than as a positive finding.
        assert_eq!(
            windows_sandbox_mode("[windows]\n# sandbox = \"unelevated\"\n"),
            None
        );
        assert_eq!(
            windows_sandbox_mode("[windows]\nsandbox = unelevated\n"),
            None
        );
        assert_eq!(
            windows_sandbox_mode("[windows]\n  sandbox   =   \"unelevated\"  # trailing\n")
                .as_deref(),
            Some("unelevated")
        );
    }

    #[test]
    fn patch_probe_reports_a_defect_only_for_a_parsed_unelevated_sandbox() {
        let home = tempfile::tempdir().unwrap();
        // Absent config: nothing is known, so nothing is claimed.
        assert!(patch_probe(Some(home.path())).patch_tool_usable);

        fs::write(
            home.path().join("config.toml"),
            "[windows]\nsandbox = \"elevated\"\n",
        )
        .unwrap();
        assert!(patch_probe(Some(home.path())).patch_tool_usable);

        fs::write(
            home.path().join("config.toml"),
            "[windows]\nsandbox = \"unelevated\"\n",
        )
        .unwrap();
        let probe = patch_probe(Some(home.path()));
        assert_eq!(probe.patch_tool_usable, cfg!(not(windows)));
        if cfg!(windows) {
            assert_eq!(probe.sandbox_mode.as_deref(), Some("unelevated"));
            assert!(probe.fix.is_some());
        }
    }
}
