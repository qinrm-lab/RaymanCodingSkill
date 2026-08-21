use std::collections::BTreeMap;
#[cfg(windows)]
use std::ffi::OsStr;
use std::ffi::OsString;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Output;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[cfg(not(windows))]
use super::probe_directory;
use super::{
    LEASE_SEQUENCE, is_valid_lease_id, lease_id_matches_label, path_text, safe_lease_id_label,
};
#[cfg(windows)]
use super::{
    WindowsLeaseManifestSeal, create_windows_lease_manifest, verify_windows_lease_manifest,
    verify_windows_lease_manifest_from_directory_handle, windows_creation_collision,
};
use crate::file_io::is_link_or_reparse;
use crate::pathfmt::display_path;
use crate::state_paths;

const VALIDATION_TEMP_ROOT_ENV: &str = "RAYMAN_VALIDATION_TEMP_ROOT";
const CARGO_HOME_ENV: &str = "CARGO_HOME";
const LOCALAPPDATA_ENV: &str = "LOCALAPPDATA";
const CODEX_SANDBOX_DIR: &str = "codex-sandbox";
const RAYMAN_VALIDATION_DIR: &str = "rayman-validation";
// Physical lease names are deliberately compact: Rust/MSVC appends several
// opaque build-script and linker components below a child process's temp path.
// The manifest, exclusive leaf creation, and held directory identity remain
// the ownership contract; these names are internal storage aliases, not an
// operator-facing API.
const LEASES_RELATIVE: &str = "v";
const LEASE_MANIFEST: &str = "lease.json";
const TEMP_DIR: &str = "t";
const NESTED_VALIDATION_DIR: &str = "n";
const TEMP_KEYS: [&str; 3] = ["TEMP", "TMP", "TMPDIR"];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct ValidationProcessLease {
    schema: String,
    id: String,
    label: String,
    created_at: String,
    root: String,
    temp_dir: String,
    nested_validation_root: String,
    environment: BTreeMap<String, String>,
}

#[cfg(windows)]
struct WindowsValidationProcessLeaseGuards {
    root: state_paths::WindowsDirectoryObjectGuard,
    temp_dir: state_paths::WindowsDirectoryChildIdentity,
    nested_validation_root: state_paths::WindowsDirectoryChildIdentity,
    manifest: WindowsLeaseManifestSeal,
}

#[cfg(windows)]
struct WindowsExternalValidationProcessLeaseGuards {
    binding: WindowsExternalLeaseIdentity,
    temp_dir: state_paths::WindowsDirectoryChildIdentity,
    nested_validation_root: state_paths::WindowsDirectoryChildIdentity,
    manifest: WindowsLeaseManifestSeal,
}

#[cfg(windows)]
enum WindowsValidationProcessLeaseBinding {
    // Workspace-local direct leases remain reachable only in Windows unit-test
    // fixtures. Production validation leases always use the external
    // identity-only binding below the held host root.
    #[allow(dead_code)]
    Direct(WindowsValidationProcessLeaseGuards),
    External(WindowsExternalValidationProcessLeaseGuards),
}

#[cfg(windows)]
struct WindowsValidationProcessLeaseCreation {
    lease: ValidationProcessLease,
    temp_dir: state_paths::WindowsDirectoryChildIdentity,
    nested_validation_root: state_paths::WindowsDirectoryChildIdentity,
    manifest: WindowsLeaseManifestSeal,
}

#[cfg(windows)]
#[derive(Default)]
struct WindowsValidationProcessLeasePartial {
    temp_dir: Option<state_paths::WindowsDirectoryChildIdentity>,
    nested_validation_root: Option<state_paths::WindowsDirectoryChildIdentity>,
    // Once `lease.json` has been captured, creation-failure cleanup must use
    // the same complete contract as normal release.  Holding the seal here
    // prevents a failure after manifest publication from turning cleanup into
    // authority over a cloned, same-content manifest.
    lease: Option<ValidationProcessLease>,
    manifest: Option<WindowsLeaseManifestSeal>,
}

#[cfg(windows)]
impl WindowsValidationProcessLeasePartial {
    fn verify_created(&self, root: &state_paths::WindowsDirectoryObjectGuard) -> Result<()> {
        state_paths::verify_windows_directory_object_at_path(
            root,
            root.path(),
            "validation process lease 创建目录",
        )?;
        if let Some(temp_dir) = &self.temp_dir {
            root.verify_direct_child(
                temp_dir,
                OsStr::new(TEMP_DIR),
                "validation process lease 临时目录",
            )?;
        }
        if let Some(nested_validation_root) = &self.nested_validation_root {
            root.verify_direct_child(
                nested_validation_root,
                OsStr::new(NESTED_VALIDATION_DIR),
                "validation process lease 嵌套验证根",
            )?;
        }
        match (&self.lease, &self.manifest) {
            (None, None) => state_paths::verify_windows_directory_object_at_path(
                root,
                root.path(),
                "validation process lease 创建目录",
            ),
            (Some(lease), Some(manifest)) => {
                let temp_dir = self.temp_dir.as_ref().ok_or_else(|| {
                    anyhow::anyhow!("validation process lease manifest 已发布但临时目录未创建")
                })?;
                let nested_validation_root =
                    self.nested_validation_root.as_ref().ok_or_else(|| {
                        anyhow::anyhow!(
                            "validation process lease manifest 已发布但嵌套验证根未创建"
                        )
                    })?;
                WindowsValidationProcessLeaseGuards::verify_current_fields(
                    root,
                    temp_dir,
                    nested_validation_root,
                    manifest,
                    lease,
                )
            }
            _ => bail!("validation process lease 创建部分状态的 manifest 封存不完整"),
        }
    }
}

#[cfg(windows)]
impl WindowsValidationProcessLeaseGuards {
    fn verify_namespace_fields(
        root: &state_paths::WindowsDirectoryObjectGuard,
        temp_dir: &state_paths::WindowsDirectoryChildIdentity,
        nested_validation_root: &state_paths::WindowsDirectoryChildIdentity,
    ) -> Result<()> {
        state_paths::verify_windows_directory_object_at_path(
            root,
            root.path(),
            "validation process lease 创建目录",
        )?;
        root.verify_direct_child(
            temp_dir,
            OsStr::new(TEMP_DIR),
            "validation process lease 临时目录",
        )?;
        root.verify_direct_child(
            nested_validation_root,
            OsStr::new(NESTED_VALIDATION_DIR),
            "validation process lease 嵌套验证根",
        )?;
        state_paths::verify_windows_directory_object_at_path(
            root,
            root.path(),
            "validation process lease 创建目录",
        )
    }

    fn verify_current_fields(
        root: &state_paths::WindowsDirectoryObjectGuard,
        temp_dir: &state_paths::WindowsDirectoryChildIdentity,
        nested_validation_root: &state_paths::WindowsDirectoryChildIdentity,
        manifest: &WindowsLeaseManifestSeal,
        lease: &ValidationProcessLease,
    ) -> Result<()> {
        Self::verify_namespace_fields(root, temp_dir, nested_validation_root)?;
        root.probe_direct_child(
            temp_dir,
            OsStr::new(TEMP_DIR),
            "validation process lease 临时目录",
        )?;
        root.probe_direct_child(
            nested_validation_root,
            OsStr::new(NESTED_VALIDATION_DIR),
            "validation process lease 嵌套验证根",
        )?;
        let bytes = verify_windows_lease_manifest(
            root,
            manifest,
            LEASE_MANIFEST,
            "validation process lease manifest",
        )?;
        let actual = parse_validation_process_manifest(&lease.id, root.path(), &bytes)?;
        if actual != *lease {
            bail!("validation process lease 复验身份发生变化: {}", lease.id);
        }
        Self::verify_namespace_fields(root, temp_dir, nested_validation_root)
    }

    #[allow(dead_code)]
    fn verify_current(&self, lease: &ValidationProcessLease) -> Result<()> {
        Self::verify_current_fields(
            &self.root,
            &self.temp_dir,
            &self.nested_validation_root,
            &self.manifest,
            lease,
        )
    }
}

#[cfg(windows)]
impl WindowsExternalValidationProcessLeaseGuards {
    fn verify_current_at_host_root(
        &self,
        host_root: &ValidationProcessStateRoot,
        lease: &ValidationProcessLease,
    ) -> Result<()> {
        host_root.with_external_lease_root(
            &self.binding,
            LEASES_RELATIVE,
            &lease.id,
            "validation process lease 创建目录",
            |root| {
                WindowsValidationProcessLeaseGuards::verify_current_fields(
                    root,
                    &self.temp_dir,
                    &self.nested_validation_root,
                    &self.manifest,
                    lease,
                )
            },
        )
    }
}

struct ManagedValidationProcessLease {
    lease: ValidationProcessLease,
    #[cfg(windows)]
    directory_guards: WindowsValidationProcessLeaseBinding,
}

impl ManagedValidationProcessLease {
    #[allow(dead_code)]
    fn verify_current(&self) -> Result<()> {
        #[cfg(windows)]
        match &self.directory_guards {
            WindowsValidationProcessLeaseBinding::Direct(guards) => {
                guards.verify_current(&self.lease)?;
            }
            WindowsValidationProcessLeaseBinding::External(_) => {
                bail!("validation process external lease 必须通过持有 validation host-temp 根复验")
            }
        }
        Ok(())
    }

    fn verify_current_at_host_root(&self, host_root: &ValidationProcessStateRoot) -> Result<()> {
        #[cfg(not(windows))]
        let _ = host_root;
        #[cfg(windows)]
        match &self.directory_guards {
            WindowsValidationProcessLeaseBinding::Direct(guards) => {
                host_root.verify_current()?;
                guards.verify_current(&self.lease)?;
                host_root.verify_current()?;
            }
            WindowsValidationProcessLeaseBinding::External(guards) => {
                guards.verify_current_at_host_root(host_root, &self.lease)?;
            }
        }
        Ok(())
    }
}

/// A validation host-temp root held from preflight through its child and
/// cleanup. The lease leaf protects a single operation; this guard also makes
/// a replacement of the configured external root fail before a child can be
/// given a path beneath the replacement.
pub(crate) struct ValidationProcessStateRoot {
    path: PathBuf,
    #[cfg(windows)]
    directory_guard: state_paths::WindowsDirectoryObjectGuard,
}

/// Identity-only binding for one `v/<id>` or `p/<id>` lease below a held host
/// root. Neither directory handle survives creation, so the host namespace is
/// still movable; every subsequent operation reopens both names relative to
/// the held host root and compares these strong identities.
#[cfg(windows)]
#[derive(Clone)]
pub(crate) struct WindowsExternalLeaseIdentity {
    namespace: state_paths::WindowsDirectoryChildIdentity,
    root: state_paths::WindowsDirectoryChildIdentity,
}

#[cfg(windows)]
impl WindowsExternalLeaseIdentity {
    pub(crate) fn root_identity(&self) -> &state_paths::WindowsDirectoryChildIdentity {
        &self.root
    }
}

impl ValidationProcessStateRoot {
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn verify_current(&self) -> Result<()> {
        #[cfg(windows)]
        state_paths::verify_windows_directory_object_at_path(
            &self.directory_guard,
            &self.path,
            "validation host-temp 根",
        )?;
        Ok(())
    }

    #[cfg(windows)]
    fn probe_current(&self) -> Result<()> {
        self.verify_current()?;
        self.directory_guard.probe_self("validation host-temp 根")?;
        self.verify_current()
    }

    /// Create a `v/<id>` or `p/<id>` lease below the held host root without
    /// retaining either descendant handle after creation.
    #[cfg(windows)]
    pub(crate) fn create_external_lease_identity(
        &self,
        namespace: &str,
        id: &str,
        label: &str,
    ) -> Result<WindowsExternalLeaseIdentity> {
        self.verify_current()?;
        let namespace_guard = self.directory_guard.open_or_create_direct_child(
            OsStr::new(namespace),
            "validation host-temp lease 命名空间",
        )?;
        let namespace_identity = namespace_guard.child_identity();
        self.verify_current()?;
        let leaf = namespace_guard.create_child_exclusive(OsStr::new(id), label);
        drop(namespace_guard);
        self.verify_current()?;
        let root = leaf?;
        self.verify_current()?;
        Ok(WindowsExternalLeaseIdentity {
            namespace: namespace_identity,
            root,
        })
    }

    /// Reopen an identity-only external lease through the held host root for
    /// one short operation. The namespace and lease handles both close before
    /// the result is returned, preserving rename/replacement observability.
    #[cfg(windows)]
    pub(crate) fn with_external_lease_root<T>(
        &self,
        binding: &WindowsExternalLeaseIdentity,
        namespace: &str,
        id: &str,
        label: &str,
        operation: impl FnOnce(&state_paths::WindowsDirectoryObjectGuard) -> Result<T>,
    ) -> Result<T> {
        self.with_external_lease_root_after_operation(
            binding,
            namespace,
            id,
            label,
            operation,
            || {},
        )
    }

    #[cfg(windows)]
    fn with_external_lease_root_after_operation<T>(
        &self,
        binding: &WindowsExternalLeaseIdentity,
        namespace: &str,
        id: &str,
        label: &str,
        operation: impl FnOnce(&state_paths::WindowsDirectoryObjectGuard) -> Result<T>,
        after_operation: impl FnOnce(),
    ) -> Result<T> {
        let root_guard = self.open_external_lease_root(binding, namespace, id, label)?;
        let result = operation(&root_guard);
        // The operation intentionally receives only a short-lived directory
        // handle, so Windows may move its namespace while it runs. Do not
        // return a lexical lease path after dropping that handle until both
        // `v|p` and the lease leaf have been reopened from the held host root
        // and compared with their creation identities again.
        drop(root_guard);
        after_operation();
        let binding_recheck = self.verify_external_lease_binding(binding, namespace, id, label);
        match (result, binding_recheck) {
            (Ok(value), Ok(())) => Ok(value),
            (Err(error), Ok(())) => Err(error),
            (Ok(_), Err(binding_error)) => Err(binding_error.context(
                "validation host-temp 外部 lease 命名空间或根在句柄操作期间发生身份变化",
            )),
            (Err(error), Err(binding_error)) => Err(error).context(format!(
                "validation host-temp 外部 lease 命名空间或根在句柄操作期间发生身份变化: {binding_error:#}"
            )),
        }
    }

    #[cfg(all(test, windows))]
    fn with_external_lease_root_after_operation_for_test<T>(
        &self,
        binding: &WindowsExternalLeaseIdentity,
        namespace: &str,
        id: &str,
        label: &str,
        operation: impl FnOnce(&state_paths::WindowsDirectoryObjectGuard) -> Result<T>,
        after_operation: impl FnOnce(),
    ) -> Result<T> {
        self.with_external_lease_root_after_operation(
            binding,
            namespace,
            id,
            label,
            operation,
            after_operation,
        )
    }

    /// Open one external lease through the held host root. The returned
    /// directory guard remains short-lived: callers must drop it before
    /// publishing a path and then revalidate this same binding.
    #[cfg(windows)]
    fn open_external_lease_root(
        &self,
        binding: &WindowsExternalLeaseIdentity,
        namespace: &str,
        id: &str,
        label: &str,
    ) -> Result<state_paths::WindowsDirectoryObjectGuard> {
        self.verify_current()?;
        let namespace_guard = self.directory_guard.open_verified_direct_child(
            &binding.namespace,
            OsStr::new(namespace),
            "validation host-temp lease 命名空间",
        )?;
        let root_guard =
            namespace_guard.open_verified_direct_child(&binding.root, OsStr::new(id), label);
        drop(namespace_guard);
        self.verify_current()?;
        root_guard
    }

    /// Reopen both external namespace components from the held host root after
    /// an operation has released its child handle. This is deliberately a
    /// separate final step rather than a lock: `FILE_SHARE_DELETE` remains
    /// enabled, so a namespace rename is permitted but cannot be published as
    /// a successful validation lease.
    #[cfg(windows)]
    fn verify_external_lease_binding(
        &self,
        binding: &WindowsExternalLeaseIdentity,
        namespace: &str,
        id: &str,
        label: &str,
    ) -> Result<()> {
        let root_guard = self.open_external_lease_root(binding, namespace, id, label)?;
        drop(root_guard);
        self.verify_current()
    }

    /// Remove a manifest-owned external lease through the held host root.
    /// No cleanup path reopens the configured root by absolute pathname.
    #[cfg(windows)]
    pub(crate) fn remove_external_lease_tree<F, G>(
        &self,
        relative: &Path,
        verifier: F,
        snapshot_verifier: G,
    ) -> Result<bool>
    where
        F: FnOnce(&fs::File, &Path) -> Result<()>,
        G: FnOnce(&Path, &fs::File) -> Result<()>,
    {
        self.directory_guard
            .remove_relative_tree_verified_with_snapshot(relative, verifier, snapshot_verifier)
    }
}

struct ValidationProcessLeaseGuard {
    state_root: ValidationProcessStateRoot,
    lease: Option<ManagedValidationProcessLease>,
}

impl ValidationProcessLeaseGuard {
    fn prepare(workspace: &Path, label: &str) -> Result<Self> {
        let state_root = acquire_validation_process_state_root(workspace)?;
        let lease = create_validation_process_lease_at_host_root(&state_root, label)?;
        Ok(Self {
            state_root,
            lease: Some(lease),
        })
    }

    fn environment(&self) -> Result<&BTreeMap<String, String>> {
        let expected = self
            .lease
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("validation process lease 已释放"))?;
        self.state_root.verify_current()?;
        expected.verify_current_at_host_root(&self.state_root)?;
        self.state_root.verify_current()?;
        Ok(&expected.lease.environment)
    }

    fn finish(&mut self) -> Result<()> {
        let Some(lease) = self.lease.take() else {
            return Ok(());
        };
        release_validation_process_lease_at_host_root(&self.state_root, &lease)
    }

    fn finish_with(&mut self, result: Result<Output>) -> Result<Output> {
        let cleanup = self.finish();
        match (result, cleanup) {
            (Ok(output), Ok(())) => Ok(output),
            (Err(error), Ok(())) => Err(error),
            (Ok(output), Err(cleanup_error)) if output.status.success() => bail!(
                "validation process 成功退出但 host-temp lease 释放失败；不会写入 receipt；stdout_sha256={} stderr_sha256={}: {cleanup_error:#}",
                sha256_hex(&output.stdout),
                sha256_hex(&output.stderr)
            ),
            (Ok(output), Err(cleanup_error)) => bail!(
                "validation process 非零退出（exit={}）且 host-temp lease 释放失败；不会写入 receipt；stdout_sha256={} stderr_sha256={}: {cleanup_error:#}",
                output.status.code().unwrap_or(-1),
                sha256_hex(&output.stdout),
                sha256_hex(&output.stderr)
            ),
            (Err(error), Err(cleanup_error)) => Err(error).context(format!(
                "validation process 启动失败后 host-temp lease 释放也失败；不会写入 receipt: {cleanup_error:#}"
            )),
        }
    }
}

impl Drop for ValidationProcessLeaseGuard {
    fn drop(&mut self) {
        if let Some(lease) = self.lease.take() {
            let _ = release_validation_process_lease_at_host_root(&self.state_root, &lease);
        }
    }
}

pub(crate) fn run_with_validation_process_lease(
    workspace: &Path,
    label: &str,
    runner: impl FnOnce(Option<&BTreeMap<String, String>>) -> Result<Output>,
) -> Result<Output> {
    if !cfg!(windows) {
        return runner(None);
    }
    let mut guard = ValidationProcessLeaseGuard::prepare(workspace, label)?;
    let execution = guard
        .environment()
        .and_then(|environment| runner(Some(environment)));
    guard.finish_with(execution)
}

#[cfg(any(not(windows), test))]
pub(crate) fn validation_process_state_root(workspace: &Path) -> Result<PathBuf> {
    if !cfg!(windows) {
        return Ok(workspace.to_path_buf());
    }
    let candidate = validation_process_root_candidate(
        std::env::var_os(VALIDATION_TEMP_ROOT_ENV),
        std::env::var_os(CARGO_HOME_ENV),
        std::env::var_os(LOCALAPPDATA_ENV),
    )?;
    prepare_validation_process_state_root(workspace, &candidate)
}

pub(crate) fn acquire_validation_process_state_root(
    workspace: &Path,
) -> Result<ValidationProcessStateRoot> {
    #[cfg(not(windows))]
    {
        return bind_validation_process_state_root(validation_process_state_root(workspace)?);
    }

    #[cfg(windows)]
    {
        let candidate = validation_process_root_candidate(
            std::env::var_os(VALIDATION_TEMP_ROOT_ENV),
            std::env::var_os(CARGO_HOME_ENV),
            std::env::var_os(LOCALAPPDATA_ENV),
        )?;
        let root = bind_validation_process_state_root(prepare_validation_process_state_root(
            workspace, &candidate,
        )?)?;
        // Do not invoke the path-based bootstrap a second time once this root
        // is held: doing so would send a raw probe through a replacement
        // namespace. The bound probe uses only the held object handle and
        // revalidates its path before and after I/O.
        root.probe_current()?;
        Ok(root)
    }
}

fn bind_validation_process_state_root(path: PathBuf) -> Result<ValidationProcessStateRoot> {
    #[cfg(windows)]
    let directory_guard =
        state_paths::hold_windows_directory_object(&path, "validation host-temp 根").with_context(
            || {
                format!(
                    "无法持有 validation host-temp 根；拒绝向身份未知的根创建 lease: {}",
                    display_path(&path)
                )
            },
        )?;
    let root = ValidationProcessStateRoot {
        path,
        #[cfg(windows)]
        directory_guard,
    };
    root.verify_current()?;
    Ok(root)
}

fn validation_process_root_candidate(
    explicit: Option<OsString>,
    cargo_home: Option<OsString>,
    local_app_data: Option<OsString>,
) -> Result<PathBuf> {
    if let Some(explicit) = explicit {
        if explicit.is_empty() {
            bail!("{VALIDATION_TEMP_ROOT_ENV} 已设置但为空；拒绝静默回退");
        }
        return Ok(PathBuf::from(explicit));
    }
    if let Some(cargo_home) = cargo_home.filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(cargo_home)
            .join(CODEX_SANDBOX_DIR)
            .join(RAYMAN_VALIDATION_DIR));
    }
    if let Some(local_app_data) = local_app_data.filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(local_app_data)
            .join("Rayman")
            .join(CODEX_SANDBOX_DIR)
            .join(RAYMAN_VALIDATION_DIR));
    }
    bail!(
        "Windows validation host-temp 缺少 {VALIDATION_TEMP_ROOT_ENV}、{CARGO_HOME_ENV} 和 {LOCALAPPDATA_ENV}"
    )
}

fn prepare_validation_process_state_root(workspace: &Path, candidate: &Path) -> Result<PathBuf> {
    if !candidate.is_absolute() {
        bail!(
            "Windows validation host-temp 必须是绝对路径: {}",
            display_path(candidate)
        );
    }
    let workspace = workspace
        .canonicalize()
        .with_context(|| format!("无法规范化 validation 工作区: {}", display_path(workspace)))?;
    let projected = project_validation_process_state_root(candidate)?;
    if windows_paths_overlap(&projected, &workspace) {
        bail!(
            "validation host-temp 必须与工作区互不包含: temp={} workspace={}",
            display_path(&projected),
            display_path(&workspace)
        );
    }
    ensure_validation_root_outside_workspace_markers(&projected)?;
    #[cfg(not(windows))]
    create_real_directory_chain(candidate)?;
    let state_root = candidate.canonicalize().with_context(|| {
        format!(
            "无法规范化 validation host-temp（Windows 根必须由受管配置预先创建）: {}",
            display_path(candidate)
        )
    })?;
    if !state_root
        .components()
        .any(|component| matches!(component, Component::Normal(_)))
    {
        bail!(
            "validation host-temp 必须是卷内专用子目录，不能直接使用磁盘根: {}",
            display_path(&state_root)
        );
    }
    crate::file_io::ensure_real_directory_labeled(&state_root, "validation host-temp 根")?;
    // Recheck after creation so a namespace race cannot invalidate the
    // no-overlap decision made before any directory was created.
    if windows_paths_overlap(&state_root, &workspace) {
        bail!(
            "validation host-temp 必须与工作区互不包含: temp={} workspace={}",
            display_path(&state_root),
            display_path(&workspace)
        );
    }
    // Recheck after creation for the same reason as the overlap check: a
    // concurrent namespace change must not turn the lease authority tree into
    // a workspace-discovery ancestor.
    ensure_validation_root_outside_workspace_markers(&state_root)?;
    #[cfg(not(windows))]
    probe_directory(&state_root).with_context(|| {
        format!(
            "validation host-temp 根不可写/读/删: {}",
            display_path(&state_root)
        )
    })?;
    Ok(state_root)
}

/// Resolve the deepest existing real ancestor and append the still-missing
/// suffix without creating it. This makes workspace-overlap rejection
/// side-effect free, including when the configured leaf does not exist yet.
fn project_validation_process_state_root(path: &Path) -> Result<PathBuf> {
    let mut existing = PathBuf::new();
    let mut missing = Vec::<OsString>::new();
    let mut missing_started = false;
    let mut saw_normal = false;

    for component in path.components() {
        match component {
            Component::Prefix(prefix) => existing.push(prefix.as_os_str()),
            Component::RootDir => existing.push(component.as_os_str()),
            Component::Normal(name) => {
                saw_normal = true;
                if missing_started {
                    missing.push(name.to_os_string());
                    continue;
                }
                existing.push(name);
                match fs::symlink_metadata(&existing) {
                    Ok(metadata) => ensure_real_directory_metadata(&existing, &metadata)?,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        existing.pop();
                        missing.push(name.to_os_string());
                        missing_started = true;
                    }
                    Err(error) => {
                        return Err(error).with_context(|| {
                            format!(
                                "无法读取 validation host-temp 路径: {}",
                                display_path(&existing)
                            )
                        });
                    }
                }
            }
            Component::CurDir | Component::ParentDir => bail!(
                "validation host-temp 含不安全路径分量: {}",
                display_path(path)
            ),
        }
    }
    if !saw_normal {
        bail!(
            "validation host-temp 必须是卷内专用子目录，不能直接使用磁盘根: {}",
            display_path(path)
        );
    }

    let mut projected = existing.canonicalize().with_context(|| {
        format!(
            "无法规范化 validation host-temp: {}",
            display_path(&existing)
        )
    })?;
    projected.extend(missing);
    Ok(projected)
}

#[cfg(not(windows))]
fn create_real_directory_chain(path: &Path) -> Result<()> {
    let mut current = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => current.push(prefix.as_os_str()),
            Component::RootDir => current.push(component.as_os_str()),
            Component::Normal(name) => {
                current.push(name);
                match fs::symlink_metadata(&current) {
                    Ok(metadata) => ensure_real_directory_metadata(&current, &metadata)?,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        fs::create_dir(&current).with_context(|| {
                            format!(
                                "无法创建 validation host-temp 目录: {}",
                                display_path(&current)
                            )
                        })?;
                        let metadata = fs::symlink_metadata(&current).with_context(|| {
                            format!(
                                "无法读取新建 validation host-temp 目录: {}",
                                display_path(&current)
                            )
                        })?;
                        ensure_real_directory_metadata(&current, &metadata)?;
                    }
                    Err(error) => {
                        return Err(error).with_context(|| {
                            format!(
                                "无法读取 validation host-temp 路径: {}",
                                display_path(&current)
                            )
                        });
                    }
                }
            }
            Component::CurDir | Component::ParentDir => bail!(
                "validation host-temp 含不安全路径分量: {}",
                display_path(path)
            ),
        }
    }
    Ok(())
}

fn ensure_real_directory_metadata(path: &Path, metadata: &fs::Metadata) -> Result<()> {
    if is_link_or_reparse(metadata) {
        bail!(
            "validation host-temp 路径拒绝链接/reparse: {}",
            display_path(path)
        );
    }
    if !metadata.file_type().is_dir() {
        bail!("validation host-temp 路径不是目录: {}", display_path(path));
    }
    Ok(())
}

fn windows_paths_overlap(left: &Path, right: &Path) -> bool {
    fn normalized(path: &Path) -> String {
        display_path(path)
            .replace('/', "\\")
            .trim_end_matches('\\')
            .to_ascii_lowercase()
    }
    let left = normalized(left);
    let right = normalized(right);
    left == right
        || left
            .strip_prefix(&right)
            .is_some_and(|tail| tail.starts_with('\\'))
        || right
            .strip_prefix(&left)
            .is_some_and(|tail| tail.starts_with('\\'))
}

fn ensure_validation_root_outside_workspace_markers(path: &Path) -> Result<()> {
    for ancestor in path.ancestors() {
        for marker_name in [".RaymanCodingSkill", ".git"] {
            let marker = ancestor.join(marker_name);
            match fs::symlink_metadata(&marker) {
                Ok(_) => bail!(
                    "validation host-temp 不能位于工作区标记之内: temp={} marker={}",
                    display_path(path),
                    display_path(&marker)
                ),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!(
                            "无法读取 validation host-temp 路径: {}",
                            display_path(&marker)
                        )
                    });
                }
            }
        }
    }
    Ok(())
}

fn validation_process_lease_relative(id: &str) -> Result<PathBuf> {
    if !is_valid_lease_id(id) {
        bail!("无效 validation process lease id: {id}");
    }
    Ok(Path::new(LEASES_RELATIVE).join(id))
}

/// Production Windows creation starts from the held host root. The legacy
/// path-rooted constructor remains only for non-Windows and focused unit-test
/// fixtures; it must never be reached by a real Windows validation child.
fn create_validation_process_lease_at_host_root(
    state_root: &ValidationProcessStateRoot,
    label: &str,
) -> Result<ManagedValidationProcessLease> {
    #[cfg(windows)]
    {
        create_validation_process_lease_with_held_host_root(state_root, label)
    }

    #[cfg(not(windows))]
    {
        create_validation_process_lease(state_root.path(), label)
    }
}

#[cfg(all(windows, test))]
fn create_validation_process_lease(
    state_root: &Path,
    label: &str,
) -> Result<ManagedValidationProcessLease> {
    let host_root = bind_validation_process_state_root(state_root.to_path_buf())?;
    create_validation_process_lease_with_held_host_root(&host_root, label)
}

#[cfg(windows)]
fn create_validation_process_lease_with_held_host_root(
    state_root: &ValidationProcessStateRoot,
    label: &str,
) -> Result<ManagedValidationProcessLease> {
    state_root.verify_current()?;
    let label = label.trim();
    let safe_label = safe_lease_id_label(label);
    for _ in 0..64 {
        let sequence = LEASE_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let timestamp = crate::timefmt::now_iso()
            .chars()
            .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
            .collect::<String>();
        let id = format!("{safe_label}-{timestamp}-{}-{sequence}", std::process::id());
        let binding = match state_root.create_external_lease_identity(
            LEASES_RELATIVE,
            &id,
            "validation process lease 创建目录",
        ) {
            Ok(binding) => binding,
            Err(error) if windows_creation_collision(&error) => continue,
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "无法通过持有 validation host-temp 根独占创建 validation process lease: {}",
                        display_path(&state_root.path().join(LEASES_RELATIVE).join(&id))
                    )
                });
            }
        };
        let relative = validation_process_lease_relative(&id)?;
        let mut partial = WindowsValidationProcessLeasePartial::default();
        let creation = state_root.with_external_lease_root(
            &binding,
            LEASES_RELATIVE,
            &id,
            "validation process lease 创建目录",
            |root_guard| {
                let lease_root = root_guard.path().to_path_buf();
                partial.temp_dir = Some(root_guard.create_child_exclusive(
                    OsStr::new(TEMP_DIR),
                    "validation process lease 临时目录",
                )?);
                partial.nested_validation_root = Some(root_guard.create_child_exclusive(
                    OsStr::new(NESTED_VALIDATION_DIR),
                    "validation process lease 嵌套验证根",
                )?);
                let temp_dir = partial
                    .temp_dir
                    .as_ref()
                    .expect("just created validation temp directory")
                    .path()
                    .to_path_buf();
                let nested_validation_root = partial
                    .nested_validation_root
                    .as_ref()
                    .expect("just created validation nested root")
                    .path()
                    .to_path_buf();
                root_guard.probe_direct_child(
                    partial
                        .temp_dir
                        .as_ref()
                        .expect("just created validation temp directory"),
                    OsStr::new(TEMP_DIR),
                    "validation process lease 临时目录",
                )?;
                root_guard.probe_direct_child(
                    partial
                        .nested_validation_root
                        .as_ref()
                        .expect("just created validation nested root"),
                    OsStr::new(NESTED_VALIDATION_DIR),
                    "validation process lease 嵌套验证根",
                )?;
                let environment = temp_environment(&temp_dir, &nested_validation_root);
                let lease = ValidationProcessLease {
                    schema: "rayman.validation-process-lease.v1".into(),
                    id: id.clone(),
                    label: label.to_string(),
                    created_at: crate::timefmt::now_iso(),
                    root: path_text(&lease_root),
                    temp_dir: path_text(&temp_dir),
                    nested_validation_root: path_text(&nested_validation_root),
                    environment,
                };
                let manifest = create_windows_lease_manifest(
                    root_guard,
                    LEASE_MANIFEST,
                    "validation process lease manifest",
                    &lease,
                )?;
                partial.lease = Some(lease);
                partial.manifest = Some(manifest);
                partial.verify_created(root_guard)?;
                Ok(WindowsValidationProcessLeaseCreation {
                    lease: partial
                        .lease
                        .take()
                        .expect("just sealed validation process lease"),
                    temp_dir: partial
                        .temp_dir
                        .take()
                        .expect("just created validation temp directory"),
                    nested_validation_root: partial
                        .nested_validation_root
                        .take()
                        .expect("just created validation nested root"),
                    manifest: partial
                        .manifest
                        .take()
                        .expect("just sealed validation process lease manifest"),
                })
            },
        );
        return match creation {
            Ok(created) => Ok(ManagedValidationProcessLease {
                lease: created.lease,
                directory_guards: WindowsValidationProcessLeaseBinding::External(
                    WindowsExternalValidationProcessLeaseGuards {
                        binding,
                        temp_dir: created.temp_dir,
                        nested_validation_root: created.nested_validation_root,
                        manifest: created.manifest,
                    },
                ),
            }),
            Err(creation_error) => {
                let cleanup = state_root.remove_external_lease_tree(
                    &relative,
                    |leaf, current_root| {
                        state_paths::verify_windows_directory_child_identity_from_open(
                            &binding.root,
                            leaf,
                            current_root,
                            "validation process lease 创建目录",
                        )?;
                        state_root.with_external_lease_root(
                            &binding,
                            LEASES_RELATIVE,
                            &id,
                            "validation process lease 创建目录",
                            |root| partial.verify_created(root),
                        )?;
                        state_root.verify_current()
                    },
                    |snapshot_path, snapshot_leaf| {
                        state_paths::verify_windows_directory_child_identity_from_open(
                            &binding.root,
                            snapshot_leaf,
                            snapshot_path,
                            "validation process lease 创建目录",
                        )?;
                        state_root.with_external_lease_root(
                            &binding,
                            LEASES_RELATIVE,
                            &id,
                            "validation process lease 创建目录",
                            |root| partial.verify_created(root),
                        )?;
                        state_root.verify_current()
                    },
                );
                match cleanup {
                    Ok(true) => Err(creation_error),
                    Ok(false) => Err(creation_error)
                        .context("validation process lease 创建失败后受管目录已消失"),
                    Err(cleanup_error) => Err(creation_error).context(format!(
                        "validation process lease 创建或探测失败后的清理也失败: {cleanup_error:#}"
                    )),
                }
            }
        };
    }
    bail!("无法独占创建 validation process lease（连续名称冲突）")
}

#[cfg(not(windows))]
fn create_validation_process_lease(
    state_root: &Path,
    label: &str,
) -> Result<ManagedValidationProcessLease> {
    create_validation_process_lease_with_probe(state_root, label, probe_directory)
}

#[cfg(not(windows))]
fn create_validation_process_lease_with_probe<F>(
    state_root: &Path,
    label: &str,
    #[allow(unused_variables, unused_mut)] mut probe: F,
) -> Result<ManagedValidationProcessLease>
where
    F: FnMut(&Path) -> Result<()>,
{
    let parent = state_paths::managed_external_dir(state_root, Path::new(LEASES_RELATIVE), true)?
        .ok_or_else(|| anyhow::anyhow!("无法创建 validation process lease 根"))?;
    let label = label.trim();
    let safe_label = safe_lease_id_label(label);
    for _ in 0..64 {
        let sequence = LEASE_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let timestamp = crate::timefmt::now_iso()
            .chars()
            .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
            .collect::<String>();
        let id = format!("{safe_label}-{timestamp}-{}-{sequence}", std::process::id());
        #[cfg(windows)]
        {
            let root_guard = match state_paths::create_windows_directory_object_exclusive(
                &parent,
                OsStr::new(&id),
                "validation process lease 创建目录",
            ) {
                Ok(root) => root,
                Err(error) if windows_creation_collision(&error) => continue,
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!(
                            "无法独占创建 validation process lease: {}",
                            display_path(&parent.join(&id))
                        )
                    });
                }
            };
            let relative = validation_process_lease_relative(&id)?;
            let mut partial = WindowsValidationProcessLeasePartial::default();
            let creation = (|| -> Result<WindowsValidationProcessLeaseCreation> {
                let lease_root = root_guard.path().to_path_buf();
                partial.temp_dir = Some(root_guard.create_child_exclusive(
                    OsStr::new(TEMP_DIR),
                    "validation process lease 临时目录",
                )?);
                partial.nested_validation_root = Some(root_guard.create_child_exclusive(
                    OsStr::new(NESTED_VALIDATION_DIR),
                    "validation process lease 嵌套验证根",
                )?);
                let temp_dir = partial
                    .temp_dir
                    .as_ref()
                    .expect("just created validation temp directory")
                    .path()
                    .to_path_buf();
                let nested_validation_root = partial
                    .nested_validation_root
                    .as_ref()
                    .expect("just created validation nested root")
                    .path()
                    .to_path_buf();
                root_guard.probe_direct_child(
                    partial
                        .temp_dir
                        .as_ref()
                        .expect("just created validation temp directory"),
                    OsStr::new(TEMP_DIR),
                    "validation process lease 临时目录",
                )?;
                root_guard.probe_direct_child(
                    partial
                        .nested_validation_root
                        .as_ref()
                        .expect("just created validation nested root"),
                    OsStr::new(NESTED_VALIDATION_DIR),
                    "validation process lease 嵌套验证根",
                )?;
                let environment = temp_environment(&temp_dir, &nested_validation_root);
                let lease = ValidationProcessLease {
                    schema: "rayman.validation-process-lease.v1".into(),
                    id: id.clone(),
                    label: label.to_string(),
                    created_at: crate::timefmt::now_iso(),
                    root: path_text(&lease_root),
                    temp_dir: path_text(&temp_dir),
                    nested_validation_root: path_text(&nested_validation_root),
                    environment,
                };
                let manifest = create_windows_lease_manifest(
                    &root_guard,
                    LEASE_MANIFEST,
                    "validation process lease manifest",
                    &lease,
                )?;
                partial.lease = Some(lease);
                partial.manifest = Some(manifest);
                partial.verify_created(&root_guard)?;
                Ok(WindowsValidationProcessLeaseCreation {
                    lease: partial
                        .lease
                        .take()
                        .expect("just sealed validation process lease"),
                    temp_dir: partial
                        .temp_dir
                        .take()
                        .expect("just created validation temp directory"),
                    nested_validation_root: partial
                        .nested_validation_root
                        .take()
                        .expect("just created validation nested root"),
                    manifest: partial
                        .manifest
                        .take()
                        .expect("just sealed validation process lease manifest"),
                })
            })();
            return match creation {
                Ok(created) => Ok(ManagedValidationProcessLease {
                    lease: created.lease,
                    directory_guards: WindowsValidationProcessLeaseBinding::Direct(
                        WindowsValidationProcessLeaseGuards {
                            root: root_guard,
                            temp_dir: created.temp_dir,
                            nested_validation_root: created.nested_validation_root,
                            manifest: created.manifest,
                        },
                    ),
                }),
                Err(creation_error) => {
                    let cleanup =
                        state_paths::remove_managed_external_dir_all_windows_verified_with_snapshot(
                            state_root,
                            &relative,
                            |leaf, current_root| {
                                state_paths::verify_windows_directory_object(
                                    &root_guard,
                                    leaf,
                                    current_root,
                                    "validation process lease 创建目录",
                                )?;
                                partial.verify_created(&root_guard)
                            },
                            |snapshot_path, snapshot_leaf| {
                                state_paths::verify_windows_directory_object(
                                    &root_guard,
                                    snapshot_leaf,
                                    snapshot_path,
                                    "validation process lease 创建目录",
                                )?;
                                partial.verify_created(&root_guard)
                            },
                        );
                    match cleanup {
                        Ok(true) => Err(creation_error),
                        Ok(false) => Err(creation_error)
                            .context("validation process lease 创建失败后受管目录已消失"),
                        Err(cleanup_error) => Err(creation_error).context(format!(
                            "validation process lease 创建或探测失败后的清理也失败: {cleanup_error:#}"
                        )),
                    }
                }
            };
        }

        #[cfg(not(windows))]
        {
            let lease_root = parent.join(&id);
            match fs::create_dir(&lease_root) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!(
                            "无法独占创建 validation process lease: {}",
                            display_path(&lease_root)
                        )
                    });
                }
            }

            let relative = validation_process_lease_relative(&id)?;
            let creation = (|| {
                let lease_root = state_paths::managed_external_dir(state_root, &relative, false)?
                    .ok_or_else(|| {
                    anyhow::anyhow!("validation process lease 创建后消失: {id}")
                })?;
                let temp_dir = lease_root.join(TEMP_DIR);
                fs::create_dir(&temp_dir).with_context(|| {
                    format!(
                        "无法创建 validation process lease 临时目录: {}",
                        display_path(&temp_dir)
                    )
                })?;
                probe(&temp_dir)?;
                let nested_validation_root = lease_root.join(NESTED_VALIDATION_DIR);
                fs::create_dir(&nested_validation_root).with_context(|| {
                    format!(
                        "无法独占创建 validation process 嵌套验证根: {}",
                        display_path(&nested_validation_root)
                    )
                })?;
                probe(&nested_validation_root)?;
                let environment = temp_environment(&temp_dir, &nested_validation_root);
                let lease = ValidationProcessLease {
                    schema: "rayman.validation-process-lease.v1".into(),
                    id: id.clone(),
                    label: label.to_string(),
                    created_at: crate::timefmt::now_iso(),
                    root: path_text(&lease_root),
                    temp_dir: path_text(&temp_dir),
                    nested_validation_root: path_text(&nested_validation_root),
                    environment,
                };
                crate::file_io::write_json(&lease_root.join(LEASE_MANIFEST), &lease)?;
                verify_validation_process_lease(state_root, &id)
            })();
            return match creation {
                Ok(lease) => Ok(ManagedValidationProcessLease { lease }),
                Err(creation_error) => {
                    let cleanup =
                        state_paths::remove_managed_external_dir_all(state_root, &relative);
                    match cleanup {
                        Ok(true) => Err(creation_error),
                        Ok(false) => Err(creation_error)
                            .context("validation process lease 创建失败后受管目录已消失"),
                        Err(cleanup_error) => Err(creation_error).context(format!(
                            "validation process lease 创建或探测失败后的清理也失败: {cleanup_error:#}"
                        )),
                    }
                }
            };
        }
    }
    bail!("无法独占创建 validation process lease（连续名称冲突）")
}

fn temp_environment(temp_dir: &Path, nested_validation_root: &Path) -> BTreeMap<String, String> {
    let mut environment = TEMP_KEYS
        .into_iter()
        .map(|name| (name.to_string(), path_text(temp_dir)))
        .collect::<BTreeMap<_, _>>();
    environment.insert(
        VALIDATION_TEMP_ROOT_ENV.to_string(),
        path_text(nested_validation_root),
    );
    environment
}

#[cfg(not(windows))]
fn load_validation_process_lease(
    state_root: &Path,
    id: &str,
) -> Result<(PathBuf, ValidationProcessLease)> {
    let relative = validation_process_lease_relative(id)?;
    let lease_root = state_paths::managed_external_dir(state_root, &relative, false)?
        .ok_or_else(|| anyhow::anyhow!("validation process lease 不存在: {id}"))?;
    let lease =
        crate::file_io::read_json::<ValidationProcessLease>(&lease_root.join(LEASE_MANIFEST))?
            .ok_or_else(|| anyhow::anyhow!("validation process lease 缺少 manifest: {id}"))?;
    validate_validation_process_manifest(id, &lease_root, &lease)?;
    Ok((lease_root, lease))
}

fn validate_validation_process_manifest(
    id: &str,
    lease_root: &Path,
    lease: &ValidationProcessLease,
) -> Result<()> {
    let temp_dir = lease_root.join(TEMP_DIR);
    let nested_validation_root = lease_root.join(NESTED_VALIDATION_DIR);
    if lease.schema != "rayman.validation-process-lease.v1"
        || lease.id != id
        || lease.label.trim() != lease.label
        || !lease_id_matches_label(id, &safe_lease_id_label(&lease.label))
        || lease.root != path_text(lease_root)
        || lease.temp_dir != path_text(&temp_dir)
        || lease.nested_validation_root != path_text(&nested_validation_root)
        || lease.environment != temp_environment(&temp_dir, &nested_validation_root)
    {
        bail!("validation process lease manifest 与受管路径不一致: {id}");
    }
    Ok(())
}

fn parse_validation_process_manifest(
    id: &str,
    lease_root: &Path,
    bytes: &[u8],
) -> Result<ValidationProcessLease> {
    let lease = serde_json::from_slice::<ValidationProcessLease>(bytes)
        .with_context(|| format!("validation process lease manifest 无法解析: {id}"))?;
    validate_validation_process_manifest(id, lease_root, &lease)?;
    Ok(lease)
}

#[cfg(not(windows))]
fn verify_validation_process_lease(state_root: &Path, id: &str) -> Result<ValidationProcessLease> {
    let (_, lease) = load_validation_process_lease(state_root, id)?;
    let temp_dir = PathBuf::from(&lease.temp_dir);
    let nested_validation_root = PathBuf::from(&lease.nested_validation_root);
    for (path, label) in [
        (&temp_dir, "validation process lease 临时目录"),
        (
            &nested_validation_root,
            "validation process lease 嵌套验证根",
        ),
    ] {
        crate::file_io::ensure_real_directory_labeled(path, label)?;
        probe_directory(path)?;
    }
    Ok(lease)
}

/// Release the production Windows lease through the exact host-root object
/// bound before creation. This deliberately does not call the path-rooted
/// release helper: a replacement of the configured root must leave both the
/// original orphan and the replacement untouched.
fn release_validation_process_lease_at_host_root(
    state_root: &ValidationProcessStateRoot,
    expected: &ManagedValidationProcessLease,
) -> Result<()> {
    #[cfg(windows)]
    {
        state_root.verify_current()?;
        expected.verify_current_at_host_root(state_root)?;
        let manifest = &expected.lease;
        let relative = validation_process_lease_relative(&manifest.id)?;
        let removed = state_root
            .remove_external_lease_tree(
                &relative,
                |leaf, lease_root| {
                    let bytes = match &expected.directory_guards {
                        WindowsValidationProcessLeaseBinding::Direct(guards) => {
                            state_paths::verify_windows_directory_object(
                                &guards.root,
                                leaf,
                                lease_root,
                                "validation process lease 创建目录",
                            )?;
                            verify_windows_lease_manifest(
                                &guards.root,
                                &guards.manifest,
                                LEASE_MANIFEST,
                                "validation process lease manifest",
                            )?
                        }
                        WindowsValidationProcessLeaseBinding::External(guards) => {
                            state_paths::verify_windows_directory_child_identity_from_open(
                                &guards.binding.root,
                                leaf,
                                lease_root,
                                "validation process lease 创建目录",
                            )?;
                            guards.verify_current_at_host_root(state_root, manifest)?;
                            verify_windows_lease_manifest_from_directory_handle(
                                leaf,
                                lease_root,
                                &guards.manifest,
                                LEASE_MANIFEST,
                                "validation process lease manifest",
                            )?
                        }
                    };
                    let actual =
                        parse_validation_process_manifest(&manifest.id, lease_root, &bytes)?;
                    if actual != *manifest {
                        bail!(
                            "validation process lease 释放身份与 session 不一致: {}",
                            manifest.id
                        );
                    }
                    expected.verify_current_at_host_root(state_root)?;
                    state_root.verify_current()
                },
                |snapshot_path, snapshot_leaf| {
                    match &expected.directory_guards {
                        WindowsValidationProcessLeaseBinding::Direct(guards) => {
                            state_paths::verify_windows_directory_object(
                                &guards.root,
                                snapshot_leaf,
                                snapshot_path,
                                "validation process lease 创建目录",
                            )?;
                        }
                        WindowsValidationProcessLeaseBinding::External(guards) => {
                            state_paths::verify_windows_directory_child_identity_from_open(
                                &guards.binding.root,
                                snapshot_leaf,
                                snapshot_path,
                                "validation process lease 创建目录",
                            )?;
                        }
                    }
                    expected.verify_current_at_host_root(state_root)?;
                    state_root.verify_current()
                },
            )
            .with_context(|| format!("无法释放 validation process lease: {}", manifest.id))?;
        if !removed {
            bail!(
                "validation process lease 在已验证释放前消失: {}",
                manifest.id
            );
        }
        state_root.verify_current()?;
        Ok(())
    }

    #[cfg(not(windows))]
    {
        release_validation_process_lease(state_root.path(), expected)
    }
}

#[cfg(not(windows))]
fn release_validation_process_lease(
    state_root: &Path,
    expected: &ManagedValidationProcessLease,
) -> Result<()> {
    #[cfg(windows)]
    {
        expected.verify_current()?;
        let WindowsValidationProcessLeaseBinding::Direct(guards) = &expected.directory_guards
        else {
            bail!("validation process external lease 必须通过持有 validation host-temp 根释放");
        };
        let manifest = &expected.lease;
        let relative = validation_process_lease_relative(&manifest.id)?;
        let removed = state_paths::remove_managed_external_dir_all_windows_verified_with_snapshot(
            state_root,
            &relative,
            |leaf, lease_root| {
                state_paths::verify_windows_directory_object(
                    &guards.root,
                    leaf,
                    lease_root,
                    "validation process lease 创建目录",
                )?;
                let bytes = verify_windows_lease_manifest(
                    &guards.root,
                    &guards.manifest,
                    LEASE_MANIFEST,
                    "validation process lease manifest",
                )?;
                let actual = parse_validation_process_manifest(&manifest.id, lease_root, &bytes)?;
                if actual != *manifest {
                    bail!(
                        "validation process lease 释放身份与 session 不一致: {}",
                        manifest.id
                    );
                }
                expected.verify_current()
            },
            |snapshot_path, snapshot_leaf| {
                state_paths::verify_windows_directory_object(
                    &guards.root,
                    snapshot_leaf,
                    snapshot_path,
                    "validation process lease 创建目录",
                )?;
                expected.verify_current()
            },
        )
        .with_context(|| format!("无法释放 validation process lease: {}", manifest.id))?;
        if !removed {
            bail!(
                "validation process lease 在已验证释放前消失: {}",
                manifest.id
            );
        }
        Ok(())
    }

    #[cfg(not(windows))]
    {
        let expected = &expected.lease;
        let (lease_root, actual) = load_validation_process_lease(state_root, &expected.id)?;
        if actual != *expected {
            bail!(
                "validation process lease 释放身份与 session 不一致: {}",
                expected.id
            );
        }
        let removed = state_paths::remove_managed_external_dir_all(
            state_root,
            &validation_process_lease_relative(&expected.id)?,
        )
        .with_context(|| {
            format!(
                "无法释放 validation process lease: {}",
                display_path(&lease_root)
            )
        })?;
        if !removed {
            bail!(
                "validation process lease 在已验证释放前消失: {}",
                expected.id
            );
        }
        Ok(())
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output(exit_code: i32) -> Output {
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

    #[test]
    fn leases_are_unique_probed_manifest_owned_and_sibling_safe() {
        let root = tempfile::tempdir().unwrap();
        let host_root = bind_validation_process_state_root(root.path().to_path_buf()).unwrap();
        let first = create_validation_process_lease_at_host_root(&host_root, "validation").unwrap();
        let second =
            create_validation_process_lease_at_host_root(&host_root, "validation").unwrap();
        assert_ne!(first.lease.id, second.lease.id);
        assert_eq!(
            first.lease.environment.get("TEMP"),
            first.lease.environment.get("TMP")
        );
        assert_eq!(
            first.lease.environment.get("TEMP"),
            first.lease.environment.get("TMPDIR")
        );
        let first_root = PathBuf::from(&first.lease.root);
        assert_eq!(
            PathBuf::from(&first.lease.nested_validation_root),
            first_root.join(NESTED_VALIDATION_DIR)
        );
        assert_eq!(
            first.lease.environment.get(VALIDATION_TEMP_ROOT_ENV),
            Some(&first.lease.nested_validation_root)
        );
        assert_ne!(
            first.lease.environment.get("TEMP"),
            first.lease.environment.get(VALIDATION_TEMP_ROOT_ENV)
        );
        first.verify_current_at_host_root(&host_root).unwrap();
        assert!(
            !root.path().join(".RaymanCodingSkill").exists(),
            "external validation leases must not create a workspace marker"
        );
        release_validation_process_lease_at_host_root(&host_root, &first).unwrap();
        assert!(!Path::new(&first.lease.root).exists());
        assert!(Path::new(&second.lease.root).is_dir());
        release_validation_process_lease_at_host_root(&host_root, &second).unwrap();
    }

    #[test]
    fn tampered_manifest_fails_closed_without_deleting_the_lease() {
        let root = tempfile::tempdir().unwrap();
        let host_root = bind_validation_process_state_root(root.path().to_path_buf()).unwrap();
        let lease = create_validation_process_lease_at_host_root(&host_root, "tamper").unwrap();
        let mut tampered = lease.lease.clone();
        tampered.temp_dir = root.path().join("outside").display().to_string();
        crate::file_io::write_json(
            &Path::new(&lease.lease.root).join(LEASE_MANIFEST),
            &tampered,
        )
        .unwrap();
        assert!(release_validation_process_lease_at_host_root(&host_root, &lease).is_err());
        assert!(Path::new(&lease.lease.root).is_dir());
    }

    #[cfg(windows)]
    #[test]
    fn pre_release_leaf_replacement_cannot_mint_validation_success() {
        let root = tempfile::tempdir().unwrap();
        let host_root = bind_validation_process_state_root(root.path().to_path_buf()).unwrap();
        let managed =
            create_validation_process_lease_at_host_root(&host_root, "pre-release-replacement")
                .unwrap();
        let lease = managed.lease.clone();
        let original = PathBuf::from(&lease.root);
        let displaced = original.with_file_name(format!("{}-displaced", lease.id));
        fs::rename(&original, &displaced).unwrap();
        fs::create_dir(&original).unwrap();
        fs::create_dir(original.join(TEMP_DIR)).unwrap();
        crate::file_io::write_json(&original.join(LEASE_MANIFEST), &lease).unwrap();

        let mut guard = ValidationProcessLeaseGuard {
            state_root: host_root,
            lease: Some(managed),
        };
        let error = guard.finish_with(Ok(output(0))).unwrap_err();
        assert!(
            format!("{error:#}").contains("不会写入 receipt"),
            "{error:#}"
        );
        assert!(format!("{error:#}").contains("创建目录"), "{error:#}");
        assert!(
            displaced.is_dir() && original.is_dir(),
            "identity mismatch must preserve both the original object and replacement"
        );
    }

    #[cfg(windows)]
    #[test]
    fn creation_failure_preserves_a_replacement_leaf() {
        let container = tempfile::tempdir().unwrap();
        let original = container.path().join("host-root");
        fs::create_dir(&original).unwrap();
        let host_root = bind_validation_process_state_root(original.clone()).unwrap();
        let managed =
            create_validation_process_lease_at_host_root(&host_root, "creation-replacement")
                .unwrap();
        let lease = managed.lease.clone();
        let lease_root = PathBuf::from(&lease.root);
        let displaced = lease_root.with_file_name(format!("{}-displaced", lease.id));
        fs::rename(&lease_root, &displaced).unwrap();
        fs::create_dir(&lease_root).unwrap();
        fs::write(lease_root.join("replacement-sentinel.txt"), b"replacement").unwrap();

        let error =
            release_validation_process_lease_at_host_root(&host_root, &managed).unwrap_err();
        assert!(format!("{error:#}").contains("创建目录"), "{error:#}");
        assert!(displaced.is_dir());
        assert_eq!(
            fs::read(lease_root.join("replacement-sentinel.txt")).unwrap(),
            b"replacement"
        );
    }

    #[cfg(windows)]
    #[test]
    fn pre_spawn_replacement_blocks_environment_publication() {
        let root = tempfile::tempdir().unwrap();
        let host_root = bind_validation_process_state_root(root.path().to_path_buf()).unwrap();
        let managed =
            create_validation_process_lease_at_host_root(&host_root, "pre-spawn-replacement")
                .unwrap();
        let lease = managed.lease.clone();
        let original = PathBuf::from(&lease.root);
        let displaced = original.with_file_name(format!("{}-displaced", lease.id));
        fs::rename(&original, &displaced).unwrap();
        fs::create_dir(&original).unwrap();
        fs::create_dir(original.join(TEMP_DIR)).unwrap();
        fs::create_dir(original.join(NESTED_VALIDATION_DIR)).unwrap();
        crate::file_io::write_json(&original.join(LEASE_MANIFEST), &lease).unwrap();

        let mut guard = ValidationProcessLeaseGuard {
            state_root: host_root,
            lease: Some(managed),
        };
        let error = guard.environment().unwrap_err();
        assert!(format!("{error:#}").contains("创建目录"), "{error:#}");
        assert!(
            guard.finish().is_err(),
            "replacement must also prevent later cleanup authority"
        );
        assert!(displaced.is_dir() && original.is_dir());
    }

    #[cfg(windows)]
    #[test]
    fn post_operation_namespace_replacement_is_rejected_before_path_publication() {
        let container = tempfile::tempdir().unwrap();
        let original = container.path().join("host-root");
        fs::create_dir(&original).unwrap();
        let host_root = bind_validation_process_state_root(original.clone()).unwrap();
        let managed =
            create_validation_process_lease_at_host_root(&host_root, "post-operation-namespace")
                .unwrap();
        let lease = managed.lease.clone();
        let binding = match &managed.directory_guards {
            WindowsValidationProcessLeaseBinding::External(guards) => guards.binding.clone(),
            WindowsValidationProcessLeaseBinding::Direct(_) => {
                panic!("production external lease must retain a host-root binding")
            }
        };
        let namespace = original.join(LEASES_RELATIVE);
        let displaced = namespace.with_file_name("v-displaced");

        let error = host_root
            .with_external_lease_root_after_operation_for_test(
                &binding,
                LEASES_RELATIVE,
                &lease.id,
                "validation process lease 创建目录",
                |_| Ok(()),
                || {
                    // This happens after the operation's own final leaf
                    // check. FILE_SHARE_DELETE intentionally allows it; the
                    // helper must reject the replacement before returning a
                    // lexical lease path to its caller.
                    fs::rename(&namespace, &displaced).unwrap();
                    fs::create_dir(&namespace).unwrap();
                    fs::write(namespace.join("replacement-sentinel.txt"), b"replacement").unwrap();
                },
            )
            .unwrap_err();

        assert!(format!("{error:#}").contains("强身份不一致"), "{error:#}");
        assert!(displaced.join(&lease.id).is_dir());
        assert_eq!(
            fs::read(namespace.join("replacement-sentinel.txt")).unwrap(),
            b"replacement"
        );
        assert!(
            release_validation_process_lease_at_host_root(&host_root, &managed).is_err(),
            "a post-operation namespace replacement must also block cleanup authority"
        );
    }

    #[cfg(windows)]
    #[test]
    fn post_operation_lease_replacement_is_rejected_before_path_publication() {
        let container = tempfile::tempdir().unwrap();
        let original = container.path().join("host-root");
        fs::create_dir(&original).unwrap();
        let host_root = bind_validation_process_state_root(original.clone()).unwrap();
        let managed =
            create_validation_process_lease_at_host_root(&host_root, "post-operation-lease")
                .unwrap();
        let lease = managed.lease.clone();
        let binding = match &managed.directory_guards {
            WindowsValidationProcessLeaseBinding::External(guards) => guards.binding.clone(),
            WindowsValidationProcessLeaseBinding::Direct(_) => {
                panic!("production external lease must retain a host-root binding")
            }
        };
        let lease_root = PathBuf::from(&lease.root);
        let displaced = lease_root.with_file_name(format!("{}-displaced", lease.id));

        let error = host_root
            .with_external_lease_root_after_operation_for_test(
                &binding,
                LEASES_RELATIVE,
                &lease.id,
                "validation process lease 创建目录",
                |_| Ok(()),
                || {
                    fs::rename(&lease_root, &displaced).unwrap();
                    fs::create_dir(&lease_root).unwrap();
                    fs::write(lease_root.join("replacement-sentinel.txt"), b"replacement").unwrap();
                },
            )
            .unwrap_err();

        assert!(format!("{error:#}").contains("强身份不一致"), "{error:#}");
        assert!(displaced.is_dir());
        assert_eq!(
            fs::read(lease_root.join("replacement-sentinel.txt")).unwrap(),
            b"replacement"
        );
        assert!(
            release_validation_process_lease_at_host_root(&host_root, &managed).is_err(),
            "a post-operation lease replacement must also block cleanup authority"
        );
    }

    #[cfg(windows)]
    #[test]
    fn child_replacement_blocks_environment_publication_and_release() {
        for (child_name, child_label) in [
            (TEMP_DIR, "临时目录"),
            (NESTED_VALIDATION_DIR, "嵌套验证根"),
        ] {
            let root = tempfile::tempdir().unwrap();
            let host_root = bind_validation_process_state_root(root.path().to_path_buf()).unwrap();
            let managed =
                create_validation_process_lease_at_host_root(&host_root, "child-replacement")
                    .unwrap();
            let lease = managed.lease.clone();
            let original = PathBuf::from(&lease.root).join(child_name);
            fs::write(original.join("original-sentinel.txt"), b"original").unwrap();
            let displaced = original.with_file_name(format!("{child_name}-displaced"));
            fs::rename(&original, &displaced).unwrap();
            fs::create_dir(&original).unwrap();
            fs::write(original.join("replacement-sentinel.txt"), b"replacement").unwrap();

            let mut guard = ValidationProcessLeaseGuard {
                state_root: host_root,
                lease: Some(managed),
            };
            let environment_error = guard.environment().unwrap_err();
            assert!(
                format!("{environment_error:#}").contains(child_label),
                "{environment_error:#}"
            );
            let cleanup_error = guard.finish().unwrap_err();
            assert!(
                format!("{cleanup_error:#}").contains(child_label),
                "{cleanup_error:#}"
            );
            assert_eq!(
                fs::read(displaced.join("original-sentinel.txt")).unwrap(),
                b"original"
            );
            assert_eq!(
                fs::read(original.join("replacement-sentinel.txt")).unwrap(),
                b"replacement"
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn same_content_manifest_replacement_fails_the_strong_identity_seal() {
        let root = tempfile::tempdir().unwrap();
        let host_root = bind_validation_process_state_root(root.path().to_path_buf()).unwrap();
        let managed =
            create_validation_process_lease_at_host_root(&host_root, "manifest-replacement")
                .unwrap();
        let lease_root = PathBuf::from(&managed.lease.root);
        let manifest = lease_root.join(LEASE_MANIFEST);
        let displaced = lease_root.join("lease-displaced.json");
        let bytes = fs::read(&manifest).unwrap();
        fs::rename(&manifest, &displaced).unwrap();
        fs::write(&manifest, &bytes).unwrap();
        assert_eq!(fs::read(&manifest).unwrap(), fs::read(&displaced).unwrap());

        let verify_error = managed.verify_current_at_host_root(&host_root).unwrap_err();
        assert!(
            format!("{verify_error:#}").contains("manifest"),
            "{verify_error:#}"
        );
        let cleanup_error =
            release_validation_process_lease_at_host_root(&host_root, &managed).unwrap_err();
        assert!(
            format!("{cleanup_error:#}").contains("manifest"),
            "{cleanup_error:#}"
        );
        assert_eq!(fs::read(&manifest).unwrap(), bytes);
        assert_eq!(fs::read(&displaced).unwrap(), bytes);
        assert!(
            lease_root.is_dir(),
            "manifest replacement must preserve the lease"
        );
    }

    #[cfg(windows)]
    #[test]
    fn host_root_replacement_is_detected_before_child_publication() {
        let container = tempfile::tempdir().unwrap();
        let original = container.path().join("host-root");
        fs::create_dir(&original).unwrap();
        let host_root = bind_validation_process_state_root(original.clone()).unwrap();
        host_root.probe_current().unwrap();
        let displaced = original.with_file_name("host-root-displaced");
        fs::rename(&original, &displaced).unwrap();
        fs::create_dir(&original).unwrap();

        let error = host_root.verify_current().unwrap_err();
        assert!(
            format!("{error:#}").contains("validation host-temp 根"),
            "{error:#}"
        );
        assert!(displaced.is_dir() && original.is_dir());
    }

    #[cfg(windows)]
    #[test]
    fn host_root_bound_generic_cleanup_preserves_both_roots_after_replacement() {
        let container = tempfile::tempdir().unwrap();
        let original = container.path().join("host-root");
        fs::create_dir(&original).unwrap();
        let host_root = bind_validation_process_state_root(original.clone()).unwrap();
        let managed =
            create_validation_process_lease_at_host_root(&host_root, "host-root").unwrap();
        let lease = managed.lease.clone();
        let displaced = original.with_file_name("host-root-displaced");

        fs::rename(&original, &displaced).unwrap();
        fs::create_dir(&original).unwrap();
        fs::write(original.join("replacement-sentinel.txt"), b"replacement").unwrap();

        let error =
            release_validation_process_lease_at_host_root(&host_root, &managed).unwrap_err();
        assert!(
            format!("{error:#}").contains("validation host-temp 根"),
            "{error:#}"
        );
        assert!(displaced.join("v").join(&lease.id).is_dir());
        assert_eq!(
            fs::read(original.join("replacement-sentinel.txt")).unwrap(),
            b"replacement"
        );
    }

    #[cfg(windows)]
    #[test]
    fn host_root_bound_pytest_cleanup_preserves_both_roots_after_replacement() {
        let container = tempfile::tempdir().unwrap();
        let original = container.path().join("host-root");
        fs::create_dir(&original).unwrap();
        let host_root = bind_validation_process_state_root(original.clone()).unwrap();
        let managed =
            super::super::create_external_managed_pytest_lease_at_host_root(&host_root, "p")
                .unwrap();
        let lease = managed.lease().clone();
        let displaced = original.with_file_name("host-root-displaced");

        fs::rename(&original, &displaced).unwrap();
        fs::create_dir(&original).unwrap();
        fs::write(original.join("replacement-sentinel.txt"), b"replacement").unwrap();

        let error =
            super::super::release_external_managed_pytest_lease_at_host_root(&host_root, &managed)
                .unwrap_err();
        assert!(
            format!("{error:#}").contains("validation host-temp 根"),
            "{error:#}"
        );
        assert!(displaced.join("p").join(&lease.id).is_dir());
        assert_eq!(
            fs::read(original.join("replacement-sentinel.txt")).unwrap(),
            b"replacement"
        );
    }

    #[test]
    fn explicit_empty_root_is_not_silently_replaced() {
        let error = validation_process_root_candidate(
            Some(OsString::new()),
            Some(OsString::from("C:/fallback")),
            None,
        )
        .unwrap_err();
        assert!(error.to_string().contains("拒绝静默回退"));
    }

    #[test]
    fn cargo_home_fallback_stays_below_a_dedicated_directory() {
        let cargo_home = PathBuf::from("cargo-home");
        assert_eq!(
            validation_process_root_candidate(None, Some(cargo_home.clone().into()), None).unwrap(),
            cargo_home
                .join(CODEX_SANDBOX_DIR)
                .join(RAYMAN_VALIDATION_DIR)
        );
    }

    #[cfg(windows)]
    #[test]
    fn drive_root_is_rejected_before_any_lease_is_created() {
        let workspace = tempfile::tempdir().unwrap();
        let drive_root = workspace.path().components().take(2).collect::<PathBuf>();
        let error =
            prepare_validation_process_state_root(workspace.path(), &drive_root).unwrap_err();
        assert!(
            error.to_string().contains("不能直接使用磁盘根"),
            "{error:#}"
        );
    }

    #[cfg(windows)]
    #[test]
    fn missing_windows_host_root_is_refused_without_path_based_bootstrap() {
        let workspace = tempfile::tempdir().unwrap();
        let container = tempfile::tempdir().unwrap();
        let candidate = container.path().join("missing-host-root");

        let error =
            prepare_validation_process_state_root(workspace.path(), &candidate).unwrap_err();
        assert!(format!("{error:#}").contains("预先创建"), "{error:#}");
        assert!(
            !candidate.exists(),
            "Windows host-root bootstrap must not create an unbound path"
        );
    }

    #[test]
    fn root_must_be_disjoint_from_the_workspace() {
        let workspace = tempfile::tempdir().unwrap();
        let candidate = workspace.path().join("host-temp");
        let error =
            prepare_validation_process_state_root(workspace.path(), &candidate).unwrap_err();
        assert!(error.to_string().contains("互不包含"), "{error:#}");
        assert!(
            !candidate.exists(),
            "overlap rejection must not create a directory in the workspace"
        );
    }

    #[test]
    fn workspace_marker_ancestor_is_rejected_before_candidate_creation() {
        for marker_name in [".RaymanCodingSkill", ".git"] {
            let workspace = tempfile::tempdir().unwrap();
            let container = tempfile::tempdir().unwrap();
            let marker = container.path().join(marker_name);
            if marker_name == ".RaymanCodingSkill" {
                fs::create_dir(&marker).unwrap();
            } else {
                fs::write(&marker, b"gitdir: elsewhere").unwrap();
            }
            let candidate = container.path().join("host-temp");

            let error =
                prepare_validation_process_state_root(workspace.path(), &candidate).unwrap_err();
            assert!(error.to_string().contains("工作区标记"), "{error:#}");
            assert!(
                !candidate.exists(),
                "marker rejection must not create {}",
                candidate.display()
            );
        }
    }

    #[cfg(not(windows))]
    #[test]
    fn non_windows_execution_is_an_exact_environment_noop() {
        let workspace = tempfile::tempdir().unwrap();
        let output = run_with_validation_process_lease(workspace.path(), "noop", |environment| {
            assert!(environment.is_none());
            Ok(output(0))
        })
        .unwrap();
        assert!(output.status.success());
        assert!(!workspace.path().join(".RaymanCodingSkill").exists());
    }

    #[cfg(windows)]
    #[test]
    fn session_releases_after_success_nonzero_and_spawn_failure() {
        let host = tempfile::tempdir().unwrap();
        for result in [
            Ok(output(0)),
            Ok(output(37)),
            Err(anyhow::anyhow!("spawn failed")),
        ] {
            let mut guard = ValidationProcessLeaseGuard {
                state_root: bind_validation_process_state_root(host.path().to_path_buf()).unwrap(),
                lease: Some(create_validation_process_lease(host.path(), "session").unwrap()),
            };
            let root = PathBuf::from(&guard.lease.as_ref().unwrap().lease.root);
            let result = guard.finish_with(result);
            assert!(!root.exists());
            if let Err(error) = result {
                assert!(format!("{error:#}").contains("spawn failed"));
            }
        }
    }

    #[cfg(windows)]
    #[test]
    fn cleanup_failure_blocks_success_and_preserves_both_failure_causes() {
        for result in [
            Ok(output(0)),
            Ok(output(37)),
            Err(anyhow::anyhow!("spawn failed")),
        ] {
            let host = tempfile::tempdir().unwrap();
            let lease = create_validation_process_lease(host.path(), "cleanup-failure").unwrap();
            let lease_root = PathBuf::from(&lease.lease.root);
            crate::file_io::write_json(
                &lease_root.join(LEASE_MANIFEST),
                &serde_json::json!({"tampered": true}),
            )
            .unwrap();
            let mut guard = ValidationProcessLeaseGuard {
                state_root: bind_validation_process_state_root(host.path().to_path_buf()).unwrap(),
                lease: Some(lease),
            };

            let error = guard.finish_with(result).unwrap_err();
            let rendered = format!("{error:#}");
            assert!(
                rendered.contains("释放") && rendered.contains("失败"),
                "{rendered}"
            );
            if rendered.contains("非零退出") {
                assert!(rendered.contains("exit=37"), "{rendered}");
            }
            if rendered.contains("启动失败") {
                assert!(rendered.contains("spawn failed"), "{rendered}");
            }
            assert!(
                lease_root.is_dir(),
                "tampered lease must remain fail closed"
            );
        }
    }
}
