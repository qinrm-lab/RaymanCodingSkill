use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde_json::json;

use crate::cli::DoctorCmd;

pub(crate) fn run(root: &Path, json_output: bool, cmd: DoctorCmd) -> Result<()> {
    let running = std::env::current_exe().context("无法定位当前 rayman 二进制")?;
    let running_hash = rayman::hash::sha256_file(&running)?;
    let path_candidate = rayman::toolchain::resolve_shell_command("rayman");
    let path_hash = path_candidate
        .as_deref()
        .map(rayman::hash::sha256_file)
        .transpose()?;
    let path_matches_running = path_hash.as_deref() == Some(running_hash.as_str());

    let activation = rayman::workspace::activation_status(root)?;
    let state_write = rayman::state_paths::state_write_probe(root);
    let host_patch = rayman::codex_host::patch_probe(None);
    let toolchain = rayman::toolchain::toolchain_probe(root);
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
    // Compare the recorded and on-disk hashes on their own terms. Folding
    // `activation.active` into this made it unreachable as a *cause*: SKILL
    // drift is exactly what clears `active`, so a drifted SKILL.md was reported
    // as "workspace 未激活" while the same output printed status `invalid`.
    let hashes_match = match (&skill_hash, &metadata_hash) {
        (Some(actual), Some(expected)) => actual.eq_ignore_ascii_case(expected),
        _ => false,
    };
    // Drift means "a hash was recorded and the file no longer matches it".
    // Reporting drift whenever a contract file merely exists misdiagnosed the
    // deactivated workspace — `deactivate` writes a contract with no
    // skill_file/skill_sha256 at all — and made the "not activated" cause
    // unreachable for it.
    //
    // Only the RECORDED hash may gate this. Also requiring the on-disk hash
    // meant a bound SKILL.md that was deleted or replaced by a link — which
    // leaves `actual_sha256` as None — stopped being diagnosed as drift and
    // fell back to generic "not activated" advice, the same misdiagnosis in
    // the other direction.
    let hashes_recorded = metadata_hash.is_some();
    let metadata_matches = activation.active && hashes_match;
    // Doctor proves the installed identity tuple only. Source-to-artifact byte
    // identity belongs to the explicit clean-checkout repository verifier.
    let identity_ready = path_matches_running && activation.active && metadata_matches;
    let report = json!({
        "workspace_activation": &activation,
        "state_write": &state_write,
        "host_patch": &host_patch,
        "toolchain": &toolchain,
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
        print_toolchain(&toolchain);
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
        // Naming one cause when three are independent sent readers at the wrong
        // repair: the usual failure is simply that this process has no `rayman`
        // on PATH (the installer updates the persistent PATH, which an already
        // running shell never inherits), and the old text told them to edit
        // skill_sha256 instead.
        let mut causes = Vec::new();
        if !path_matches_running {
            causes.push(if path_candidate.is_none() {
                "PATH 上找不到 rayman：安装器只改持久化 PATH，已经开着的终端不会继承；新开一个终端，或先把安装目录加进本进程 PATH"
            } else {
                "PATH 上的 rayman 与当前运行的二进制不是同一份：用仓库 release 二进制重新安装"
            });
        }
        // Drift is the more specific diagnosis and must be reported first: it is
        // what cleared `active` in the first place, so testing activation first
        // buried it under generic "not activated" advice.
        if hashes_recorded && !hashes_match {
            causes.push("workspace SKILL.md 与记录的 skill_sha256 不一致：SKILL.md 改动后需重新 activate 重绑");
        } else if !activation.active {
            causes.push("workspace 未激活：运行 `rayman workspace activate --skill-file <canonical-SKILL.md> --yes`");
        }
        bail!("已安装身份契约不一致：{}", causes.join("；"));
    }
    Ok(())
}

/// Reachability is reported before any gate that depends on it, so a missing
/// toolchain is diagnosed here instead of surfacing later as an unexplained
/// workspace blocker.
fn print_toolchain(probes: &[rayman::toolchain::ToolProbe]) {
    for probe in probes {
        // Print the state as its own complete authored line rather than nesting
        // it in the tool line. `required_for` is itself authored Chinese that
        // already contains full-width parens, so wrapping it in another pair
        // produced a capture no template could match and the state stayed
        // Chinese under en for exactly the tool whose reason is longest.
        match (probe.found, probe.relevant) {
            (true, _) => match probe.path.as_deref() {
                Some(path) => println!("  工具 {}: {}", probe.name, path),
                None => println!("  工具 {}: 已找到", probe.name),
            },
            (false, true) => {
                println!("  工具 {}: 不可达", probe.name);
                println!("    需要它来: {}", probe.required_for);
                // The JSON report carries `unspawnable_shim`, but the human
                // surface dropped it, so a `.bat`/`.cmd` shim on PATH looked
                // identical to a missing tool and got repair advice that cannot
                // work. `unreachable_tool_advice` already distinguishes the two
                // and both of its strings are registered for en.
                println!(
                    "    {}",
                    rayman::toolchain::unreachable_tool_advice(probe.name)
                );
            }
            (false, false) => println!("  工具 {}: 不可达，本工作区不需要", probe.name),
        }
    }
}
