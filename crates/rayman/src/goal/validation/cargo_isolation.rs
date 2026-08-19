use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

use super::{ParsedValidationCommand, cargo_subcommand};
use crate::temp::{
    ManagedCargoTargetLease, create_managed_cargo_target_lease, release_managed_cargo_target_lease,
};

const CARGO_TARGET_DIR: &str = "CARGO_TARGET_DIR";

/// Process-local environment for one complete validation operation. On
/// Windows, a Rayman CLI running from Cargo's effective target cannot safely
/// let a nested Cargo invocation relink that same image. One unique managed
/// target is therefore shared by list proof and every repeat, then released
/// before any receipt is persisted.
pub struct ValidationExecutionSession {
    root: PathBuf,
    inherited_target: Option<OsString>,
    lease: Option<ManagedCargoTargetLease>,
}

impl ValidationExecutionSession {
    pub fn prepare(root: &Path, command: &ParsedValidationCommand) -> Result<Self> {
        let inherited_target = std::env::var_os(CARGO_TARGET_DIR);
        if !cfg!(windows) {
            return Ok(Self {
                root: root.to_path_buf(),
                inherited_target,
                lease: None,
            });
        }
        let current_exe = std::env::current_exe().context("无法定位当前 rayman 二进制")?;
        Self::prepare_for(root, command, &current_exe, inherited_target, true)
    }

    fn prepare_for(
        root: &Path,
        command: &ParsedValidationCommand,
        current_exe: &Path,
        inherited_target: Option<OsString>,
        windows: bool,
    ) -> Result<Self> {
        if !windows {
            return Ok(Self {
                root: root.to_path_buf(),
                inherited_target,
                lease: None,
            });
        }

        let direct_cargo = cargo_subcommand(command).is_some();
        let explicit_targets = if direct_cargo {
            cargo_option_values_before_separator(command, "--target-dir")
        } else {
            Vec::new()
        };
        if explicit_targets.len() > 1 {
            bail!(
                "Windows 自托管验证拒绝多个 --target-dir；无法证明 Cargo 输出不会覆盖当前 rayman"
            );
        }

        if let Some(explicit) = explicit_targets.first() {
            if explicit.is_empty() {
                bail!("Windows 自托管验证拒绝空 --target-dir");
            }
            let explicit = resolve_target_dir(root, Some(OsStr::new(explicit)));
            if current_is_direct_cargo_artifact(&explicit, current_exe)? {
                bail!(
                    "Windows 自托管验证拒绝指向当前 rayman 的显式 --target-dir；环境隔离无法覆盖命令行参数"
                );
            }
            return Ok(Self {
                root: root.to_path_buf(),
                inherited_target,
                lease: None,
            });
        }

        let inherited_is_effective = inherited_target
            .as_deref()
            .is_some_and(|value| !value.is_empty());
        let default_target = root.join("target");
        let inherited_target_path = resolve_target_dir(root, inherited_target.as_deref());
        let current_in_inherited =
            current_is_direct_cargo_artifact(&inherited_target_path, current_exe)?;
        let current_in_default = current_is_direct_cargo_artifact(&default_target, current_exe)?;
        let potential_workspace_self_host = current_in_inherited
            || current_in_default
            || current_may_be_workspace_cargo_artifact(root, command, current_exe)?;
        let effective_target = if inherited_is_effective {
            inherited_target_path
        } else if current_in_default {
            default_target
        } else if !potential_workspace_self_host {
            inherited_target_path
        } else {
            cargo_reported_target_dir(root, command, inherited_target.as_deref())?
        };
        let current_in_effective =
            current_is_direct_cargo_artifact(&effective_target, current_exe)?;
        if direct_cargo && cargo_has_option(command, "--config") && potential_workspace_self_host {
            bail!(
                "Windows 自托管验证拒绝带 --config 的 Cargo 命令；其 build.target-dir 可能绕过环境隔离"
            );
        }

        let lease = if current_in_effective {
            // Keep the internal label deliberately short. MSVC link.exe still
            // encounters non-long-path-compatible output stages, and Cargo
            // appends deep build-script/package/hash components below target.
            // The manifest and exclusive id carry ownership; a verbose label
            // only consumes the compatibility budget without adding authority.
            Some(create_managed_cargo_target_lease(root, "c")?)
        } else {
            None
        };
        Ok(Self {
            root: root.to_path_buf(),
            inherited_target,
            lease,
        })
    }

    /// Apply only the one environment key owned by this session. Removing then
    /// restoring the captured value makes every child independent of builder
    /// reuse and preserves an empty inherited value exactly.
    pub fn apply(&self, process: &mut Command) -> Result<()> {
        process.env_remove(CARGO_TARGET_DIR);
        if let Some(managed) = &self.lease {
            managed.verify_current()?;
            let lease = managed.lease();
            process.env(CARGO_TARGET_DIR, &lease.target_dir);
        } else if let Some(value) = &self.inherited_target {
            process.env(CARGO_TARGET_DIR, value);
        }
        Ok(())
    }

    pub fn finish(&mut self) -> Result<()> {
        let Some(lease) = self.lease.take() else {
            return Ok(());
        };
        release_managed_cargo_target_lease(&self.root, &lease)?;
        Ok(())
    }

    /// Release on both success and failure. A cleanup failure is part of the
    /// validation failure and therefore prevents the caller from minting a
    /// receipt; when execution also failed, both causes remain visible.
    pub fn finish_with<T>(&mut self, result: Result<T>) -> Result<T> {
        let cleanup = self.finish();
        match (result, cleanup) {
            (Ok(value), Ok(())) => Ok(value),
            (Err(error), Ok(())) => Err(error),
            (Ok(_), Err(cleanup)) => Err(cleanup).context(
                "Windows 自托管验证已执行，但 Cargo target lease 释放失败；不会写入 receipt",
            ),
            (Err(error), Err(cleanup)) => Err(error).context(format!(
                "验证失败后 Cargo target lease 释放也失败: {cleanup:#}"
            )),
        }
    }

    #[cfg(test)]
    fn managed_target_dir(&self) -> Option<&Path> {
        self.lease
            .as_ref()
            .map(|lease| Path::new(&lease.lease().target_dir))
    }
}

impl Drop for ValidationExecutionSession {
    fn drop(&mut self) {
        if let Some(lease) = self.lease.take() {
            let _ = release_managed_cargo_target_lease(&self.root, &lease);
        }
    }
}

fn cargo_has_option(command: &ParsedValidationCommand, name: &str) -> bool {
    command
        .args
        .iter()
        .take_while(|argument| argument.as_str() != "--")
        .any(|argument| argument == name || argument.starts_with(&format!("{name}=")))
}

fn cargo_option_values_before_separator<'a>(
    command: &'a ParsedValidationCommand,
    name: &str,
) -> Vec<&'a str> {
    let mut values = Vec::new();
    let mut index = 0;
    while let Some(argument) = command.args.get(index) {
        if argument == "--" {
            break;
        }
        if argument == name {
            if let Some(value) = command.args.get(index + 1) {
                values.push(value.as_str());
            }
            index += 2;
            continue;
        }
        if let Some(value) = argument.strip_prefix(&format!("{name}=")) {
            values.push(value);
        }
        index += 1;
    }
    values
}

fn resolve_target_dir(root: &Path, value: Option<&OsStr>) -> PathBuf {
    let Some(value) = value.filter(|value| !value.is_empty()) else {
        return root.join("target");
    };
    let path = PathBuf::from(value);
    if path.is_absolute() {
        path
    } else {
        root.join(path)
    }
}

fn run_cargo_metadata_process(root: &Path, process: &mut Command) -> Result<std::process::Output> {
    crate::temp::run_with_validation_process_lease(root, "m", |environment| {
        if let Some(environment) = environment {
            process.envs(environment);
        }
        process
            .output()
            .context("无法执行 cargo metadata 以确定 Windows 自托管验证 target")
    })
}

fn cargo_reported_target_dir(
    root: &Path,
    command: &ParsedValidationCommand,
    inherited_target: Option<&OsStr>,
) -> Result<PathBuf> {
    let manifest_paths = if cargo_subcommand(command).is_some() {
        cargo_option_values_before_separator(command, "--manifest-path")
    } else {
        Vec::new()
    };
    if manifest_paths.len() > 1 {
        bail!("Windows 自托管验证拒绝多个 --manifest-path；无法唯一解析 Cargo workspace");
    }
    if manifest_paths.first().is_some_and(|value| value.is_empty()) {
        bail!("Windows 自托管验证拒绝空 --manifest-path");
    }

    let mut process = Command::new("cargo");
    process
        .args([
            "metadata",
            "--format-version",
            "1",
            "--no-deps",
            "--locked",
            "--offline",
        ])
        .current_dir(root)
        .env_remove(CARGO_TARGET_DIR);
    if let Some(value) = inherited_target {
        process.env(CARGO_TARGET_DIR, value);
    }
    if let Some(manifest) = manifest_paths.first() {
        process.args(["--manifest-path", manifest]);
    }
    let output = run_cargo_metadata_process(root, &mut process)?;
    if !output.status.success() {
        bail!(
            "cargo metadata 无法证明 Windows 自托管验证的有效 target（exit={}）；拒绝回退到猜测路径",
            output.status.code().unwrap_or(-1)
        );
    }
    let document: serde_json::Value = serde_json::from_slice(&output.stdout)
        .context("无法解析 cargo metadata 的 Windows 自托管 target 结果")?;
    let target = document
        .get("target_directory")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow::anyhow!("cargo metadata 缺少有效 target_directory"))?;
    let target = PathBuf::from(target);
    if !target.is_absolute() {
        bail!(
            "cargo metadata 返回非绝对 target_directory: {target}",
            target = target.display()
        );
    }
    Ok(target)
}

fn current_may_be_workspace_cargo_artifact(
    root: &Path,
    command: &ParsedValidationCommand,
    current_exe: &Path,
) -> Result<bool> {
    let current = current_exe
        .canonicalize()
        .with_context(|| format!("无法规范化当前 rayman: {}", current_exe.display()))?;
    let workspace = root
        .canonicalize()
        .with_context(|| format!("无法规范化 Windows 自托管工作区: {}", root.display()))?;
    if current.starts_with(&workspace) {
        let mut ancestor = current.parent();
        while let Some(path) = ancestor.filter(|path| path.starts_with(&workspace)) {
            if path.join(".rustc_info.json").is_file() {
                return Ok(true);
            }
            if path == workspace {
                break;
            }
            ancestor = path.parent();
        }
    }

    let cargo_workspace_possible = root.join("Cargo.toml").is_file()
        || (cargo_subcommand(command).is_some()
            && !cargo_option_values_before_separator(command, "--manifest-path").is_empty());
    Ok(cargo_workspace_possible && current_file_has_multiple_links(&current)?)
}

#[cfg(windows)]
fn current_file_has_multiple_links(path: &Path) -> Result<bool> {
    use std::os::windows::io::AsRawHandle as _;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };

    let file = crate::file_io::open_file_no_follow(path)
        .with_context(|| format!("无法打开当前 rayman 身份: {}", path.display()))?;
    let mut information = BY_HANDLE_FILE_INFORMATION::default();
    let read = unsafe { GetFileInformationByHandle(file.as_raw_handle() as _, &mut information) };
    if read == 0 {
        return Err(std::io::Error::last_os_error())
            .with_context(|| format!("无法读取当前 rayman 硬链接计数: {}", path.display()));
    }
    Ok(information.nNumberOfLinks > 1)
}

#[cfg(unix)]
fn current_file_has_multiple_links(path: &Path) -> Result<bool> {
    use std::os::unix::fs::MetadataExt as _;
    Ok(fs::metadata(path)?.nlink() > 1)
}

#[cfg(not(any(windows, unix)))]
fn current_file_has_multiple_links(_: &Path) -> Result<bool> {
    Ok(false)
}

/// Cargo's directly runnable binary is `<target>/<profile>/rayman[.exe]` or
/// `<target>/<triple>/<profile>/rayman[.exe]`. Copies below an extra helper
/// directory are intentionally not treated as collision targets because Cargo
/// will relink the direct artifact, not that copy.
fn current_is_direct_cargo_artifact(target: &Path, current_exe: &Path) -> Result<bool> {
    let target = match target.canonicalize() {
        Ok(target) => target,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("无法规范化候选 Cargo target: {}", target.display()));
        }
    };
    let current = current_exe
        .canonicalize()
        .with_context(|| format!("无法规范化当前 rayman: {}", current_exe.display()))?;
    if let Ok(relative) = current.strip_prefix(&target) {
        let components = relative
            .components()
            .filter_map(|component| match component {
                Component::Normal(value) => Some(value),
                _ => None,
            })
            .collect::<Vec<_>>();
        if matches!(components.len(), 2 | 3)
            && components
                .last()
                .and_then(|value| value.to_str())
                .is_some_and(|file_name| {
                    matches!(
                        file_name.to_ascii_lowercase().as_str(),
                        "rayman" | "rayman.exe"
                    )
                })
        {
            return Ok(true);
        }
    }

    #[cfg(windows)]
    {
        windows_target_contains_running_file(&target, &current)
    }
    #[cfg(not(windows))]
    {
        Ok(false)
    }
}

#[cfg(windows)]
fn windows_target_contains_running_file(target: &Path, current: &Path) -> Result<bool> {
    let current_identity = windows_file_identity(current)?.ok_or_else(|| {
        anyhow::anyhow!("当前 rayman 不是可读取的普通文件: {}", current.display())
    })?;
    for first in fs::read_dir(target)
        .with_context(|| format!("无法枚举候选 Cargo target: {}", target.display()))?
    {
        let first = first?;
        let first_path = first.path();
        let first_metadata = fs::symlink_metadata(&first_path)?;
        if crate::file_io::is_link_or_reparse(&first_metadata) {
            bail!(
                "候选 Cargo target 含链接/reparse 条目: {}",
                first_path.display()
            );
        }
        if !first_metadata.file_type().is_dir() {
            continue;
        }
        if windows_candidate_matches(&first_path.join("rayman.exe"), &current_identity)? {
            return Ok(true);
        }
        for second in fs::read_dir(&first_path).with_context(|| {
            format!("无法枚举候选 Cargo target 子目录: {}", first_path.display())
        })? {
            let second = second?;
            let second_path = second.path();
            let second_metadata = fs::symlink_metadata(&second_path)?;
            if crate::file_io::is_link_or_reparse(&second_metadata) {
                bail!(
                    "候选 Cargo target 含链接/reparse 条目: {}",
                    second_path.display()
                );
            }
            if second_metadata.file_type().is_dir()
                && windows_candidate_matches(&second_path.join("rayman.exe"), &current_identity)?
            {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

#[cfg(windows)]
fn windows_candidate_matches(
    candidate: &Path,
    current: &crate::file_io::FileIdentity,
) -> Result<bool> {
    Ok(windows_file_identity(candidate)?
        .as_ref()
        .is_some_and(|candidate| same_windows_file_object(candidate, current)))
}

#[cfg(windows)]
fn windows_file_identity(path: &Path) -> Result<Option<crate::file_io::FileIdentity>> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("无法读取文件身份: {}", path.display()));
        }
    };
    if crate::file_io::is_link_or_reparse(&metadata) || !metadata.file_type().is_file() {
        return Ok(None);
    }
    let file = crate::file_io::open_file_no_follow(path)
        .with_context(|| format!("无法打开文件身份: {}", path.display()))?;
    let handle_metadata = file
        .metadata()
        .with_context(|| format!("无法读取文件句柄元数据: {}", path.display()))?;
    let identity =
        crate::file_io::file_identity_from_handle(&file, &handle_metadata, path, "Cargo artifact")?;
    if !crate::file_io::has_strong_file_identity(&identity) {
        bail!("Cargo artifact 缺少强文件身份: {}", path.display());
    }
    Ok(Some(identity))
}

#[cfg(windows)]
fn same_windows_file_object(
    left: &crate::file_io::FileIdentity,
    right: &crate::file_io::FileIdentity,
) -> bool {
    left.volume_serial_number == right.volume_serial_number
        && left.file_index == right.file_index
        && left.volume_serial_number_64 == right.volume_serial_number_64
        && left.file_id_128 == right.file_id_128
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn validation_process_temp_probe_child() {
        if std::env::var("RAYMAN_CARGO_METADATA_TEMP_PROBE").as_deref() == Ok("1") {
            println!(
                "RAYMAN_CARGO_METADATA_TEMP={}",
                std::env::var("TEMP").expect("managed TEMP")
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn internal_cargo_metadata_process_uses_and_releases_host_temp() {
        let workspace = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
        let root = workspace.path();
        let test_name = format!("{}::validation_process_temp_probe_child", module_path!());
        let test_name = test_name
            .strip_prefix("rayman::")
            .unwrap_or(&test_name)
            .to_string();
        let mut process = Command::new(std::env::current_exe().unwrap());
        process
            .arg(&test_name)
            .args(["--exact", "--nocapture"])
            .env("RAYMAN_CARGO_METADATA_TEMP_PROBE", "1");

        let output = run_cargo_metadata_process(root, &mut process).unwrap();
        assert!(
            output.status.success(),
            "child stderr={}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8(output.stdout).unwrap();
        let process_temp = stdout
            .lines()
            .find_map(|line| line.strip_prefix("RAYMAN_CARGO_METADATA_TEMP="))
            .map(PathBuf::from)
            .unwrap_or_else(|| panic!("missing managed temp marker in {stdout:?}"));
        let host_root = crate::temp::validation_process_state_root(root).unwrap();
        let host_root = PathBuf::from(crate::pathfmt::display_path(&host_root));
        assert!(
            process_temp.starts_with(host_root.join("v")),
            "metadata temp escaped host root: {}",
            process_temp.display()
        );
        assert!(
            !process_temp.exists(),
            "metadata temp lease was not released: {}",
            process_temp.display()
        );
    }

    fn command(text: &str) -> ParsedValidationCommand {
        super::super::parse_validation_command(text).unwrap()
    }

    fn fake_rayman(root: &Path, target: &Path) -> PathBuf {
        let executable = target.join("debug/rayman.exe");
        std::fs::create_dir_all(executable.parent().unwrap()).unwrap();
        std::fs::write(&executable, b"fixture").unwrap();
        std::fs::create_dir_all(root.join(".RaymanCodingSkill")).unwrap();
        executable
    }

    #[test]
    fn self_hosted_aliases_create_unique_managed_sessions() {
        let workspace = tempfile::tempdir().unwrap();
        let root = workspace.path();
        let target = root.join("target");
        let executable = fake_rayman(root, &target);
        let inherited = Some(OsString::from("./nested/../target"));
        std::fs::create_dir_all(root.join("nested")).unwrap();

        let mut first = ValidationExecutionSession::prepare_for(
            root,
            &command("cargo test --workspace --all-targets"),
            &executable,
            inherited.clone(),
            true,
        )
        .unwrap();
        let mut second = ValidationExecutionSession::prepare_for(
            root,
            &command("cargo test --workspace --all-targets"),
            &executable,
            inherited,
            true,
        )
        .unwrap();
        let first_target = first.managed_target_dir().unwrap().to_path_buf();
        let second_target = second.managed_target_dir().unwrap().to_path_buf();
        assert_ne!(first_target, second_target);
        assert!(first_target.starts_with(root.join(".RaymanCodingSkill/tmp")));
        assert!(!first_target.starts_with(&target));

        first.finish().unwrap();
        second.finish().unwrap();
        assert!(!first_target.exists());
        assert!(!second_target.exists());
    }

    #[test]
    fn non_self_hosted_and_non_windows_are_exact_noops() {
        let workspace = tempfile::tempdir().unwrap();
        let root = workspace.path();
        let running_target = root.join("target");
        let executable = fake_rayman(root, &running_target);
        let safe_target = root.join("safe-target");
        std::fs::create_dir(&safe_target).unwrap();

        let session = ValidationExecutionSession::prepare_for(
            root,
            &command("cargo test --workspace --all-targets"),
            &executable,
            Some(safe_target.clone().into_os_string()),
            true,
        )
        .unwrap();
        assert!(session.managed_target_dir().is_none());
        let mut child = Command::new("cargo");
        session.apply(&mut child).unwrap();
        let captured = child
            .get_envs()
            .find(|(name, _)| *name == OsStr::new(CARGO_TARGET_DIR))
            .and_then(|(_, value)| value)
            .unwrap();
        assert_eq!(captured, safe_target.as_os_str());

        let non_windows = ValidationExecutionSession::prepare_for(
            root,
            &command("cargo test --workspace --all-targets"),
            &executable,
            Some(OsString::from("target")),
            false,
        )
        .unwrap();
        assert!(non_windows.managed_target_dir().is_none());
    }

    #[test]
    fn direct_cargo_overrides_cannot_bypass_a_self_hosted_session() {
        let workspace = tempfile::tempdir().unwrap();
        let root = workspace.path();
        let target = root.join("target");
        let executable = fake_rayman(root, &target);

        for text in [
            "cargo test --workspace --target-dir target",
            "cargo test --workspace --target-dir=target",
            "cargo test --workspace --config build.target-dir=target",
        ] {
            assert!(
                ValidationExecutionSession::prepare_for(
                    root,
                    &command(text),
                    &executable,
                    None,
                    true,
                )
                .is_err(),
                "must reject {text}"
            );
        }

        let mut libtest_argument = ValidationExecutionSession::prepare_for(
            root,
            &command("cargo test --workspace -- --target-dir target --config fake"),
            &executable,
            None,
            true,
        )
        .unwrap();
        assert!(libtest_argument.managed_target_dir().is_some());
        libtest_argument.finish().unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn cargo_config_target_and_differently_named_hardlink_alias_share_strong_identity_detection() {
        let workspace = tempfile::tempdir().unwrap();
        let root = workspace.path();
        std::fs::create_dir_all(root.join(".cargo")).unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"cargo-config-target-fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .unwrap();
        std::fs::create_dir(root.join("src")).unwrap();
        std::fs::write(root.join("src/lib.rs"), "pub fn fixture() {}\n").unwrap();
        std::fs::write(
            root.join("Cargo.lock"),
            "version = 4\n\n[[package]]\nname = \"cargo-config-target-fixture\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        std::fs::write(
            root.join(".cargo/config.toml"),
            "[build]\ntarget-dir = \"configured-target\"\n",
        )
        .unwrap();
        let configured_target = root.join("configured-target");
        let executable = fake_rayman(root, &configured_target);
        std::fs::write(configured_target.join(".rustc_info.json"), "{}\n").unwrap();
        let parsed = command("cargo test --workspace --all-targets");

        let mut direct =
            ValidationExecutionSession::prepare_for(root, &parsed, &executable, None, true)
                .unwrap();
        assert!(direct.managed_target_dir().is_some());
        direct.finish().unwrap();

        let alias = root.join("hardlink-alias/rayman-dev.exe");
        std::fs::create_dir_all(alias.parent().unwrap()).unwrap();
        std::fs::hard_link(&executable, &alias).unwrap();
        let mut hardlink =
            ValidationExecutionSession::prepare_for(root, &parsed, &alias, None, true).unwrap();
        assert!(hardlink.managed_target_dir().is_some());
        hardlink.finish().unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn manifest_path_uses_the_nested_workspace_reported_target() {
        let workspace = tempfile::tempdir().unwrap();
        let root = workspace.path();
        let nested = root.join("nested-workspace");
        std::fs::create_dir_all(nested.join("src")).unwrap();
        std::fs::write(
            nested.join("Cargo.toml"),
            "[package]\nname = \"nested-target-fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .unwrap();
        std::fs::write(nested.join("src/lib.rs"), "pub fn fixture() {}\n").unwrap();
        std::fs::write(
            nested.join("Cargo.lock"),
            "version = 4\n\n[[package]]\nname = \"nested-target-fixture\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        let target = nested.join("target");
        let executable = fake_rayman(root, &target);
        std::fs::write(target.join(".rustc_info.json"), "{}\n").unwrap();
        let mut session = ValidationExecutionSession::prepare_for(
            root,
            &command(
                "cargo test --manifest-path nested-workspace/Cargo.toml --workspace --all-targets",
            ),
            &executable,
            None,
            true,
        )
        .unwrap();

        assert!(session.managed_target_dir().is_some());
        session.finish().unwrap();
    }

    #[test]
    fn execution_failure_releases_but_cleanup_failure_stays_fatal() {
        let workspace = tempfile::tempdir().unwrap();
        let root = workspace.path();
        let target = root.join("target");
        let executable = fake_rayman(root, &target);
        let parsed = command("cargo test --workspace --all-targets");

        let mut failed =
            ValidationExecutionSession::prepare_for(root, &parsed, &executable, None, true)
                .unwrap();
        let failed_target = failed.managed_target_dir().unwrap().to_path_buf();
        let error = failed
            .finish_with::<()>(Err(anyhow::anyhow!("injected execution failure")))
            .unwrap_err();
        assert!(error.to_string().contains("injected execution failure"));
        assert!(!failed_target.exists());

        let mut tampered =
            ValidationExecutionSession::prepare_for(root, &parsed, &executable, None, true)
                .unwrap();
        let lease = tampered.lease.as_ref().unwrap().lease().clone();
        let mut altered = lease.clone();
        altered.target_dir = root.join("outside").display().to_string();
        crate::file_io::write_json(&Path::new(&lease.root).join("lease.json"), &altered).unwrap();
        let error = tampered.finish_with(Ok(())).unwrap_err();
        assert!(error.to_string().contains("不会写入 receipt"), "{error:#}");
        assert!(Path::new(&lease.root).exists());
    }

    #[test]
    fn missing_cargo_target_lease_blocks_success_receipt() {
        let workspace = tempfile::tempdir().unwrap();
        let root = workspace.path();
        let target = root.join("target");
        let executable = fake_rayman(root, &target);
        let parsed = command("cargo test --workspace --all-targets");
        let mut session =
            ValidationExecutionSession::prepare_for(root, &parsed, &executable, None, true)
                .unwrap();
        let lease_root = PathBuf::from(&session.lease.as_ref().unwrap().lease().root);
        fs::remove_dir_all(&lease_root).unwrap();

        let error = session.finish_with(Ok(())).unwrap_err();
        assert!(error.to_string().contains("不会写入 receipt"), "{error:#}");
        assert!(
            format!("{error:#}").contains("已验证释放前消失")
                || format!("{error:#}").contains("不存在")
                || format!("{error:#}").contains("系统找不到指定的文件"),
            "{error:#}"
        );
    }

    #[cfg(windows)]
    #[test]
    fn pre_release_target_leaf_replacement_blocks_success_receipt() {
        let workspace = tempfile::tempdir().unwrap();
        let root = workspace.path();
        let target = root.join("target");
        let executable = fake_rayman(root, &target);
        let parsed = command("cargo test --workspace --all-targets");
        let mut session =
            ValidationExecutionSession::prepare_for(root, &parsed, &executable, None, true)
                .unwrap();
        let lease = session.lease.as_ref().unwrap().lease().clone();
        let original = PathBuf::from(&lease.root);
        let displaced = original.with_file_name(format!("{}-displaced", lease.id));
        fs::rename(&original, &displaced).unwrap();
        fs::create_dir(&original).unwrap();
        fs::create_dir(original.join("t")).unwrap();
        crate::file_io::write_json(&original.join("lease.json"), &lease).unwrap();

        let error = session.finish_with(Ok(())).unwrap_err();
        assert!(error.to_string().contains("不会写入 receipt"), "{error:#}");
        assert!(format!("{error:#}").contains("创建目录"), "{error:#}");
        assert!(
            displaced.is_dir() && original.is_dir(),
            "identity mismatch must preserve both the original target and replacement"
        );
    }

    #[cfg(windows)]
    #[test]
    fn pre_spawn_target_leaf_replacement_blocks_child_setup() {
        let workspace = tempfile::tempdir().unwrap();
        let root = workspace.path();
        let target = root.join("target");
        let executable = fake_rayman(root, &target);
        let parsed = command("cargo test --workspace --all-targets");
        let mut session =
            ValidationExecutionSession::prepare_for(root, &parsed, &executable, None, true)
                .unwrap();
        let lease = session.lease.as_ref().unwrap().lease().clone();
        let original = PathBuf::from(&lease.root);
        let displaced = original.with_file_name(format!("{}-displaced", lease.id));
        fs::rename(&original, &displaced).unwrap();
        fs::create_dir(&original).unwrap();
        fs::create_dir(original.join("t")).unwrap();
        crate::file_io::write_json(&original.join("lease.json"), &lease).unwrap();

        let mut child = Command::new("must-not-spawn-after-target-replacement");
        let error = session.apply(&mut child).unwrap_err();
        assert!(format!("{error:#}").contains("创建目录"), "{error:#}");
        assert!(
            session.finish_with(Ok(())).is_err(),
            "replacement must also prevent later cleanup authority"
        );
        assert!(displaced.is_dir() && original.is_dir());
    }
}
