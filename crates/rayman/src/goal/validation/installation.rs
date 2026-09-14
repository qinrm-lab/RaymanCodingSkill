use super::*;

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
