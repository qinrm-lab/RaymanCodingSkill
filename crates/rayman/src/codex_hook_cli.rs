use anyhow::{Result, bail};

use crate::cli::{CodexHookAction, CodexHookCmd};

pub(crate) fn run(json_output: bool, command: &CodexHookCmd) -> Result<()> {
    let report = match &command.action {
        CodexHookAction::Stop => {
            // The block/allow decision travels only in the stdout JSON
            // ({"decision":"block",...} vs {"continue":true}); the process always exits 0.
            // These are Codex's published Stop/common output fields. Keep transient
            // Rayman observations internal: unknown top-level fields have no compatibility
            // promise. Installation still requires reviewing the exact hooks file and a
            // fresh host trust/restart boundary; exit code alone never proves enforcement.
            let response = rayman::codex_hook::run_stop_from_stdin();
            println!("{}", serde_json::to_string(&response)?);
            return Ok(());
        }
        CodexHookAction::Install { codex_home, yes } => {
            if !*yes {
                bail!("Codex hook installation writes hooks.json; add --yes to confirm");
            }
            let executable = std::env::current_exe()?.canonicalize()?;
            rayman::codex_hook::install(codex_home.as_deref(), &executable)?
        }
        CodexHookAction::Status { codex_home } => {
            rayman::codex_hook::status(codex_home.as_deref())?
        }
        CodexHookAction::Uninstall { codex_home, yes } => {
            if !*yes {
                bail!("Codex hook uninstall changes hooks.json; add --yes to confirm");
            }
            rayman::codex_hook::uninstall(codex_home.as_deref())?
        }
    };
    if json_output {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "Rayman Codex Stop guard: {} (changed={}, path={})",
            if report.installed {
                "installed"
            } else {
                "not installed"
            },
            report.changed,
            report.hooks_path
        );
        if let Some(command) = report.command {
            println!("  command: {command}");
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_and_uninstall_require_confirmation_before_writes() {
        let codex_home = tempfile::tempdir().unwrap();
        for action in [
            CodexHookAction::Install {
                codex_home: Some(codex_home.path().to_path_buf()),
                yes: false,
            },
            CodexHookAction::Uninstall {
                codex_home: Some(codex_home.path().to_path_buf()),
                yes: false,
            },
        ] {
            let error = run(false, &CodexHookCmd { action }).unwrap_err();
            assert!(error.to_string().contains("--yes"));
            assert!(!codex_home.path().join("hooks.json").exists());
        }
    }
}
