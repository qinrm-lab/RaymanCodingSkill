use super::*;

const CODEX_POWERSHELL_BROKER_INSTALLER: &str = "scripts/install-codex-powershell-broker.ps1";

fn installer_self_test_switch(argument: &str) -> bool {
    let lowered = argument.to_ascii_lowercase();
    let name = lowered.split(':').next().unwrap_or(lowered.as_str());
    name.len() >= 2 && "-selftest".starts_with(name)
}

pub(super) fn release_installer_invocation(root: &Path, command: &ParsedValidationCommand) -> bool {
    trusted_gate_script(root, command) == Some("install-rayman.ps1")
        && !command
            .args
            .iter()
            .any(|argument| installer_self_test_switch(argument))
}

pub(super) fn release_installer_invocation_with_context(
    decision: &GoalDecisionContext<'_>,
    command: &ParsedValidationCommand,
) -> Result<bool> {
    Ok(
        trusted_gate_script_with_context(decision, command)? == Some("install-rayman.ps1")
            && !command
                .args
                .iter()
                .any(|argument| installer_self_test_switch(argument)),
    )
}

fn broker_install_args(command: &ParsedValidationCommand) -> bool {
    let exact_count = |needle: &str| {
        command
            .args
            .iter()
            .filter(|argument| argument.eq_ignore_ascii_case(needle))
            .count()
    };
    let is_switch_prefix = |argument: &str, switch: &str| {
        let lowered = argument.to_ascii_lowercase();
        let name = lowered.split(':').next().unwrap_or(lowered.as_str());
        name.len() >= 2 && switch.starts_with(name)
    };
    let has_non_exact_required_form = command.args.iter().any(|argument| {
        let lowered = argument.to_ascii_lowercase();
        let name = lowered.split(':').next().unwrap_or(lowered.as_str());
        matches!(name, "-install" | "-yes") && !matches!(lowered.as_str(), "-install" | "-yes")
    });
    let has_conflicting_mode = command.args.iter().any(|argument| {
        ["-check", "-selftest", "-uninstall"]
            .iter()
            .any(|switch| is_switch_prefix(argument, switch))
    });

    exact_count("-Install") == 1
        && exact_count("-Yes") == 1
        && !has_non_exact_required_form
        && !has_conflicting_mode
}

pub(super) fn broker_install_invocation(
    root: &Path,
    command: &ParsedValidationCommand,
) -> Result<bool> {
    if !broker_install_args(command) {
        return Ok(false);
    }
    Ok(resolve_live_powershell_script(root, command)?
        .is_some_and(|script| script.logical_key == CODEX_POWERSHELL_BROKER_INSTALLER))
}

pub(super) fn broker_install_invocation_with_context(
    decision: &GoalDecisionContext<'_>,
    command: &ParsedValidationCommand,
) -> Result<bool> {
    if !broker_install_args(command) {
        return Ok(false);
    }
    Ok(
        captured_workspace_powershell_key_with_context(decision, command)?
            .is_some_and(|key| key == CODEX_POWERSHELL_BROKER_INSTALLER),
    )
}
