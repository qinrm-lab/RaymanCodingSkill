use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde_json::json;

use crate::cli::DoctorCmd;

pub(crate) fn run(root: &Path, json_output: bool, cmd: DoctorCmd) -> Result<()> {
    let running = std::env::current_exe().context("无法定位当前 rayman 二进制")?;
    let running_hash = rayman::hash::sha256_file(&running)?;
    let path_candidate = find_path_rayman();
    let path_hash = path_candidate
        .as_deref()
        .map(rayman::hash::sha256_file)
        .transpose()?;
    let path_matches_running = path_hash.as_deref() == Some(running_hash.as_str());

    let activation = rayman::workspace::activation_status(root)?;
    let state_write = rayman::state_paths::state_write_probe(root);
    let host_patch = rayman::codex_host::patch_probe(None);
    let skill_path = activation
        .skill_file
        .as_deref()
        .map(PathBuf::from)
        .map(|path| {
            if path.is_absolute() {
                path
            } else {
                root.join(path)
            }
        })
        .unwrap_or_else(|| root.join("SKILL.md"));
    let skill_hash = activation.actual_sha256.clone();
    let metadata_hash = activation.expected_sha256.clone();
    let metadata_matches = activation.active
        && match (&skill_hash, &metadata_hash) {
            (Some(actual), Some(expected)) => actual.eq_ignore_ascii_case(expected),
            _ => false,
        };
    // Doctor proves the installed identity tuple only. Source-to-artifact byte
    // identity belongs to the explicit clean-checkout repository verifier.
    let identity_ready = path_matches_running && activation.active && metadata_matches;
    let report = json!({
        "workspace_activation": &activation,
        "state_write": &state_write,
        "host_patch": &host_patch,
        "contract": rayman::CLI_CONTRACT,
        "version": rayman::CLI_VERSION,
        "running": {
            "path": running,
            "sha256": running_hash,
        },
        "path_rayman": path_candidate.as_ref().map(|path| json!({
            "path": path,
            "sha256": path_hash,
            "matches_running": path_matches_running,
        })),
        "repo_release": {
            "checked": false,
            "status": "not_checked_by_doctor",
            "required_verifier": crate::SOURCE_FRESH_VERIFIER,
        },
        "workspace_skill": {
            "path": skill_path,
            "sha256": skill_hash,
            "recorded_sha256": metadata_hash,
            "matches_recorded": metadata_matches,
        },
        "release_identity": {
            "ready": identity_ready,
            "scope": "running_binary_path_command_and_workspace_skill_identity",
        },
        "source_fresh": {
            "verified": false,
            "status": "not_checked_by_doctor",
            "required_verifier": crate::SOURCE_FRESH_VERIFIER,
        },
    });
    if json_output {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "已安装身份契约: {} v{}",
            rayman::CLI_CONTRACT,
            rayman::CLI_VERSION
        );
        println!("  当前二进制: {}", running.display());
        println!("  PATH 命令一致: {path_matches_running}");
        println!("  workspace activation: {}", activation.status);
        crate::print_state_write_probe(&state_write);
        crate::print_host_patch_probe(&host_patch);
        println!(
            "  仓库源码产物: 未由 doctor 检查；交接/CI 由 `{}` 验证",
            crate::SOURCE_FRESH_VERIFIER
        );
        println!("  workspace SKILL 一致: {metadata_matches}");
        println!("  已安装身份 READY: {identity_ready}");
        println!(
            "  源码新鲜度: 未由 doctor 证明；交接/CI 必须运行 `{}`",
            crate::SOURCE_FRESH_VERIFIER
        );
    }
    if cmd.check && !identity_ready {
        bail!(
            "已安装身份契约不一致：请使用仓库 release 二进制同步安装，并更新 .RaymanCodingSkill/workspace_skill.yaml 的 skill_sha256"
        );
    }
    Ok(())
}

#[cfg(windows)]
fn find_path_rayman() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
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
    std::env::split_paths(&path).find_map(|dir| {
        extensions
            .iter()
            .map(|extension| dir.join(format!("rayman{extension}")))
            .find(|candidate| candidate.is_file())
    })
}

#[cfg(not(windows))]
fn find_path_rayman() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join("rayman"))
        .find(|candidate| candidate.is_file())
}
