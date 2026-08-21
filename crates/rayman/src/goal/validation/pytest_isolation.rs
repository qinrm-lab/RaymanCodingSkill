use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Output;

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

use super::{ParsedValidationCommand, pytest_invocation, python_pytest_module_index};

struct ManagedPytestLeaseGuard {
    root: PathBuf,
    storage: ManagedPytestLeaseStorage,
    lease: Option<crate::temp::ManagedPytestLease>,
}

#[derive(Clone, Copy)]
#[cfg_attr(not(test), allow(dead_code))]
enum ManagedPytestLeaseStorage {
    Workspace,
    External,
}

impl ManagedPytestLeaseGuard {
    fn prepare(root: &Path, storage: ManagedPytestLeaseStorage) -> Result<Self> {
        let lease = match storage {
            ManagedPytestLeaseStorage::Workspace => {
                crate::temp::create_managed_pytest_lease(root, "goal-validation")?
            }
            ManagedPytestLeaseStorage::External => {
                crate::temp::create_external_managed_pytest_lease(root, "p")?
            }
        };
        Ok(Self {
            root: root.to_path_buf(),
            storage,
            lease: Some(lease),
        })
    }

    fn lease(&self) -> Result<&crate::temp::PytestLease> {
        let lease = self
            .lease
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("pytest lease 已释放"))?;
        lease.verify_current()?;
        Ok(lease.lease())
    }

    fn finish(&mut self) -> Result<()> {
        let Some(lease) = self.lease.take() else {
            return Ok(());
        };
        match self.storage {
            ManagedPytestLeaseStorage::Workspace => {
                crate::temp::release_managed_pytest_lease(&self.root, &lease)
            }
            ManagedPytestLeaseStorage::External => {
                crate::temp::release_external_managed_pytest_lease(&self.root, &lease)
            }
        }
    }
}

impl Drop for ManagedPytestLeaseGuard {
    fn drop(&mut self) {
        if let Some(lease) = self.lease.take() {
            let _ = match self.storage {
                ManagedPytestLeaseStorage::Workspace => {
                    crate::temp::release_managed_pytest_lease(&self.root, &lease)
                }
                ManagedPytestLeaseStorage::External => {
                    crate::temp::release_external_managed_pytest_lease(&self.root, &lease)
                }
            };
        }
    }
}

fn pytest_argument_start(command: &ParsedValidationCommand) -> Option<usize> {
    if !pytest_invocation(command) {
        return None;
    }
    python_pytest_module_index(command).or(Some(0))
}

fn pytest_pre_separator_arguments(command: &ParsedValidationCommand) -> Option<&[String]> {
    let start = pytest_argument_start(command)?;
    let arguments = &command.args[start..];
    let end = arguments
        .iter()
        .position(|argument| argument == "--")
        .unwrap_or(arguments.len());
    Some(&arguments[..end])
}

fn managed_ini_override(argument: &str) -> bool {
    let key = argument
        .split_once('=')
        .map_or(argument, |(key, _)| key)
        .trim()
        .to_ascii_lowercase();
    matches!(key.as_str(), "cache_dir" | "addopts")
}

fn clustered_ini_override(arguments: &[String], index: usize) -> Option<String> {
    let argument = arguments.get(index)?;
    let flags = argument.strip_prefix('-')?;
    if flags.is_empty() || flags.starts_with('-') {
        return None;
    }
    let lowered = flags.to_ascii_lowercase();
    for (offset, flag) in lowered.char_indices() {
        // Plugin-defined valueless short flags can legally precede pytest's
        // `-o` in one cluster (xdist's `-d` is a common example). There is no
        // closed plugin flag registry to whitelist here, so fail closed on an
        // attached managed key after any `o` in a single-dash token.
        if flag != 'o' {
            continue;
        }
        let attached = &lowered[offset + flag.len_utf8()..];
        if !attached.is_empty() && managed_ini_override(attached) {
            return Some(argument.clone());
        }
        if attached.is_empty()
            && let Some(value) = arguments.get(index + 1)
            && managed_ini_override(value)
        {
            return Some(format!("{argument} {value}"));
        }
    }
    None
}

fn python_xoption_overrides_pycache(argument: &str) -> bool {
    argument
        .trim_start_matches('=')
        .split_once('=')
        .map_or(argument.trim_start_matches('='), |(key, _)| key)
        .eq_ignore_ascii_case("pycache_prefix")
}

fn python_isolation_override(command: &ParsedValidationCommand) -> Option<String> {
    let pytest_start = python_pytest_module_index(command)?;
    let interpreter_end = pytest_start.checked_sub(2)?;
    let arguments = &command.args[..interpreter_end];
    let mut index = 0;
    while let Some(argument) = arguments.get(index) {
        let Some(flags) = argument.strip_prefix('-').filter(|flags| !flags.is_empty()) else {
            index += 1;
            continue;
        };

        if let Some(rest) = flags.strip_prefix('W') {
            index += if rest.is_empty() { 2 } else { 1 };
            continue;
        }
        if let Some(rest) = flags.strip_prefix('X') {
            if rest.is_empty() {
                if let Some(value) = arguments.get(index + 1)
                    && python_xoption_overrides_pycache(value)
                {
                    return Some(format!("{argument} {value}"));
                }
                index += 2;
                continue;
            }
            if python_xoption_overrides_pycache(rest) {
                return Some(argument.clone());
            }
            index += 1;
            continue;
        }
        if flags.chars().any(|flag| matches!(flag, 'E' | 'I')) {
            return Some(argument.clone());
        }
        index += 1;
    }
    None
}

fn pytest_argument_file(command: &ParsedValidationCommand) -> Option<String> {
    pytest_pre_separator_arguments(command)?
        .iter()
        .find(|argument| argument.starts_with('@'))
        .cloned()
}

fn managed_override(command: &ParsedValidationCommand) -> Option<String> {
    let arguments = pytest_pre_separator_arguments(command)?;
    let mut index = 0;
    while let Some(argument) = arguments.get(index) {
        let lowered = argument.to_ascii_lowercase();
        if lowered == "--basetemp" || lowered.starts_with("--basetemp=") {
            return Some(argument.clone());
        }

        if matches!(lowered.as_str(), "-o" | "--override-ini") {
            if let Some(value) = arguments.get(index + 1)
                && managed_ini_override(value)
            {
                return Some(format!("{argument} {value}"));
            }
            index += 2;
            continue;
        }

        let inline = lowered
            .strip_prefix("--override-ini=")
            .or_else(|| lowered.strip_prefix("-o="))
            .or_else(|| lowered.strip_prefix("-o").filter(|value| !value.is_empty()));
        if inline.is_some_and(managed_ini_override) {
            return Some(argument.clone());
        }
        if let Some(clustered) = clustered_ini_override(arguments, index) {
            return Some(clustered);
        }
        index += 1;
    }
    None
}

pub(super) fn validate_pytest_isolation_overrides(command: &ParsedValidationCommand) -> Result<()> {
    if let Some(argument) = python_isolation_override(command) {
        bail!(
            "pytest 隔离由 Rayman 管理；Python -E/-I 或 -X pycache_prefix 会绕过受管 pycache: {argument}"
        );
    }
    if let Some(argument) = pytest_argument_file(command) {
        bail!("pytest 隔离由 Rayman 管理；不能使用会在检查后展开的 @argsfile: {argument}");
    }
    if let Some(argument) = managed_override(command) {
        bail!("pytest 隔离由 Rayman 管理；不能覆盖 --basetemp、cache_dir 或 addopts: {argument}");
    }
    Ok(())
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

pub(super) fn pytest_has_pre_separator_option(
    command: &ParsedValidationCommand,
    options: &[&str],
) -> bool {
    pytest_pre_separator_arguments(command).is_some_and(|arguments| {
        arguments.iter().any(|argument| {
            let name = argument.split('=').next().unwrap_or(argument);
            options.contains(&name)
        })
    })
}

pub(super) fn insert_pytest_args_before_separator(
    command: &mut ParsedValidationCommand,
    arguments: impl IntoIterator<Item = String>,
) -> Result<()> {
    let start = pytest_argument_start(command)
        .ok_or_else(|| anyhow::anyhow!("受管 pytest 参数只能注入 pytest 调用"))?;
    let insertion = command.args[start..]
        .iter()
        .position(|argument| argument == "--")
        .map_or(command.args.len(), |offset| start + offset);
    command.args.splice(insertion..insertion, arguments);
    Ok(())
}

fn managed_pytest_command(
    command: &ParsedValidationCommand,
    pytest_args: &[String],
) -> Result<ParsedValidationCommand> {
    validate_pytest_isolation_overrides(command)?;
    let mut executable = command.clone();
    insert_pytest_args_before_separator(&mut executable, pytest_args.iter().cloned())?;
    // This must be the final pre-separator override. Clearing only the
    // inherited PYTEST_ADDOPTS environment leaves pytest.ini, pyproject.toml,
    // setup.cfg and tox.ini free to inject selectors or non-executing modes.
    insert_pytest_args_before_separator(&mut executable, ["-o".into(), "addopts=".into()])?;
    Ok(executable)
}

/// Execute one physical pytest process inside one freshly created, probed and
/// manifest-owned lease. Every caller invocation gets a different lease, so
/// collect, run and authority repeats cannot share temp or cache state.
///
/// The logical command remains caller-owned. Only the cloned physical argv and
/// process environment reach `runner`, and release completes before a result is
/// returned. Cleanup failure therefore prevents every receipt-writing caller
/// from observing a successful execution.
pub fn run_with_managed_pytest_lease(
    root: &Path,
    command: &ParsedValidationCommand,
    runner: impl FnOnce(&ParsedValidationCommand, Option<&BTreeMap<String, String>>) -> Result<Output>,
) -> Result<Output> {
    run_with_managed_pytest_lease_storage(
        root,
        command,
        ManagedPytestLeaseStorage::Workspace,
        runner,
    )
}

#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn run_with_external_managed_pytest_lease(
    root: &Path,
    command: &ParsedValidationCommand,
    runner: impl FnOnce(&ParsedValidationCommand, Option<&BTreeMap<String, String>>) -> Result<Output>,
) -> Result<Output> {
    run_with_managed_pytest_lease_storage(
        root,
        command,
        ManagedPytestLeaseStorage::External,
        runner,
    )
}

pub(super) fn run_with_external_managed_pytest_lease_at_host_root(
    host_root: &crate::temp::ValidationProcessStateRoot,
    command: &ParsedValidationCommand,
    runner: impl FnOnce(&ParsedValidationCommand, Option<&BTreeMap<String, String>>) -> Result<Output>,
) -> Result<Output> {
    if !pytest_invocation(command) {
        host_root.verify_current()?;
        let result = runner(command, None);
        host_root.verify_current()?;
        return result;
    }
    validate_pytest_isolation_overrides(command)?;
    host_root.verify_current()?;
    let lease = crate::temp::create_external_managed_pytest_lease_at_host_root(host_root, "p")?;
    let execution = (|| -> Result<Output> {
        host_root.verify_current()?;
        lease.verify_current_at_host_root(host_root)?;
        host_root.verify_current()?;
        let logical = lease.lease();
        let executable = managed_pytest_command(command, &logical.pytest_args)?;
        runner(&executable, Some(&logical.environment))
    })();
    let cleanup =
        crate::temp::release_external_managed_pytest_lease_at_host_root(host_root, &lease);
    let root_recheck = host_root.verify_current();
    match (execution, cleanup, root_recheck) {
        (Ok(output), Ok(()), Ok(())) => Ok(output),
        (Ok(output), Err(cleanup_error), Ok(())) if output.status.success() => bail!(
            "pytest 验证结束后无法释放受管 lease；stdout_sha256={} stderr_sha256={}: {cleanup_error:#}",
            sha256_hex(&output.stdout),
            sha256_hex(&output.stderr)
        ),
        (Ok(output), Err(cleanup_error), Ok(())) => bail!(
            "pytest 验证进程非零退出（exit={}）且 lease 释放失败；stdout_sha256={} stderr_sha256={}: {cleanup_error:#}",
            output.status.code().unwrap_or(-1),
            sha256_hex(&output.stdout),
            sha256_hex(&output.stderr)
        ),
        (Err(error), Ok(()), Ok(())) => Err(error),
        (Err(error), Err(cleanup_error), Ok(())) => bail!(
            "pytest 执行准备或启动失败: {error:#}; lease 释放也失败: {cleanup_error:#}"
        ),
        (Ok(_), Ok(()), Err(root_error)) => Err(root_error.context(
            "pytest validation child completed but validation host-temp root identity changed",
        )),
        (Ok(output), Err(cleanup_error), Err(root_error)) => Err(root_error).context(format!(
            "pytest validation child completed with lease cleanup failure; stdout_sha256={} stderr_sha256={}: {cleanup_error:#}",
            sha256_hex(&output.stdout),
            sha256_hex(&output.stderr)
        )),
        (Err(error), Ok(()), Err(root_error)) => Err(error).context(format!(
            "validation host-temp root identity changed during pytest execution: {root_error:#}"
        )),
        (Err(error), Err(cleanup_error), Err(root_error)) => Err(error).context(format!(
            "validation process lease cleanup and host-temp root identity both failed: cleanup={cleanup_error:#}; root={root_error:#}"
        )),
    }
}

fn run_with_managed_pytest_lease_storage(
    root: &Path,
    command: &ParsedValidationCommand,
    storage: ManagedPytestLeaseStorage,
    runner: impl FnOnce(&ParsedValidationCommand, Option<&BTreeMap<String, String>>) -> Result<Output>,
) -> Result<Output> {
    if !pytest_invocation(command) {
        return runner(command, None);
    }
    validate_pytest_isolation_overrides(command)?;

    // create_pytest_lease finishes by verifying the manifest and probing every
    // directory. The physical command is built only from that verified object.
    let mut guard = ManagedPytestLeaseGuard::prepare(root, storage)?;
    let lease = guard.lease()?;
    // Keep command construction inside the operation result. Even an internal
    // construction failure after lease creation must pass through cleanup.
    let execution = managed_pytest_command(command, &lease.pytest_args)
        .and_then(|executable| runner(&executable, Some(&lease.environment)));
    let cleanup = guard.finish();

    match (execution, cleanup) {
        (Ok(output), Ok(())) => Ok(output),
        (Ok(output), Err(cleanup_error)) if output.status.success() => {
            bail!(
                "pytest 验证结束后无法释放受管 lease；stdout_sha256={} stderr_sha256={}: {cleanup_error:#}",
                sha256_hex(&output.stdout),
                sha256_hex(&output.stderr)
            )
        }
        (Ok(output), Err(cleanup_error)) => bail!(
            "pytest 验证进程非零退出（exit={}）且 lease 释放失败；stdout_sha256={} stderr_sha256={}: {cleanup_error:#}",
            output.status.code().unwrap_or(-1),
            sha256_hex(&output.stdout),
            sha256_hex(&output.stderr)
        ),
        (Err(execution_error), Ok(())) => Err(execution_error),
        (Err(execution_error), Err(cleanup_error)) => bail!(
            "pytest 执行准备或启动失败: {execution_error:#}; lease 释放也失败: {cleanup_error:#}"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn process_output(exit_code: i32) -> Output {
        #[cfg(unix)]
        let status = {
            use std::os::unix::process::ExitStatusExt;
            std::process::ExitStatus::from_raw(exit_code << 8)
        };
        #[cfg(windows)]
        let status = {
            use std::os::windows::process::ExitStatusExt;
            std::process::ExitStatus::from_raw(exit_code as u32)
        };
        Output {
            status,
            stdout: Vec::new(),
            stderr: Vec::new(),
        }
    }

    fn corrupt_lease_manifest(environment: Option<&BTreeMap<String, String>>) {
        let temp = environment
            .and_then(|values| values.get("TEMP"))
            .expect("managed TEMP");
        let lease_root = Path::new(temp).parent().expect("lease root");
        std::fs::write(lease_root.join("lease.json"), b"{}").unwrap();
    }

    fn injected(command: &str) -> ParsedValidationCommand {
        let parsed = super::super::parse_validation_command(command).unwrap();
        managed_pytest_command(
            &parsed,
            &[
                "--basetemp".into(),
                "managed/base".into(),
                "-o".into(),
                "cache_dir=managed/cache".into(),
            ],
        )
        .unwrap()
    }

    #[test]
    fn managed_arguments_target_every_supported_pytest_shape_before_separator() {
        for command in [
            "pytest -q -- tests/test_api.py",
            "python -m pytest -q -- tests/test_api.py",
            "py -3.12 -m pytest -q -- tests/test_api.py",
        ] {
            let injected = injected(command);
            let separator = injected.args.iter().position(|arg| arg == "--").unwrap();
            let basetemp = injected
                .args
                .iter()
                .position(|arg| arg == "--basetemp")
                .unwrap();
            let cache = injected
                .args
                .iter()
                .position(|arg| arg == "cache_dir=managed/cache")
                .unwrap();
            let addopts = injected
                .args
                .iter()
                .position(|arg| arg == "addopts=")
                .unwrap();
            assert!(basetemp < separator, "{command}: {:?}", injected.args);
            assert!(cache < separator, "{command}: {:?}", injected.args);
            assert!(addopts < separator, "{command}: {:?}", injected.args);
            assert!(cache < addopts, "{command}: {:?}", injected.args);
            assert_eq!(
                injected
                    .args
                    .iter()
                    .filter(|arg| *arg == "addopts=")
                    .count(),
                1,
                "{command}: {:?}",
                injected.args
            );
            assert_eq!(injected.args.last().unwrap(), "tests/test_api.py");
        }
    }

    #[test]
    fn user_managed_isolation_overrides_are_rejected_before_separator_only() {
        for override_args in [
            "--basetemp user",
            "--basetemp=user",
            "-o cache_dir=user",
            "-ocache_dir=user",
            "-o=cache_dir=user",
            "-qocache_dir=user",
            "-docache_dir=user",
            "-doaddopts=-x",
            "-zocache_dir=user",
            "-vvo addopts=-x",
            "--override-ini cache_dir=user",
            "--override-ini=cache_dir=user",
            "-o addopts=-x",
            "--override-ini=addopts=-x",
        ] {
            let parsed = super::super::parse_validation_command(&format!(
                "python -m pytest {override_args}"
            ))
            .unwrap();
            assert!(
                validate_pytest_isolation_overrides(&parsed).is_err(),
                "must reject {override_args}"
            );
        }

        let unrelated = super::super::parse_validation_command(
            "python -m pytest -o log_cli=true -- --basetemp",
        )
        .unwrap();
        assert!(validate_pytest_isolation_overrides(&unrelated).is_ok());

        for command in [
            "python -E -m pytest",
            "python -Is -m pytest",
            "python -X pycache_prefix=outside -m pytest",
            "python -Xpycache_prefix=outside -m pytest",
            "pytest @managed-overrides.txt",
        ] {
            let parsed = super::super::parse_validation_command(command).unwrap();
            assert!(
                validate_pytest_isolation_overrides(&parsed).is_err(),
                "must reject {command}"
            );
        }

        for command in [
            "python -m pytest -- -qocache_dir=user",
            "python -m pytest -- @literal-selector.txt",
        ] {
            let parsed = super::super::parse_validation_command(command).unwrap();
            assert!(
                validate_pytest_isolation_overrides(&parsed).is_ok(),
                "separator must make this positional: {command}"
            );
        }
    }

    #[test]
    fn spawn_failure_still_releases_the_fresh_lease() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join(".RaymanCodingSkill")).unwrap();
        let command = super::super::parse_validation_command("python -m pytest").unwrap();
        let error = run_with_managed_pytest_lease(root.path(), &command, |_, environment| {
            assert!(environment.is_some());
            Err::<Output, _>(anyhow::anyhow!("simulated spawn failure"))
        })
        .unwrap_err();
        assert!(error.to_string().contains("simulated spawn failure"));
        let leases = root.path().join(".RaymanCodingSkill/tmp/leases");
        assert!(
            !leases.exists() || std::fs::read_dir(leases).unwrap().next().is_none(),
            "spawn failure left a lease behind"
        );
    }

    #[test]
    fn successful_execution_releases_the_fresh_lease() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join(".RaymanCodingSkill")).unwrap();
        let command = super::super::parse_validation_command("pytest -q").unwrap();

        let output = run_with_managed_pytest_lease(root.path(), &command, |_, environment| {
            assert!(environment.is_some());
            Ok(process_output(0))
        })
        .unwrap();

        assert!(output.status.success());
        let leases = root.path().join(".RaymanCodingSkill/tmp/leases");
        assert!(!leases.exists() || std::fs::read_dir(leases).unwrap().next().is_none());
    }

    #[test]
    fn external_execution_uses_a_marker_free_tree_and_releases_on_success() {
        let root = tempfile::tempdir().unwrap();
        let command = super::super::parse_validation_command("pytest -q").unwrap();
        let mut observed_root = None;

        let output =
            run_with_external_managed_pytest_lease(root.path(), &command, |_, environment| {
                let temp = environment
                    .and_then(|values| values.get("TEMP"))
                    .expect("managed TEMP");
                let lease_root = PathBuf::from(temp).parent().unwrap().to_path_buf();
                assert!(
                    lease_root
                        .canonicalize()
                        .unwrap()
                        .starts_with(root.path().join("p").canonicalize().unwrap())
                );
                assert!(!root.path().join(".RaymanCodingSkill").exists());
                observed_root = Some(lease_root);
                Ok(process_output(0))
            })
            .unwrap();

        assert!(output.status.success());
        assert!(!observed_root.expect("lease root was observed").exists());
        assert!(!root.path().join(".RaymanCodingSkill").exists());
    }

    #[cfg(windows)]
    #[test]
    fn external_pre_release_leaf_replacement_blocks_success() {
        let root = tempfile::tempdir().unwrap();
        let command = super::super::parse_validation_command("pytest -q").unwrap();
        let mut observed = None;

        let error =
            run_with_external_managed_pytest_lease(root.path(), &command, |_, environment| {
                let temp = environment
                    .and_then(|values| values.get("TEMP"))
                    .expect("managed TEMP");
                let lease_root = PathBuf::from(temp).parent().unwrap().to_path_buf();
                let manifest = std::fs::read(lease_root.join("lease.json")).unwrap();
                let id = lease_root.file_name().unwrap().to_string_lossy();
                let displaced = lease_root.with_file_name(format!("{id}-displaced"));
                std::fs::rename(&lease_root, &displaced).unwrap();
                std::fs::create_dir(&lease_root).unwrap();
                for name in ["b", "c", "t", "y", "n"] {
                    std::fs::create_dir(lease_root.join(name)).unwrap();
                }
                std::fs::write(lease_root.join("lease.json"), manifest).unwrap();
                observed = Some((displaced, lease_root));
                Ok(process_output(0))
            })
            .unwrap_err();

        assert!(format!("{error:#}").contains("创建目录"), "{error:#}");
        let (displaced, replacement) = observed.expect("replacement paths");
        assert!(
            displaced.is_dir() && replacement.is_dir(),
            "identity mismatch must preserve both the original object and replacement"
        );
        assert!(!root.path().join(".RaymanCodingSkill").exists());
    }

    #[cfg(windows)]
    #[test]
    fn external_pre_spawn_leaf_replacement_blocks_runner() {
        let root = tempfile::tempdir().unwrap();
        let mut guard =
            ManagedPytestLeaseGuard::prepare(root.path(), ManagedPytestLeaseStorage::External)
                .unwrap();
        let lease = guard.lease().unwrap().clone();
        let original = PathBuf::from(&lease.root);
        let displaced = original.with_file_name(format!("{}-displaced", lease.id));
        std::fs::rename(&original, &displaced).unwrap();
        std::fs::create_dir(&original).unwrap();
        for name in ["b", "c", "t", "y", "n"] {
            std::fs::create_dir(original.join(name)).unwrap();
        }
        crate::file_io::write_json(&original.join("lease.json"), &lease).unwrap();

        let mut runner_called = false;
        let error = guard
            .lease()
            .map(|_| {
                runner_called = true;
            })
            .unwrap_err();
        assert!(format!("{error:#}").contains("创建目录"), "{error:#}");
        assert!(
            !runner_called,
            "runner must not receive a replacement lease"
        );
        assert!(guard.finish().is_err());
        assert!(displaced.is_dir() && original.is_dir());
        assert!(!root.path().join(".RaymanCodingSkill").exists());
    }

    #[test]
    fn panic_unwind_uses_the_best_effort_release_guard() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join(".RaymanCodingSkill")).unwrap();
        let command = super::super::parse_validation_command("pytest -q").unwrap();
        let mut observed_root = None;

        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = run_with_managed_pytest_lease(root.path(), &command, |_, environment| {
                let temp = environment
                    .and_then(|values| values.get("TEMP"))
                    .expect("managed TEMP");
                observed_root = Some(PathBuf::from(temp).parent().unwrap().to_path_buf());
                panic!("simulated validation panic");
            });
        }));

        assert!(panic.is_err());
        assert!(!observed_root.expect("lease root was observed").exists());
    }

    #[test]
    fn external_panic_unwind_releases_the_same_storage_kind() {
        let root = tempfile::tempdir().unwrap();
        let command = super::super::parse_validation_command("pytest -q").unwrap();
        let mut observed_root = None;

        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ =
                run_with_external_managed_pytest_lease(root.path(), &command, |_, environment| {
                    let temp = environment
                        .and_then(|values| values.get("TEMP"))
                        .expect("managed TEMP");
                    observed_root = Some(PathBuf::from(temp).parent().unwrap().to_path_buf());
                    panic!("simulated external validation panic");
                });
        }));

        assert!(panic.is_err());
        assert!(!observed_root.expect("lease root was observed").exists());
        assert!(!root.path().join(".RaymanCodingSkill").exists());
    }

    #[test]
    fn missing_managed_lease_blocks_a_successful_result() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join(".RaymanCodingSkill")).unwrap();
        let command = super::super::parse_validation_command("pytest -q").unwrap();

        let error = run_with_managed_pytest_lease(root.path(), &command, |_, environment| {
            let temp = environment
                .and_then(|values| values.get("TEMP"))
                .expect("managed TEMP");
            let lease_root = Path::new(temp).parent().expect("lease root");
            std::fs::remove_dir_all(lease_root).unwrap();
            Ok(process_output(0))
        })
        .unwrap_err();
        let rendered = format!("{error:#}");

        assert!(rendered.contains("无法释放受管 lease"), "{rendered}");
        assert!(
            rendered.contains("消失")
                || rendered.contains("不存在")
                || rendered.contains("系统找不到指定的文件")
                || rendered.contains("The system cannot find the file specified"),
            "{rendered}"
        );
    }

    #[test]
    fn cleanup_failure_rejects_a_successful_execution() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join(".RaymanCodingSkill")).unwrap();
        let command = super::super::parse_validation_command("pytest -q").unwrap();

        let error = run_with_managed_pytest_lease(root.path(), &command, |_, environment| {
            corrupt_lease_manifest(environment);
            Ok(process_output(0))
        })
        .unwrap_err();

        assert!(format!("{error:#}").contains("无法释放受管 lease"));
    }

    #[test]
    fn nonzero_execution_and_cleanup_failure_are_reported_together() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join(".RaymanCodingSkill")).unwrap();
        let command = super::super::parse_validation_command("pytest -q").unwrap();

        let error = run_with_managed_pytest_lease(root.path(), &command, |_, environment| {
            corrupt_lease_manifest(environment);
            Ok(process_output(37))
        })
        .unwrap_err();
        let rendered = format!("{error:#}");

        assert!(rendered.contains("exit=37"), "{rendered}");
        assert!(rendered.contains("lease 释放失败"), "{rendered}");
        assert!(rendered.contains("stdout_sha256="), "{rendered}");
        assert!(rendered.contains("stderr_sha256="), "{rendered}");
    }
}
