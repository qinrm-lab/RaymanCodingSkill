use std::collections::BTreeMap;
use std::path::Path;
use std::process::Output;

use anyhow::Result;

use super::pytest_isolation::run_with_external_managed_pytest_lease_at_host_root;
use super::{ParsedValidationCommand, pytest_invocation, run_with_managed_pytest_lease};

/// Execute one physical validation child with an independently probed temp
/// lease. Pytest already owns a richer per-process lease, so Windows relocates
/// that one lease to the external host-temp root instead of nesting a second
/// generic lease whose TEMP variables pytest would overwrite.
pub fn run_with_managed_validation_temp(
    root: &Path,
    command: &ParsedValidationCommand,
    runner: impl FnOnce(&ParsedValidationCommand, Option<&BTreeMap<String, String>>) -> Result<Output>,
) -> Result<Output> {
    if pytest_invocation(command) {
        if cfg!(windows) {
            let host_root = crate::temp::acquire_validation_process_state_root(root)?;
            return run_with_external_managed_pytest_lease_at_host_root(
                &host_root, command, runner,
            );
        }
        return run_with_managed_pytest_lease(root, command, runner);
    }

    crate::temp::run_with_validation_process_lease(root, "v", |environment| {
        runner(command, environment)
    })
}

pub fn test_invocation_requires_pytest_isolation(command: &ParsedValidationCommand) -> bool {
    pytest_invocation(command)
}
