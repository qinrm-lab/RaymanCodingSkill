//! 精简版托管临时目录：运行期临时文件放工作区本地 `.RaymanCodingSkill/tmp/`，
//! 不用系统临时目录、不记忆跨项目位置。提供创建、递归审计与清理；清理只删自己管辖的目录。

use std::collections::BTreeMap;
#[cfg(windows)]
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(windows)]
use anyhow::bail;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[cfg(windows)]
use crate::file_io::FileIdentity;
use crate::file_io::is_link_or_reparse;
use crate::pathfmt::display_path;
use crate::state_paths;

mod validation_process;

#[cfg(windows)]
pub(crate) use validation_process::WindowsExternalLeaseIdentity;
#[cfg(all(test, windows))]
pub(crate) use validation_process::validation_process_state_root;
pub(crate) use validation_process::{
    ValidationProcessStateRoot, acquire_validation_process_state_root,
    run_with_validation_process_lease,
};

const TEMP_ROOT: &str = ".RaymanCodingSkill/tmp";
const MAX_REPORTED_ERRORS: usize = 64;
const LEASES_RELATIVE: &str = "tmp/leases";
// External validation leases are internal, manifest-owned storage. Keep their
// physical components short because pytest and nested Cargo invocations append
// their own deep output paths below them on Windows.
const EXTERNAL_PYTEST_LEASES_RELATIVE: &str = "p";
const LEASE_MANIFEST: &str = "lease.json";
const CARGO_TARGET_LEASES_RELATIVE: &str = "tmp/c";
const CARGO_TARGET_LEASE_MANIFEST: &str = "lease.json";
const CARGO_TARGET_DIR: &str = "t";
static LEASE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub fn temp_root(root: &Path) -> PathBuf {
    root.join(TEMP_ROOT)
}

/// 在托管临时根下创建（若不存在）一个具名子目录并返回其路径。
pub fn scratch_dir(root: &Path, label: &str) -> Result<PathBuf> {
    let safe = safe_label(label);
    state_paths::managed_state_dir(root, &Path::new("tmp").join(&safe), true)?
        .ok_or_else(|| anyhow::anyhow!("无法创建受管临时目录"))
}

fn safe_label(label: &str) -> String {
    let mut safe: String = label
        .chars()
        .map(|ch| {
            if ch.is_control() || matches!(ch, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*')
            {
                '_'
            } else {
                ch
            }
        })
        .collect();
    // Apply Windows' trailing-dot/space rule everywhere so checkpoints remain portable.
    while safe.ends_with(' ') || safe.ends_with('.') {
        safe.pop();
        safe.push('_');
    }
    // Windows 保留设备名（con/nul/aux/com1…）作目录名非法，会报一条与输入无关的 OS 错误。
    if is_windows_reserved_name(&safe) {
        safe.push('_');
    }
    if safe.is_empty() {
        "scratch".into()
    } else {
        safe
    }
}

fn safe_lease_id_label(label: &str) -> String {
    let safe = safe_label(label)
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>();
    let safe = safe.trim_matches(['-', '.']).to_string();
    if safe.is_empty() {
        "pytest".into()
    } else {
        safe
    }
}

fn is_windows_reserved_name(name: &str) -> bool {
    // Device names remain reserved with an extension (for example NUL.txt).
    let lower = name
        .trim_end_matches([' ', '.'])
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    matches!(lower.as_str(), "con" | "prn" | "aux" | "nul")
        || (lower.len() == 4
            && (lower.starts_with("com") || lower.starts_with("lpt"))
            && lower.as_bytes()[3].is_ascii_digit())
}

/// A source-excluded pytest run root. Operator leases stay workspace-local;
/// Windows validation leases use a dedicated external root. The manifest makes
/// ownership explicit so cleanup never accepts an arbitrary caller path.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PytestLease {
    pub schema: String,
    pub id: String,
    pub label: String,
    pub created_at: String,
    pub root: String,
    pub basetemp: String,
    pub cache_dir: String,
    pub temp_dir: String,
    pub pycache_dir: String,
    pub nested_validation_root: String,
    pub environment: BTreeMap<String, String>,
    pub pytest_args: Vec<String>,
}

/// One validation-owned pytest lease plus the Windows directory object that
/// was opened immediately after the leaf was created. The held handle is not
/// serialized: a child can copy `lease.json`, but it cannot make a replacement
/// directory acquire the strong identity of this still-open object.
pub(crate) struct ManagedPytestLease {
    lease: PytestLease,
    #[cfg(windows)]
    directory_guards: WindowsPytestLeaseBinding,
    #[cfg(windows)]
    storage: PytestLeaseStorage,
}

impl ManagedPytestLease {
    pub(crate) fn lease(&self) -> &PytestLease {
        &self.lease
    }

    pub(crate) fn verify_current(&self) -> Result<()> {
        #[cfg(windows)]
        match &self.directory_guards {
            WindowsPytestLeaseBinding::Direct(guards) => {
                guards.verify_current(&self.lease, self.storage)?;
            }
            WindowsPytestLeaseBinding::External(_) => {
                bail!("pytest external lease 必须通过持有 validation host-temp 根复验")
            }
        }
        Ok(())
    }

    pub(crate) fn verify_current_at_host_root(
        &self,
        host_root: &ValidationProcessStateRoot,
    ) -> Result<()> {
        #[cfg(not(windows))]
        let _ = host_root;
        #[cfg(windows)]
        match &self.directory_guards {
            WindowsPytestLeaseBinding::Direct(guards) => {
                host_root.verify_current()?;
                guards.verify_current(&self.lease, self.storage)?;
                host_root.verify_current()?;
            }
            WindowsPytestLeaseBinding::External(guards) => {
                guards.verify_current_at_host_root(host_root, &self.lease, self.storage)?;
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum PytestLeaseStorage {
    Workspace,
    External,
}

#[derive(Clone, Copy)]
struct PytestLeaseLayout {
    basetemp: &'static str,
    cache_dir: &'static str,
    temp_dir: &'static str,
    pycache_dir: &'static str,
    nested_validation_root: &'static str,
}

const WORKSPACE_PYTEST_LEASE_LAYOUT: PytestLeaseLayout = PytestLeaseLayout {
    basetemp: "basetemp",
    cache_dir: "cache",
    temp_dir: "temp",
    pycache_dir: "pycache",
    nested_validation_root: "nested-validation",
};

const EXTERNAL_PYTEST_LEASE_LAYOUT: PytestLeaseLayout = PytestLeaseLayout {
    basetemp: "b",
    cache_dir: "c",
    temp_dir: "t",
    pycache_dir: "y",
    nested_validation_root: "n",
};

fn pytest_lease_layout(storage: PytestLeaseStorage) -> PytestLeaseLayout {
    match storage {
        // Keep the operator-facing workspace lease layout stable. Only the
        // Windows-only external validation storage needs the compact aliases.
        PytestLeaseStorage::Workspace => WORKSPACE_PYTEST_LEASE_LAYOUT,
        PytestLeaseStorage::External => EXTERNAL_PYTEST_LEASE_LAYOUT,
    }
}

#[cfg(windows)]
struct WindowsPytestLeaseGuards {
    root: state_paths::WindowsDirectoryObjectGuard,
    basetemp: state_paths::WindowsDirectoryChildIdentity,
    cache_dir: state_paths::WindowsDirectoryChildIdentity,
    temp_dir: state_paths::WindowsDirectoryChildIdentity,
    pycache_dir: state_paths::WindowsDirectoryChildIdentity,
    nested_validation_root: state_paths::WindowsDirectoryChildIdentity,
    manifest: WindowsLeaseManifestSeal,
}

#[cfg(windows)]
struct WindowsExternalPytestLeaseGuards {
    binding: WindowsExternalLeaseIdentity,
    basetemp: state_paths::WindowsDirectoryChildIdentity,
    cache_dir: state_paths::WindowsDirectoryChildIdentity,
    temp_dir: state_paths::WindowsDirectoryChildIdentity,
    pycache_dir: state_paths::WindowsDirectoryChildIdentity,
    nested_validation_root: state_paths::WindowsDirectoryChildIdentity,
    manifest: WindowsLeaseManifestSeal,
}

#[cfg(windows)]
#[derive(Clone, Copy)]
struct WindowsPytestLeaseFields<'a> {
    basetemp: &'a state_paths::WindowsDirectoryChildIdentity,
    cache_dir: &'a state_paths::WindowsDirectoryChildIdentity,
    temp_dir: &'a state_paths::WindowsDirectoryChildIdentity,
    pycache_dir: &'a state_paths::WindowsDirectoryChildIdentity,
    nested_validation_root: &'a state_paths::WindowsDirectoryChildIdentity,
    manifest: &'a WindowsLeaseManifestSeal,
}

#[cfg(windows)]
enum WindowsPytestLeaseBinding {
    Direct(WindowsPytestLeaseGuards),
    External(WindowsExternalPytestLeaseGuards),
}

#[cfg(windows)]
struct WindowsPytestLeaseCreation {
    lease: PytestLease,
    basetemp: state_paths::WindowsDirectoryChildIdentity,
    cache_dir: state_paths::WindowsDirectoryChildIdentity,
    temp_dir: state_paths::WindowsDirectoryChildIdentity,
    pycache_dir: state_paths::WindowsDirectoryChildIdentity,
    nested_validation_root: state_paths::WindowsDirectoryChildIdentity,
    manifest: WindowsLeaseManifestSeal,
}

#[cfg(windows)]
impl WindowsPytestLeaseGuards {
    fn verify_namespace_fields(
        root: &state_paths::WindowsDirectoryObjectGuard,
        basetemp: &state_paths::WindowsDirectoryChildIdentity,
        cache_dir: &state_paths::WindowsDirectoryChildIdentity,
        temp_dir: &state_paths::WindowsDirectoryChildIdentity,
        pycache_dir: &state_paths::WindowsDirectoryChildIdentity,
        nested_validation_root: &state_paths::WindowsDirectoryChildIdentity,
        layout: PytestLeaseLayout,
    ) -> Result<()> {
        state_paths::verify_windows_directory_object_at_path(
            root,
            root.path(),
            "pytest lease 创建目录",
        )?;
        root.verify_direct_child(
            basetemp,
            OsStr::new(layout.basetemp),
            "pytest lease basetemp 子目录",
        )?;
        root.verify_direct_child(
            cache_dir,
            OsStr::new(layout.cache_dir),
            "pytest lease cache 子目录",
        )?;
        root.verify_direct_child(
            temp_dir,
            OsStr::new(layout.temp_dir),
            "pytest lease TEMP 子目录",
        )?;
        root.verify_direct_child(
            pycache_dir,
            OsStr::new(layout.pycache_dir),
            "pytest lease pycache 子目录",
        )?;
        root.verify_direct_child(
            nested_validation_root,
            OsStr::new(layout.nested_validation_root),
            "pytest lease 嵌套验证子目录",
        )?;
        state_paths::verify_windows_directory_object_at_path(
            root,
            root.path(),
            "pytest lease 创建目录",
        )
    }

    fn verify_current_fields(
        root: &state_paths::WindowsDirectoryObjectGuard,
        fields: WindowsPytestLeaseFields<'_>,
        lease: &PytestLease,
        storage: PytestLeaseStorage,
    ) -> Result<()> {
        let layout = pytest_lease_layout(storage);
        Self::verify_namespace_fields(
            root,
            fields.basetemp,
            fields.cache_dir,
            fields.temp_dir,
            fields.pycache_dir,
            fields.nested_validation_root,
            layout,
        )?;
        for (child, name, label) in [
            (
                fields.basetemp,
                layout.basetemp,
                "pytest lease basetemp 子目录",
            ),
            (
                fields.cache_dir,
                layout.cache_dir,
                "pytest lease cache 子目录",
            ),
            (fields.temp_dir, layout.temp_dir, "pytest lease TEMP 子目录"),
            (
                fields.pycache_dir,
                layout.pycache_dir,
                "pytest lease pycache 子目录",
            ),
            (
                fields.nested_validation_root,
                layout.nested_validation_root,
                "pytest lease 嵌套验证子目录",
            ),
        ] {
            root.probe_direct_child(child, OsStr::new(name), label)?;
        }
        let bytes = verify_windows_lease_manifest(
            root,
            fields.manifest,
            LEASE_MANIFEST,
            "pytest lease manifest",
        )?;
        let actual = parse_pytest_lease_manifest(&lease.id, root.path(), storage, &bytes)?;
        if actual != *lease {
            anyhow::bail!(
                "pytest lease manifest 与 validation session 不一致: {}",
                lease.id
            );
        }
        Self::verify_namespace_fields(
            root,
            fields.basetemp,
            fields.cache_dir,
            fields.temp_dir,
            fields.pycache_dir,
            fields.nested_validation_root,
            layout,
        )
    }

    fn fields(&self) -> WindowsPytestLeaseFields<'_> {
        WindowsPytestLeaseFields {
            basetemp: &self.basetemp,
            cache_dir: &self.cache_dir,
            temp_dir: &self.temp_dir,
            pycache_dir: &self.pycache_dir,
            nested_validation_root: &self.nested_validation_root,
            manifest: &self.manifest,
        }
    }

    fn verify_current(&self, lease: &PytestLease, storage: PytestLeaseStorage) -> Result<()> {
        Self::verify_current_fields(&self.root, self.fields(), lease, storage)
    }
}

#[cfg(windows)]
impl WindowsExternalPytestLeaseGuards {
    fn fields(&self) -> WindowsPytestLeaseFields<'_> {
        WindowsPytestLeaseFields {
            basetemp: &self.basetemp,
            cache_dir: &self.cache_dir,
            temp_dir: &self.temp_dir,
            pycache_dir: &self.pycache_dir,
            nested_validation_root: &self.nested_validation_root,
            manifest: &self.manifest,
        }
    }

    fn verify_current_at_host_root(
        &self,
        host_root: &ValidationProcessStateRoot,
        lease: &PytestLease,
        storage: PytestLeaseStorage,
    ) -> Result<()> {
        host_root.with_external_lease_root(
            &self.binding,
            EXTERNAL_PYTEST_LEASES_RELATIVE,
            &lease.id,
            "pytest lease 创建目录",
            |root| {
                WindowsPytestLeaseGuards::verify_current_fields(root, self.fields(), lease, storage)
            },
        )
    }
}

#[cfg(windows)]
#[derive(Debug)]
pub(super) struct WindowsLeaseManifestSeal {
    bytes: Vec<u8>,
    identity: FileIdentity,
}

#[cfg(windows)]
pub(super) fn create_windows_lease_manifest<T: Serialize>(
    root: &state_paths::WindowsDirectoryObjectGuard,
    name: &str,
    label: &str,
    value: &T,
) -> Result<WindowsLeaseManifestSeal> {
    let text = serde_json::to_string_pretty(value).context("无法序列化 JSON")?;
    let bytes = text.into_bytes();
    let identity = root.create_file_exclusive_and_seal(OsStr::new(name), &bytes, label)?;
    Ok(WindowsLeaseManifestSeal { bytes, identity })
}

#[cfg(windows)]
pub(super) fn verify_windows_lease_manifest(
    root: &state_paths::WindowsDirectoryObjectGuard,
    expected: &WindowsLeaseManifestSeal,
    name: &str,
    label: &str,
) -> Result<Vec<u8>> {
    let path = root.path().join(name);
    let (bytes, identity) = root.read_file_with_identity(OsStr::new(name), label)?;
    if bytes != expected.bytes || identity != expected.identity {
        anyhow::bail!("{label}在创建后发生身份或内容变化: {}", display_path(&path));
    }
    Ok(bytes)
}

/// Verify a sealed manifest from a leaf handle supplied by a handle-relative
/// cleanup traversal. External host-root leases retain only directory identity
/// tokens, so cleanup cannot depend on a long-lived lease-root handle.
#[cfg(windows)]
pub(super) fn verify_windows_lease_manifest_from_directory_handle(
    directory: &std::fs::File,
    logical_directory: &Path,
    expected: &WindowsLeaseManifestSeal,
    name: &str,
    label: &str,
) -> Result<Vec<u8>> {
    let path = logical_directory.join(name);
    let (bytes, identity) = state_paths::read_windows_file_from_directory_handle_with_identity(
        directory,
        OsStr::new(name),
        logical_directory,
        label,
    )?;
    if bytes != expected.bytes || identity != expected.identity {
        anyhow::bail!("{label}在创建后发生身份或内容变化: {}", display_path(&path));
    }
    Ok(bytes)
}

#[cfg(windows)]
pub(super) fn windows_creation_collision(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|source| {
                source.kind() == std::io::ErrorKind::AlreadyExists
                    || matches!(source.raw_os_error(), Some(80 | 183))
            })
    })
}

fn lease_relative(id: &str) -> Result<PathBuf> {
    // 纯点 id 会塌缩：`Path::components()` 把非首位 `.` 归一化掉，
    // `tmp/leases/.` 解析成 leases 根本身，release 的 remove_dir_all 会
    // 删光所有兄弟 lease；下游 normal_components 守卫看到的是归一化之后
    // 的路径，永远拦不住它。Windows 还会剥掉尾部 `.`（`a.` → `a`），
    // 形成跨 lease 别名。两类都必须在 id 门口拒绝。
    if !is_valid_lease_id(id) {
        anyhow::bail!("无效 pytest lease id: {id}");
    }
    Ok(Path::new(LEASES_RELATIVE).join(id))
}

fn pytest_lease_relative(id: &str, storage: PytestLeaseStorage) -> Result<PathBuf> {
    let workspace_relative = lease_relative(id)?;
    Ok(match storage {
        PytestLeaseStorage::Workspace => workspace_relative,
        PytestLeaseStorage::External => Path::new(EXTERNAL_PYTEST_LEASES_RELATIVE).join(id),
    })
}

fn is_valid_lease_id(id: &str) -> bool {
    !(id.is_empty()
        || id.len() > 160
        || !id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
        || !id.chars().any(|ch| ch.is_ascii_alphanumeric())
        || id.ends_with('.'))
}

fn lease_path(root: &Path, id: &str, create: bool) -> Result<Option<PathBuf>> {
    state_paths::managed_state_dir(root, &lease_relative(id)?, create)
}

fn pytest_lease_path(
    root: &Path,
    id: &str,
    create: bool,
    storage: PytestLeaseStorage,
) -> Result<Option<PathBuf>> {
    match storage {
        PytestLeaseStorage::Workspace => lease_path(root, id, create),
        PytestLeaseStorage::External => {
            state_paths::managed_external_dir(root, &pytest_lease_relative(id, storage)?, create)
        }
    }
}

#[cfg(not(windows))]
fn remove_pytest_lease_dir(root: &Path, id: &str, storage: PytestLeaseStorage) -> Result<bool> {
    let relative = pytest_lease_relative(id, storage)?;
    match storage {
        PytestLeaseStorage::Workspace => state_paths::remove_managed_state_dir_all(root, &relative),
        PytestLeaseStorage::External => {
            state_paths::remove_managed_external_dir_all(root, &relative)
        }
    }
}

#[cfg(any(not(windows), test))]
fn create_exclusive_pytest_lease_leaf(parent: &Path, id: &str) -> Result<Option<PathBuf>> {
    let lease_root = parent.join(id);
    match fs::create_dir(&lease_root) {
        Ok(()) => Ok(Some(lease_root)),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(None),
        Err(error) => Err(error)
            .with_context(|| format!("无法独占创建 pytest lease: {}", display_path(&lease_root))),
    }
}

#[cfg(windows)]
fn remove_pytest_lease_dir_windows_verified_with_snapshot<F, G>(
    root: &Path,
    id: &str,
    storage: PytestLeaseStorage,
    verifier: F,
    snapshot_verifier: G,
) -> Result<bool>
where
    F: FnOnce(&fs::File, &Path) -> Result<()>,
    G: FnOnce(&Path, &fs::File) -> Result<()>,
{
    let relative = pytest_lease_relative(id, storage)?;
    match storage {
        PytestLeaseStorage::Workspace => {
            state_paths::remove_managed_state_dir_all_windows_verified_with_snapshot(
                root,
                &relative,
                verifier,
                snapshot_verifier,
            )
        }
        PytestLeaseStorage::External => {
            state_paths::remove_managed_external_dir_all_windows_verified_with_snapshot(
                root,
                &relative,
                verifier,
                snapshot_verifier,
            )
        }
    }
}

#[cfg(windows)]
fn remove_pytest_lease_dir_windows_verified<F>(
    root: &Path,
    id: &str,
    storage: PytestLeaseStorage,
    verifier: F,
) -> Result<bool>
where
    F: FnOnce(&fs::File, &Path) -> Result<()>,
{
    remove_pytest_lease_dir_windows_verified_with_snapshot(root, id, storage, verifier, |_, _| {
        Ok(())
    })
}

fn path_text(path: &Path) -> String {
    // Strip the Windows `\\?\` verbatim prefix that canonicalize() always emits: these
    // strings are handed to pytest as --basetemp/cache_dir, exported as TMP/TEMP/TMPDIR/
    // PYTHONPYCACHEPREFIX, and printed by `temp scratch`. Verbatim-prefixed paths disable
    // normalization in downstream tools and are surprising in output. The self-comparison in
    // verify_pytest_lease normalizes both sides through this same function.
    display_path(path)
}

/// Create a unique pytest lease and prove every directory can be traversed,
/// written, read and cleaned by the current execution token.
pub fn create_pytest_lease(root: &Path, label: &str) -> Result<PytestLease> {
    Ok(create_pytest_lease_with_probe_storage(
        root,
        label,
        probe_directory,
        PytestLeaseStorage::Workspace,
    )?
    .lease)
}

pub(crate) fn create_managed_pytest_lease(root: &Path, label: &str) -> Result<ManagedPytestLease> {
    create_pytest_lease_with_probe_storage(
        root,
        label,
        probe_directory,
        PytestLeaseStorage::Workspace,
    )
}

pub(crate) fn create_external_managed_pytest_lease(
    root: &Path,
    label: &str,
) -> Result<ManagedPytestLease> {
    create_pytest_lease_with_probe_storage(
        root,
        label,
        probe_directory,
        PytestLeaseStorage::External,
    )
}

/// Create the production Windows external pytest lease from the held
/// validation host root. The path-rooted external constructor remains for
/// non-Windows and local fixtures; a Windows validation child must not reopen
/// its authority root by pathname after that root has been bound.
pub(crate) fn create_external_managed_pytest_lease_at_host_root(
    host_root: &ValidationProcessStateRoot,
    label: &str,
) -> Result<ManagedPytestLease> {
    #[cfg(windows)]
    {
        create_external_managed_pytest_lease_with_host_root(host_root, label)
    }

    #[cfg(not(windows))]
    {
        create_external_managed_pytest_lease(host_root.path(), label)
    }
}

#[cfg(windows)]
fn create_external_managed_pytest_lease_with_host_root(
    host_root: &ValidationProcessStateRoot,
    label: &str,
) -> Result<ManagedPytestLease> {
    host_root.verify_current()?;
    let label = label.trim();
    let storage = PytestLeaseStorage::External;
    let layout = pytest_lease_layout(storage);
    for _ in 0..64 {
        let sequence = LEASE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let timestamp = crate::timefmt::now_iso()
            .chars()
            .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
            .collect::<String>();
        let id = format!(
            "{}-{}-{}-{}",
            safe_lease_id_label(label),
            timestamp,
            std::process::id(),
            sequence
        );
        let binding = match host_root.create_external_lease_identity(
            EXTERNAL_PYTEST_LEASES_RELATIVE,
            &id,
            "pytest lease 创建目录",
        ) {
            Ok(binding) => binding,
            Err(error) if windows_creation_collision(&error) => continue,
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "无法通过持有 validation host-temp 根独占创建 pytest lease: {}",
                        display_path(
                            &host_root
                                .path()
                                .join(EXTERNAL_PYTEST_LEASES_RELATIVE)
                                .join(&id)
                        )
                    )
                });
            }
        };
        let relative = pytest_lease_relative(&id, storage)?;
        let creation = host_root.with_external_lease_root(
            &binding,
            EXTERNAL_PYTEST_LEASES_RELATIVE,
            &id,
            "pytest lease 创建目录",
            |root_guard| {
                let lease_root = root_guard.path().to_path_buf();
                let basetemp_guard = root_guard.create_child_exclusive(
                    OsStr::new(layout.basetemp),
                    "pytest lease basetemp 子目录",
                )?;
                let cache_dir_guard = root_guard.create_child_exclusive(
                    OsStr::new(layout.cache_dir),
                    "pytest lease cache 子目录",
                )?;
                let temp_dir_guard = root_guard.create_child_exclusive(
                    OsStr::new(layout.temp_dir),
                    "pytest lease TEMP 子目录",
                )?;
                let pycache_dir_guard = root_guard.create_child_exclusive(
                    OsStr::new(layout.pycache_dir),
                    "pytest lease pycache 子目录",
                )?;
                let nested_validation_root_guard = root_guard.create_child_exclusive(
                    OsStr::new(layout.nested_validation_root),
                    "pytest lease 嵌套验证子目录",
                )?;
                let basetemp = basetemp_guard.path().to_path_buf();
                let cache_dir = cache_dir_guard.path().to_path_buf();
                let temp_dir = temp_dir_guard.path().to_path_buf();
                let pycache_dir = pycache_dir_guard.path().to_path_buf();
                let nested_validation_root = nested_validation_root_guard.path().to_path_buf();
                for (child, name, child_label) in [
                    (
                        &basetemp_guard,
                        layout.basetemp,
                        "pytest lease basetemp 子目录",
                    ),
                    (
                        &cache_dir_guard,
                        layout.cache_dir,
                        "pytest lease cache 子目录",
                    ),
                    (&temp_dir_guard, layout.temp_dir, "pytest lease TEMP 子目录"),
                    (
                        &pycache_dir_guard,
                        layout.pycache_dir,
                        "pytest lease pycache 子目录",
                    ),
                    (
                        &nested_validation_root_guard,
                        layout.nested_validation_root,
                        "pytest lease 嵌套验证子目录",
                    ),
                ] {
                    root_guard.probe_direct_child(child, OsStr::new(name), child_label)?;
                }
                let environment =
                    lease_environment(&temp_dir, &pycache_dir, &nested_validation_root, storage);
                let lease = PytestLease {
                    schema: "rayman.pytest-lease.v1".into(),
                    id: id.clone(),
                    label: label.to_string(),
                    created_at: crate::timefmt::now_iso(),
                    root: path_text(&lease_root),
                    basetemp: path_text(&basetemp),
                    cache_dir: path_text(&cache_dir),
                    temp_dir: path_text(&temp_dir),
                    pycache_dir: path_text(&pycache_dir),
                    nested_validation_root: path_text(&nested_validation_root),
                    environment,
                    pytest_args: vec![
                        "--basetemp".into(),
                        path_text(&basetemp),
                        "-o".into(),
                        format!("cache_dir={}", path_text(&cache_dir)),
                    ],
                };
                let manifest = create_windows_lease_manifest(
                    root_guard,
                    LEASE_MANIFEST,
                    "pytest lease manifest",
                    &lease,
                )?;
                WindowsPytestLeaseGuards::verify_current_fields(
                    root_guard,
                    WindowsPytestLeaseFields {
                        basetemp: &basetemp_guard,
                        cache_dir: &cache_dir_guard,
                        temp_dir: &temp_dir_guard,
                        pycache_dir: &pycache_dir_guard,
                        nested_validation_root: &nested_validation_root_guard,
                        manifest: &manifest,
                    },
                    &lease,
                    storage,
                )?;
                Ok(WindowsPytestLeaseCreation {
                    lease,
                    basetemp: basetemp_guard,
                    cache_dir: cache_dir_guard,
                    temp_dir: temp_dir_guard,
                    pycache_dir: pycache_dir_guard,
                    nested_validation_root: nested_validation_root_guard,
                    manifest,
                })
            },
        );
        return match creation {
            Ok(created) => Ok(ManagedPytestLease {
                lease: created.lease,
                directory_guards: WindowsPytestLeaseBinding::External(
                    WindowsExternalPytestLeaseGuards {
                        binding,
                        basetemp: created.basetemp,
                        cache_dir: created.cache_dir,
                        temp_dir: created.temp_dir,
                        pycache_dir: created.pycache_dir,
                        nested_validation_root: created.nested_validation_root,
                        manifest: created.manifest,
                    },
                ),
                storage,
            }),
            Err(creation_error) => {
                let cleanup = host_root.remove_external_lease_tree(
                    &relative,
                    |leaf, lease_root| {
                        state_paths::verify_windows_directory_child_identity_from_open(
                            binding.root_identity(),
                            leaf,
                            lease_root,
                            "pytest lease 创建目录",
                        )?;
                        host_root.with_external_lease_root(
                            &binding,
                            EXTERNAL_PYTEST_LEASES_RELATIVE,
                            &id,
                            "pytest lease 创建目录",
                            |_| Ok(()),
                        )?;
                        host_root.verify_current()
                    },
                    |snapshot_path, snapshot_leaf| {
                        state_paths::verify_windows_directory_child_identity_from_open(
                            binding.root_identity(),
                            snapshot_leaf,
                            snapshot_path,
                            "pytest lease 创建目录",
                        )?;
                        host_root.with_external_lease_root(
                            &binding,
                            EXTERNAL_PYTEST_LEASES_RELATIVE,
                            &id,
                            "pytest lease 创建目录",
                            |_| Ok(()),
                        )?;
                        host_root.verify_current()
                    },
                );
                match cleanup {
                    Ok(true) => Err(creation_error),
                    Ok(false) => {
                        Err(creation_error).context("pytest lease 创建失败后受管目录已消失")
                    }
                    Err(cleanup_error) => Err(creation_error).context(format!(
                        "pytest lease 创建或探测失败后的清理也失败: {cleanup_error:#}"
                    )),
                }
            }
        };
    }
    bail!("无法独占创建 pytest lease（连续名称冲突）")
}
#[cfg(test)]
fn create_pytest_lease_with_probe<F>(root: &Path, label: &str, mut probe: F) -> Result<PytestLease>
where
    F: FnMut(&Path) -> Result<()>,
{
    Ok(create_pytest_lease_with_probe_storage(
        root,
        label,
        &mut probe,
        PytestLeaseStorage::Workspace,
    )?
    .lease)
}

fn create_pytest_lease_with_probe_storage<F>(
    root: &Path,
    label: &str,
    #[allow(unused_variables, unused_mut)] mut probe: F,
    storage: PytestLeaseStorage,
) -> Result<ManagedPytestLease>
where
    F: FnMut(&Path) -> Result<()>,
{
    #[cfg(windows)]
    let _ = &mut probe;
    // Build the id from the same text the manifest stores. Deriving it from the
    // raw argument while storing `label.trim()` made the id/label binding check
    // reject leases this very function had just created, whenever trimming
    // changed the sanitized value — create then failed its own verify and left
    // an orphan no `pytest-release` could name.
    let label = label.trim();
    let parent_relative = match storage {
        PytestLeaseStorage::Workspace => Path::new(LEASES_RELATIVE),
        PytestLeaseStorage::External => Path::new(EXTERNAL_PYTEST_LEASES_RELATIVE),
    };
    let parent = match storage {
        PytestLeaseStorage::Workspace => {
            state_paths::managed_state_dir(root, parent_relative, true)?
        }
        PytestLeaseStorage::External => {
            state_paths::managed_external_dir(root, parent_relative, true)?
        }
    }
    .ok_or_else(|| anyhow::anyhow!("无法创建 pytest lease 根"))?;

    for _ in 0..64 {
        let sequence = LEASE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let timestamp = crate::timefmt::now_iso()
            .chars()
            .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
            .collect::<String>();
        let id = format!(
            "{}-{}-{}-{}",
            safe_lease_id_label(label),
            timestamp,
            std::process::id(),
            sequence
        );
        #[cfg(windows)]
        {
            let root_guard = match state_paths::create_windows_directory_object_exclusive(
                &parent,
                OsStr::new(&id),
                "pytest lease 创建目录",
            ) {
                Ok(root) => root,
                Err(error) if windows_creation_collision(&error) => continue,
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!(
                            "无法独占创建 pytest lease: {}",
                            display_path(&parent.join(&id))
                        )
                    });
                }
            };
            let creation = (|| -> Result<WindowsPytestLeaseCreation> {
                let lease_root = root_guard.path().to_path_buf();
                let layout = pytest_lease_layout(storage);
                let basetemp_guard = root_guard.create_child_exclusive(
                    OsStr::new(layout.basetemp),
                    "pytest lease basetemp 子目录",
                )?;
                let cache_dir_guard = root_guard.create_child_exclusive(
                    OsStr::new(layout.cache_dir),
                    "pytest lease cache 子目录",
                )?;
                let temp_dir_guard = root_guard.create_child_exclusive(
                    OsStr::new(layout.temp_dir),
                    "pytest lease TEMP 子目录",
                )?;
                let pycache_dir_guard = root_guard.create_child_exclusive(
                    OsStr::new(layout.pycache_dir),
                    "pytest lease pycache 子目录",
                )?;
                let nested_validation_root_guard = root_guard.create_child_exclusive(
                    OsStr::new(layout.nested_validation_root),
                    "pytest lease 嵌套验证子目录",
                )?;
                let basetemp = basetemp_guard.path().to_path_buf();
                let cache_dir = cache_dir_guard.path().to_path_buf();
                let temp_dir = temp_dir_guard.path().to_path_buf();
                let pycache_dir = pycache_dir_guard.path().to_path_buf();
                let nested_validation_root = nested_validation_root_guard.path().to_path_buf();
                for (child, name, label) in [
                    (
                        &basetemp_guard,
                        layout.basetemp,
                        "pytest lease basetemp 子目录",
                    ),
                    (
                        &cache_dir_guard,
                        layout.cache_dir,
                        "pytest lease cache 子目录",
                    ),
                    (&temp_dir_guard, layout.temp_dir, "pytest lease TEMP 子目录"),
                    (
                        &pycache_dir_guard,
                        layout.pycache_dir,
                        "pytest lease pycache 子目录",
                    ),
                    (
                        &nested_validation_root_guard,
                        layout.nested_validation_root,
                        "pytest lease 嵌套验证子目录",
                    ),
                ] {
                    root_guard.probe_direct_child(child, OsStr::new(name), label)?;
                    #[cfg(test)]
                    probe(child.path())?;
                }
                let environment =
                    lease_environment(&temp_dir, &pycache_dir, &nested_validation_root, storage);
                let lease = PytestLease {
                    schema: "rayman.pytest-lease.v1".into(),
                    id: id.clone(),
                    label: label.to_string(),
                    created_at: crate::timefmt::now_iso(),
                    root: path_text(&lease_root),
                    basetemp: path_text(&basetemp),
                    cache_dir: path_text(&cache_dir),
                    temp_dir: path_text(&temp_dir),
                    pycache_dir: path_text(&pycache_dir),
                    nested_validation_root: path_text(&nested_validation_root),
                    environment,
                    pytest_args: vec![
                        "--basetemp".into(),
                        path_text(&basetemp),
                        "-o".into(),
                        format!("cache_dir={}", path_text(&cache_dir)),
                    ],
                };
                let manifest = create_windows_lease_manifest(
                    &root_guard,
                    LEASE_MANIFEST,
                    "pytest lease manifest",
                    &lease,
                )?;
                WindowsPytestLeaseGuards::verify_current_fields(
                    &root_guard,
                    WindowsPytestLeaseFields {
                        basetemp: &basetemp_guard,
                        cache_dir: &cache_dir_guard,
                        temp_dir: &temp_dir_guard,
                        pycache_dir: &pycache_dir_guard,
                        nested_validation_root: &nested_validation_root_guard,
                        manifest: &manifest,
                    },
                    &lease,
                    storage,
                )?;
                Ok(WindowsPytestLeaseCreation {
                    lease,
                    basetemp: basetemp_guard,
                    cache_dir: cache_dir_guard,
                    temp_dir: temp_dir_guard,
                    pycache_dir: pycache_dir_guard,
                    nested_validation_root: nested_validation_root_guard,
                    manifest,
                })
            })();
            return match creation {
                Ok(created) => Ok(ManagedPytestLease {
                    lease: created.lease,
                    directory_guards: WindowsPytestLeaseBinding::Direct(WindowsPytestLeaseGuards {
                        root: root_guard,
                        basetemp: created.basetemp,
                        cache_dir: created.cache_dir,
                        temp_dir: created.temp_dir,
                        pycache_dir: created.pycache_dir,
                        nested_validation_root: created.nested_validation_root,
                        manifest: created.manifest,
                    }),
                    storage,
                }),
                Err(creation_error) => {
                    let cleanup = remove_pytest_lease_dir_windows_verified(
                        root,
                        &id,
                        storage,
                        |leaf, current_root| {
                            state_paths::verify_windows_directory_object(
                                &root_guard,
                                leaf,
                                current_root,
                                "pytest lease 创建目录",
                            )
                        },
                    );
                    match cleanup {
                        Ok(true) => Err(creation_error),
                        Ok(false) => {
                            Err(creation_error).context("pytest lease 创建失败后受管目录已消失")
                        }
                        Err(cleanup_error) => Err(creation_error).context(format!(
                            "pytest lease 创建或探测失败后的清理也失败: {cleanup_error}"
                        )),
                    }
                }
            };
        }

        #[cfg(not(windows))]
        {
            let Some(_lease_root) = create_exclusive_pytest_lease_leaf(&parent, &id)? else {
                continue;
            };
            let creation = (|| {
                let lease_root = pytest_lease_path(root, &id, false, storage)?
                    .ok_or_else(|| anyhow::anyhow!("pytest lease 创建后消失: {id}"))?;
                let layout = pytest_lease_layout(storage);
                let basetemp = lease_root.join(layout.basetemp);
                let cache_dir = lease_root.join(layout.cache_dir);
                let temp_dir = lease_root.join(layout.temp_dir);
                let pycache_dir = lease_root.join(layout.pycache_dir);
                let nested_validation_root = lease_root.join(layout.nested_validation_root);
                for path in [
                    &basetemp,
                    &cache_dir,
                    &temp_dir,
                    &pycache_dir,
                    &nested_validation_root,
                ] {
                    fs::create_dir(path).with_context(|| {
                        format!("无法创建 pytest lease 子目录: {}", display_path(path))
                    })?;
                    ensure_real_directory(path)?;
                    probe(path)?;
                }
                let environment =
                    lease_environment(&temp_dir, &pycache_dir, &nested_validation_root, storage);
                let lease = PytestLease {
                    schema: "rayman.pytest-lease.v1".into(),
                    id: id.clone(),
                    label: label.to_string(),
                    created_at: crate::timefmt::now_iso(),
                    root: path_text(&lease_root),
                    basetemp: path_text(&basetemp),
                    cache_dir: path_text(&cache_dir),
                    temp_dir: path_text(&temp_dir),
                    pycache_dir: path_text(&pycache_dir),
                    nested_validation_root: path_text(&nested_validation_root),
                    environment,
                    pytest_args: vec![
                        "--basetemp".into(),
                        path_text(&basetemp),
                        "-o".into(),
                        format!("cache_dir={}", path_text(&cache_dir)),
                    ],
                };
                crate::file_io::write_json(&lease_root.join(LEASE_MANIFEST), &lease)?;
                verify_pytest_lease_with_storage(root, &id, storage)?;
                Ok(lease)
            })();
            return match creation {
                Ok(lease) => Ok(ManagedPytestLease { lease }),
                Err(creation_error) => match remove_pytest_lease_dir(root, &id, storage) {
                    Ok(true) => Err(creation_error),
                    Ok(false) => {
                        Err(creation_error).context("pytest lease 创建失败后受管目录已消失")
                    }
                    Err(cleanup_error) => Err(creation_error).context(format!(
                        "pytest lease 创建或探测失败后的清理也失败: {cleanup_error}"
                    )),
                },
            };
        }
    }

    anyhow::bail!("无法独占创建 pytest lease（连续名称冲突）")
}

/// 受管 lease 的**完整** environment。create 与 verify 共用同一份构造，
/// 二者才不会各自演进出不同口径。
fn lease_environment(
    temp_dir: &Path,
    pycache_dir: &Path,
    nested_validation_root: &Path,
    storage: PytestLeaseStorage,
) -> BTreeMap<String, String> {
    let mut environment = BTreeMap::new();
    environment.insert("TMP".into(), path_text(temp_dir));
    environment.insert("TEMP".into(), path_text(temp_dir));
    environment.insert("TMPDIR".into(), path_text(temp_dir));
    environment.insert("PYTHONPYCACHEPREFIX".into(), path_text(pycache_dir));
    environment.insert("PYTHONDONTWRITEBYTECODE".into(), "1".into());
    // On Windows, only the external validation lease is marker-free. A
    // workspace-local operator lease lives below `.RaymanCodingSkill`, so
    // publishing it as a nested validation authority would correctly be
    // rejected by validation_process_state_root. Non-Windows execution does
    // not consume this variable, but retaining it keeps managed pytest's
    // recursive contract consistent there.
    if !cfg!(windows) || matches!(storage, PytestLeaseStorage::External) {
        environment.insert(
            "RAYMAN_VALIDATION_TEMP_ROOT".into(),
            path_text(nested_validation_root),
        );
    }
    environment
}

fn probe_directory(path: &Path) -> Result<()> {
    let sequence = LEASE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let probe = path.join(format!(
        ".rayman-probe-{}-{sequence}.tmp",
        std::process::id()
    ));
    fs::write(&probe, b"rayman-temp-probe")
        .with_context(|| format!("受管临时目录写探针失败: {}", display_path(&probe)))?;
    let bytes = fs::read(&probe)
        .with_context(|| format!("受管临时目录读探针失败: {}", display_path(&probe)))?;
    if bytes != b"rayman-temp-probe" {
        anyhow::bail!("受管临时目录探针内容不一致: {}", display_path(&probe));
    }
    fs::remove_file(&probe)
        .with_context(|| format!("受管临时目录清理探针失败: {}", display_path(&probe)))?;
    Ok(())
}

/// Does `id` have the exact shape `create_pytest_lease` produces for this
/// sanitized label — `{label}-{timestamp}-{pid}-{sequence}`?
///
/// The label cannot simply be split off: the timestamp is `now_iso()` with every
/// non-alphanumeric byte mapped to `-`, so it contains dashes of its own and the
/// field count is not fixed. What is pinned instead is everything that does not
/// depend on the label: the boundary must fall on a `-`, the timestamp must
/// begin with the ISO year digit, and the last two fields must be the pure-digit
/// pid and sequence. A bare `starts_with` accepted any label that sanitized to a
/// *prefix* of the real one; this rejects those unless the truncation happens to
/// leave a digit at the boundary and a digit-only tail, which the generator's own
/// alphabet makes a far narrower opening.
fn lease_id_matches_label(id: &str, sanitized_label: &str) -> bool {
    let Some(rest) = id.strip_prefix(sanitized_label) else {
        return false;
    };
    let Some(rest) = rest.strip_prefix('-') else {
        return false;
    };
    if !rest.starts_with(|ch: char| ch.is_ascii_digit()) {
        return false;
    }
    let mut tail = rest.rsplitn(3, '-');
    let (Some(sequence), Some(pid), Some(timestamp)) = (tail.next(), tail.next(), tail.next())
    else {
        return false;
    };
    !timestamp.is_empty()
        && !pid.is_empty()
        && !sequence.is_empty()
        && pid.chars().all(|ch| ch.is_ascii_digit())
        && sequence.chars().all(|ch| ch.is_ascii_digit())
}

fn load_pytest_lease_identity_with_storage(
    root: &Path,
    id: &str,
    storage: PytestLeaseStorage,
) -> Result<(PathBuf, PytestLease)> {
    let lease_root = pytest_lease_path(root, id, false, storage)?
        .ok_or_else(|| anyhow::anyhow!("pytest lease 不存在: {id}"))?;
    let lease = crate::file_io::read_json::<PytestLease>(&lease_root.join(LEASE_MANIFEST))?
        .ok_or_else(|| anyhow::anyhow!("pytest lease 缺少 manifest: {id}"))?;
    validate_pytest_lease_manifest(id, &lease_root, storage, &lease)?;
    ensure_real_directory(&lease_root)?;
    Ok((lease_root, lease))
}

fn validate_pytest_lease_manifest(
    id: &str,
    lease_root: &Path,
    storage: PytestLeaseStorage,
    lease: &PytestLease,
) -> Result<()> {
    let layout = pytest_lease_layout(storage);
    let basetemp = lease_root.join(layout.basetemp);
    let cache_dir = lease_root.join(layout.cache_dir);
    let temp_dir = lease_root.join(layout.temp_dir);
    let pycache_dir = lease_root.join(layout.pycache_dir);
    let nested_validation_root = lease_root.join(layout.nested_validation_root);
    if lease.schema != "rayman.pytest-lease.v1"
        || lease.id != id
        // label 是 id 的前缀来源；不比对就等于 manifest 里有一段无人校验的
        // 自由文本，改写它能让 probe 发布出与 lease 身份不符的标签。
        // 光用 `starts_with` 是前缀测试：任何能 sanitize 成真实 label 前缀的
        // 篡改值都会照样通过。这里改为校验 id 的完整生成形态（见函数注释）。
        || !lease_id_matches_label(id, &safe_lease_id_label(&lease.label))
        || lease.root != path_text(lease_root)
        || lease.basetemp != path_text(&basetemp)
        || lease.cache_dir != path_text(&cache_dir)
        || lease.temp_dir != path_text(&temp_dir)
        || lease.pycache_dir != path_text(&pycache_dir)
        || lease.nested_validation_root != path_text(&nested_validation_root)
        // environment 必须整体相等，不能只逐键查五个已知项：逐键查是子集比较，
        // 注入的额外变量（PYTHONPATH 等）能穿过这道 manifest 门禁，再被
        // `temp pytest-probe` 当作"已核验"原样发布出去。README 承诺的是
        // "publish exact argv/environment"，那就必须逐字节相等。
        || lease.environment
            != lease_environment(
                &temp_dir,
                &pycache_dir,
                &nested_validation_root,
                storage,
            )
        || lease.pytest_args
            != vec![
                "--basetemp".to_string(),
                path_text(&basetemp),
                "-o".to_string(),
                format!("cache_dir={}", path_text(&cache_dir)),
            ]
    {
        anyhow::bail!("pytest lease manifest 与受管路径不一致: {id}");
    }
    Ok(())
}

#[cfg(windows)]
fn parse_pytest_lease_manifest(
    id: &str,
    lease_root: &Path,
    storage: PytestLeaseStorage,
    bytes: &[u8],
) -> Result<PytestLease> {
    let lease = serde_json::from_slice::<PytestLease>(bytes)
        .with_context(|| format!("pytest lease manifest 无法解析: {id}"))?;
    validate_pytest_lease_manifest(id, lease_root, storage, &lease)?;
    Ok(lease)
}

pub fn verify_pytest_lease(root: &Path, id: &str) -> Result<PytestLease> {
    verify_pytest_lease_with_storage(root, id, PytestLeaseStorage::Workspace)
}

fn verify_pytest_lease_with_storage(
    root: &Path,
    id: &str,
    storage: PytestLeaseStorage,
) -> Result<PytestLease> {
    let (_, lease) = load_pytest_lease_identity_with_storage(root, id, storage)?;
    let basetemp = PathBuf::from(&lease.basetemp);
    let cache_dir = PathBuf::from(&lease.cache_dir);
    let temp_dir = PathBuf::from(&lease.temp_dir);
    let pycache_dir = PathBuf::from(&lease.pycache_dir);
    let nested_validation_root = PathBuf::from(&lease.nested_validation_root);
    for path in [
        &basetemp,
        &cache_dir,
        &temp_dir,
        &pycache_dir,
        &nested_validation_root,
    ] {
        ensure_real_directory(path)?;
        probe_directory(path)?;
    }
    Ok(lease)
}

/// Remove exactly one manifest-owned lease selected by its operator-visible
/// id. Arbitrary paths are never accepted and a corrupt manifest fails closed.
/// On Windows the manifest is read from the same held leaf handle that
/// authorizes handle-bound deletion.
pub fn release_pytest_lease(root: &Path, id: &str) -> Result<()> {
    #[cfg(windows)]
    {
        let removed = release_pytest_lease_by_id_windows(root, id)?;
        if !removed {
            anyhow::bail!("pytest lease 在已验证释放前消失: {id}");
        }
        Ok(())
    }

    #[cfg(not(windows))]
    {
        let (lease_root, _) =
            load_pytest_lease_identity_with_storage(root, id, PytestLeaseStorage::Workspace)?;
        let removed = remove_pytest_lease_dir(root, id, PytestLeaseStorage::Workspace)
            .with_context(|| format!("无法释放 pytest lease: {}", display_path(&lease_root)))?;
        if !removed {
            anyhow::bail!("pytest lease 在已验证释放前消失: {id}");
        }
        Ok(())
    }
}

#[cfg(windows)]
fn release_pytest_lease_by_id_windows(root: &Path, id: &str) -> Result<bool> {
    remove_pytest_lease_dir_windows_verified(
        root,
        id,
        PytestLeaseStorage::Workspace,
        |leaf, lease_root| {
            let bytes = state_paths::read_windows_file_from_directory_handle(
                leaf,
                std::ffi::OsStr::new(LEASE_MANIFEST),
                lease_root,
                "pytest lease manifest",
            )?;
            parse_pytest_lease_manifest(id, lease_root, PytestLeaseStorage::Workspace, &bytes)?;
            Ok(())
        },
    )
    .with_context(|| format!("无法释放 pytest lease: {id}"))
}

/// Release the exact lease created for one managed pytest process. The full
/// in-memory identity must match the manifest read from the deletion leaf, and
/// a missing lease is an error so validation cannot mint a receipt after lost
/// cleanup authority.
pub(crate) fn release_managed_pytest_lease(
    root: &Path,
    expected: &ManagedPytestLease,
) -> Result<()> {
    release_managed_pytest_lease_with_storage(root, expected, PytestLeaseStorage::Workspace)
}

pub(crate) fn release_external_managed_pytest_lease(
    root: &Path,
    expected: &ManagedPytestLease,
) -> Result<()> {
    release_managed_pytest_lease_with_storage(root, expected, PytestLeaseStorage::External)
}

/// Release an external pytest lease through the host-root object used to
/// create it. Windows cleanup never starts a second path-based traversal of
/// that root, so a root replacement leaves both trees fail-closed.
pub(crate) fn release_external_managed_pytest_lease_at_host_root(
    host_root: &ValidationProcessStateRoot,
    expected: &ManagedPytestLease,
) -> Result<()> {
    #[cfg(windows)]
    {
        host_root.verify_current()?;
        expected.verify_current_at_host_root(host_root)?;
        let manifest = &expected.lease;
        let relative = pytest_lease_relative(&manifest.id, PytestLeaseStorage::External)?;
        let removed = host_root
            .remove_external_lease_tree(
                &relative,
                |leaf, lease_root| {
                    let guards = match &expected.directory_guards {
                        WindowsPytestLeaseBinding::External(guards) => guards,
                        WindowsPytestLeaseBinding::Direct(_) => {
                            bail!("pytest external lease 释放缺少外部 host-root 身份绑定")
                        }
                    };
                    state_paths::verify_windows_directory_child_identity_from_open(
                        guards.binding.root_identity(),
                        leaf,
                        lease_root,
                        "pytest lease 创建目录",
                    )?;
                    expected.verify_current_at_host_root(host_root)?;
                    let bytes = verify_windows_lease_manifest_from_directory_handle(
                        leaf,
                        lease_root,
                        &guards.manifest,
                        LEASE_MANIFEST,
                        "pytest lease manifest",
                    )?;
                    let actual = parse_pytest_lease_manifest(
                        &manifest.id,
                        lease_root,
                        PytestLeaseStorage::External,
                        &bytes,
                    )?;
                    if actual != *manifest {
                        anyhow::bail!(
                            "pytest lease 释放身份与 validation session 不一致: {}",
                            manifest.id
                        );
                    }
                    expected.verify_current_at_host_root(host_root)?;
                    host_root.verify_current()
                },
                |snapshot_path, snapshot_leaf| {
                    let guards = match &expected.directory_guards {
                        WindowsPytestLeaseBinding::External(guards) => guards,
                        WindowsPytestLeaseBinding::Direct(_) => {
                            bail!("pytest external lease 释放缺少外部 host-root 身份绑定")
                        }
                    };
                    state_paths::verify_windows_directory_child_identity_from_open(
                        guards.binding.root_identity(),
                        snapshot_leaf,
                        snapshot_path,
                        "pytest lease 创建目录",
                    )?;
                    expected.verify_current_at_host_root(host_root)?;
                    host_root.verify_current()
                },
            )
            .with_context(|| {
                format!(
                    "无法释放 pytest lease: {}",
                    display_path(Path::new(&manifest.root))
                )
            })?;
        if !removed {
            anyhow::bail!("pytest lease 在已验证释放前消失: {}", manifest.id);
        }
        host_root.verify_current()?;
        Ok(())
    }

    #[cfg(not(windows))]
    {
        release_external_managed_pytest_lease(host_root.path(), expected)
    }
}

fn release_managed_pytest_lease_with_storage(
    root: &Path,
    expected: &ManagedPytestLease,
    storage: PytestLeaseStorage,
) -> Result<()> {
    #[cfg(windows)]
    {
        release_managed_pytest_lease_windows(root, expected, storage)
    }

    #[cfg(not(windows))]
    {
        let expected = &expected.lease;
        let (lease_root, actual) =
            load_pytest_lease_identity_with_storage(root, &expected.id, storage)?;
        if actual != *expected {
            anyhow::bail!(
                "pytest lease 释放身份与 validation session 不一致: {}",
                expected.id
            );
        }
        let removed = remove_pytest_lease_dir(root, &expected.id, storage)
            .with_context(|| format!("无法释放 pytest lease: {}", display_path(&lease_root)))?;
        if !removed {
            anyhow::bail!("pytest lease 在已验证释放前消失: {}", expected.id);
        }
        Ok(())
    }
}

#[cfg(windows)]
fn release_managed_pytest_lease_windows(
    root: &Path,
    expected: &ManagedPytestLease,
    storage: PytestLeaseStorage,
) -> Result<()> {
    let guards = match &expected.directory_guards {
        WindowsPytestLeaseBinding::Direct(guards) => guards,
        WindowsPytestLeaseBinding::External(_) => {
            bail!("pytest external lease 必须通过持有 validation host-temp 根释放")
        }
    };
    expected.verify_current()?;
    let manifest = &expected.lease;
    let removed = remove_pytest_lease_dir_windows_verified_with_snapshot(
        root,
        &manifest.id,
        storage,
        |leaf, lease_root| {
            state_paths::verify_windows_directory_object(
                &guards.root,
                leaf,
                lease_root,
                "pytest lease 创建目录",
            )?;
            let bytes = state_paths::read_windows_file_from_directory_handle(
                leaf,
                std::ffi::OsStr::new(LEASE_MANIFEST),
                lease_root,
                "pytest lease manifest",
            )?;
            let actual = parse_pytest_lease_manifest(&manifest.id, lease_root, storage, &bytes)?;
            if actual != *manifest {
                anyhow::bail!(
                    "pytest lease 释放身份与 validation session 不一致: {}",
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
                "pytest lease 创建目录",
            )?;
            expected.verify_current()
        },
    )
    .with_context(|| {
        format!(
            "无法释放 pytest lease: {}",
            display_path(Path::new(&manifest.root))
        )
    })?;
    if !removed {
        anyhow::bail!("pytest lease 在已验证释放前消失: {}", manifest.id);
    }
    Ok(())
}

/// A unique Cargo target owned by one outer validation execution. Keeping the
/// manifest beside, rather than inside, `target` prevents `cargo clean` from
/// deleting the ownership record that makes release safe.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CargoTargetLease {
    pub schema: String,
    pub id: String,
    pub label: String,
    pub created_at: String,
    pub root: String,
    pub target_dir: String,
}

pub(crate) struct ManagedCargoTargetLease {
    lease: CargoTargetLease,
    #[cfg(windows)]
    directory_guards: WindowsCargoTargetLeaseGuards,
}

impl ManagedCargoTargetLease {
    pub(crate) fn lease(&self) -> &CargoTargetLease {
        &self.lease
    }

    pub(crate) fn verify_current(&self) -> Result<()> {
        #[cfg(windows)]
        self.directory_guards.verify_current(&self.lease)?;
        Ok(())
    }
}

#[cfg(windows)]
struct WindowsCargoTargetLeaseGuards {
    root: state_paths::WindowsDirectoryObjectGuard,
    target: state_paths::WindowsDirectoryChildIdentity,
    manifest: WindowsLeaseManifestSeal,
}

#[cfg(windows)]
struct WindowsCargoTargetLeaseCreation {
    lease: CargoTargetLease,
    target: state_paths::WindowsDirectoryChildIdentity,
    manifest: WindowsLeaseManifestSeal,
}

#[cfg(windows)]
impl WindowsCargoTargetLeaseGuards {
    fn verify_current_fields(
        root: &state_paths::WindowsDirectoryObjectGuard,
        target: &state_paths::WindowsDirectoryChildIdentity,
        manifest: &WindowsLeaseManifestSeal,
        lease: &CargoTargetLease,
    ) -> Result<()> {
        state_paths::verify_windows_directory_object_at_path(
            root,
            root.path(),
            "Cargo target lease 创建目录",
        )?;
        root.verify_direct_child(
            target,
            OsStr::new(CARGO_TARGET_DIR),
            "Cargo target lease target 子目录",
        )?;
        root.probe_direct_child(
            target,
            OsStr::new(CARGO_TARGET_DIR),
            "Cargo target lease target 子目录",
        )?;
        let bytes = verify_windows_lease_manifest(
            root,
            manifest,
            CARGO_TARGET_LEASE_MANIFEST,
            "Cargo target lease manifest",
        )?;
        let actual = parse_cargo_target_lease_manifest(&lease.id, root.path(), &bytes)?;
        if actual != *lease {
            anyhow::bail!(
                "Cargo target lease manifest 与 validation session 不一致: {}",
                lease.id
            );
        }
        root.verify_direct_child(
            target,
            OsStr::new(CARGO_TARGET_DIR),
            "Cargo target lease target 子目录",
        )?;
        state_paths::verify_windows_directory_object_at_path(
            root,
            root.path(),
            "Cargo target lease 创建目录",
        )
    }

    fn verify_current(&self, lease: &CargoTargetLease) -> Result<()> {
        Self::verify_current_fields(&self.root, &self.target, &self.manifest, lease)
    }
}

fn cargo_target_lease_relative(id: &str) -> Result<PathBuf> {
    if !is_valid_lease_id(id) {
        anyhow::bail!("无效 Cargo target lease id: {id}");
    }
    Ok(Path::new(CARGO_TARGET_LEASES_RELATIVE).join(id))
}

fn cargo_target_lease_path(root: &Path, id: &str, create: bool) -> Result<Option<PathBuf>> {
    state_paths::managed_state_dir(root, &cargo_target_lease_relative(id)?, create)
}

/// Create and probe a manifest-owned Cargo target. The leaf lease directory is
/// created exclusively so a stale or pre-planted directory is never adopted
/// and later removed as if this process owned it.
pub fn create_cargo_target_lease(root: &Path, label: &str) -> Result<CargoTargetLease> {
    Ok(
        create_managed_cargo_target_lease_with_probe(root, label, probe_cargo_target_directory)?
            .lease,
    )
}

pub(crate) fn create_managed_cargo_target_lease(
    root: &Path,
    label: &str,
) -> Result<ManagedCargoTargetLease> {
    create_managed_cargo_target_lease_with_probe(root, label, probe_cargo_target_directory)
}

#[cfg(test)]
fn create_cargo_target_lease_with_probe<F>(
    root: &Path,
    label: &str,
    mut probe: F,
) -> Result<CargoTargetLease>
where
    F: FnMut(&Path) -> Result<()>,
{
    Ok(create_managed_cargo_target_lease_with_probe(root, label, &mut probe)?.lease)
}

fn create_managed_cargo_target_lease_with_probe<F>(
    root: &Path,
    label: &str,
    #[allow(unused_variables, unused_mut)] mut probe: F,
) -> Result<ManagedCargoTargetLease>
where
    F: FnMut(&Path) -> Result<()>,
{
    #[cfg(windows)]
    let _ = &mut probe;
    let parent =
        state_paths::managed_state_dir(root, Path::new(CARGO_TARGET_LEASES_RELATIVE), true)?
            .ok_or_else(|| anyhow::anyhow!("无法创建 Cargo target lease 根"))?;
    let label = label.trim();

    for _ in 0..32 {
        let sequence = LEASE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let timestamp = crate::timefmt::now_iso()
            .chars()
            .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
            .collect::<String>();
        let id = format!(
            "{}-{}-{}-{}",
            safe_lease_id_label(label),
            timestamp,
            std::process::id(),
            sequence
        );
        #[cfg(windows)]
        {
            let root_guard = match state_paths::create_windows_directory_object_exclusive(
                &parent,
                OsStr::new(&id),
                "Cargo target lease 创建目录",
            ) {
                Ok(root) => root,
                Err(error) if windows_creation_collision(&error) => continue,
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!(
                            "无法独占创建 Cargo target lease: {}",
                            display_path(&parent.join(&id))
                        )
                    });
                }
            };
            let creation = (|| -> Result<WindowsCargoTargetLeaseCreation> {
                let lease_root = root_guard.path().to_path_buf();
                let target_guard = root_guard.create_child_exclusive(
                    OsStr::new(CARGO_TARGET_DIR),
                    "Cargo target lease target 子目录",
                )?;
                let target_dir = target_guard.path().to_path_buf();
                root_guard.probe_direct_child(
                    &target_guard,
                    OsStr::new(CARGO_TARGET_DIR),
                    "Cargo target lease target 子目录",
                )?;
                #[cfg(test)]
                probe(target_guard.path())?;
                let lease = CargoTargetLease {
                    schema: "rayman.cargo-target-lease.v1".into(),
                    id: id.clone(),
                    label: label.to_string(),
                    created_at: crate::timefmt::now_iso(),
                    root: path_text(&lease_root),
                    target_dir: path_text(&target_dir),
                };
                let manifest = create_windows_lease_manifest(
                    &root_guard,
                    CARGO_TARGET_LEASE_MANIFEST,
                    "Cargo target lease manifest",
                    &lease,
                )?;
                WindowsCargoTargetLeaseGuards::verify_current_fields(
                    &root_guard,
                    &target_guard,
                    &manifest,
                    &lease,
                )?;
                Ok(WindowsCargoTargetLeaseCreation {
                    lease,
                    target: target_guard,
                    manifest,
                })
            })();
            return match creation {
                Ok(created) => Ok(ManagedCargoTargetLease {
                    lease: created.lease,
                    directory_guards: WindowsCargoTargetLeaseGuards {
                        root: root_guard,
                        target: created.target,
                        manifest: created.manifest,
                    },
                }),
                Err(creation_error) => {
                    let relative = cargo_target_lease_relative(&id)?;
                    let cleanup =
                        state_paths::remove_managed_state_dir_all_windows_verified_with_snapshot(
                            root,
                            &relative,
                            |leaf, current_root| {
                                state_paths::verify_windows_directory_object(
                                    &root_guard,
                                    leaf,
                                    current_root,
                                    "Cargo target lease 创建目录",
                                )
                            },
                            |snapshot_path, snapshot_leaf| {
                                state_paths::verify_windows_directory_object(
                                    &root_guard,
                                    snapshot_leaf,
                                    snapshot_path,
                                    "Cargo target lease 创建目录",
                                )
                            },
                        );
                    match cleanup {
                        Ok(true) => Err(creation_error),
                        Ok(false) => Err(creation_error)
                            .context("Cargo target lease 创建失败后受管目录已消失"),
                        Err(cleanup_error) => Err(creation_error).context(format!(
                            "Cargo target lease 创建或探测失败后的清理也失败: {cleanup_error}"
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
                            "无法独占创建 Cargo target lease: {}",
                            display_path(&lease_root)
                        )
                    });
                }
            }
            let relative = cargo_target_lease_relative(&id)?;
            let creation = (|| {
                let lease_root = state_paths::managed_state_dir(root, &relative, false)?
                    .ok_or_else(|| anyhow::anyhow!("Cargo target lease 创建后消失: {id}"))?;
                let target_dir = lease_root.join(CARGO_TARGET_DIR);
                fs::create_dir(&target_dir).with_context(|| {
                    format!(
                        "无法创建 Cargo target lease 子目录: {}",
                        display_path(&target_dir)
                    )
                })?;
                let target_dir =
                    state_paths::managed_state_dir(root, &relative.join(CARGO_TARGET_DIR), false)?
                        .ok_or_else(|| {
                            anyhow::anyhow!("Cargo target lease 子目录创建后消失: {id}")
                        })?;
                probe(&target_dir)?;
                let lease = CargoTargetLease {
                    schema: "rayman.cargo-target-lease.v1".into(),
                    id: id.clone(),
                    label: label.to_string(),
                    created_at: crate::timefmt::now_iso(),
                    root: path_text(&lease_root),
                    target_dir: path_text(&target_dir),
                };
                crate::file_io::write_json(&lease_root.join(CARGO_TARGET_LEASE_MANIFEST), &lease)?;
                verify_cargo_target_lease(root, &id)
            })();
            return match creation {
                Ok(lease) => Ok(ManagedCargoTargetLease { lease }),
                Err(creation_error) => {
                    match state_paths::remove_managed_state_dir_all(root, &relative) {
                        Ok(true) => Err(creation_error),
                        Ok(false) => Err(creation_error)
                            .context("Cargo target lease 创建失败后受管目录已消失"),
                        Err(cleanup_error) => Err(creation_error).context(format!(
                            "Cargo target lease 创建或探测失败后的清理也失败: {cleanup_error}"
                        )),
                    }
                }
            };
        }
    }

    anyhow::bail!("无法独占创建 Cargo target lease（连续名称冲突）")
}

fn probe_cargo_target_directory(path: &Path) -> Result<()> {
    let sequence = LEASE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let probe = path.join(format!(
        ".rayman-cargo-target-probe-{}-{sequence}.tmp",
        std::process::id()
    ));
    fs::write(&probe, b"rayman-cargo-target-probe")
        .with_context(|| format!("Cargo target lease 写探针失败: {}", display_path(&probe)))?;
    let bytes = fs::read(&probe)
        .with_context(|| format!("Cargo target lease 读探针失败: {}", display_path(&probe)))?;
    if bytes != b"rayman-cargo-target-probe" {
        anyhow::bail!(
            "Cargo target lease 探针内容不一致: {}",
            display_path(&probe)
        );
    }
    fs::remove_file(&probe)
        .with_context(|| format!("Cargo target lease 清理探针失败: {}", display_path(&probe)))?;
    Ok(())
}

fn load_cargo_target_lease_identity(root: &Path, id: &str) -> Result<(PathBuf, CargoTargetLease)> {
    let lease_root = cargo_target_lease_path(root, id, false)?
        .ok_or_else(|| anyhow::anyhow!("Cargo target lease 不存在: {id}"))?;
    let lease = crate::file_io::read_json::<CargoTargetLease>(
        &lease_root.join(CARGO_TARGET_LEASE_MANIFEST),
    )?
    .ok_or_else(|| anyhow::anyhow!("Cargo target lease 缺少 manifest: {id}"))?;
    validate_cargo_target_lease_manifest(id, &lease_root, &lease)?;
    ensure_real_directory(&lease_root)?;
    Ok((lease_root, lease))
}

fn validate_cargo_target_lease_manifest(
    id: &str,
    lease_root: &Path,
    lease: &CargoTargetLease,
) -> Result<()> {
    let target_dir = lease_root.join(CARGO_TARGET_DIR);
    if lease.schema != "rayman.cargo-target-lease.v1"
        || lease.id != id
        || !lease_id_matches_label(id, &safe_lease_id_label(&lease.label))
        || lease.root != path_text(lease_root)
        || lease.target_dir != path_text(&target_dir)
    {
        anyhow::bail!("Cargo target lease manifest 与受管路径不一致: {id}");
    }
    Ok(())
}

#[cfg(windows)]
fn parse_cargo_target_lease_manifest(
    id: &str,
    lease_root: &Path,
    bytes: &[u8],
) -> Result<CargoTargetLease> {
    let lease = serde_json::from_slice::<CargoTargetLease>(bytes)
        .with_context(|| format!("Cargo target lease manifest 无法解析: {id}"))?;
    validate_cargo_target_lease_manifest(id, lease_root, &lease)?;
    Ok(lease)
}

/// Revalidate ownership, containment, directory identity and write capability
/// immediately before every child process spawn.
pub fn verify_cargo_target_lease(root: &Path, id: &str) -> Result<CargoTargetLease> {
    let (_, lease) = load_cargo_target_lease_identity(root, id)?;
    let target_dir = PathBuf::from(&lease.target_dir);
    ensure_real_directory(&target_dir)?;
    probe_cargo_target_directory(&target_dir)?;
    Ok(lease)
}

/// Release exactly one manifest-owned Cargo target. Runtime children below the
/// target may be absent; the verified lease root and manifest remain the delete
/// authority.
pub fn release_cargo_target_lease(root: &Path, expected: &CargoTargetLease) -> Result<()> {
    #[cfg(windows)]
    {
        release_cargo_target_lease_windows(root, expected, |_| Ok(()))
    }

    #[cfg(not(windows))]
    {
        let (lease_root, actual) = load_cargo_target_lease_identity(root, &expected.id)?;
        if actual != *expected {
            anyhow::bail!(
                "Cargo target lease 释放身份与 session 不一致: {}",
                expected.id
            );
        }
        let removed = state_paths::remove_managed_state_dir_all(
            root,
            &cargo_target_lease_relative(&expected.id)?,
        )
        .with_context(|| format!("无法释放 Cargo target lease: {}", display_path(&lease_root)))?;
        if !removed {
            anyhow::bail!("Cargo target lease 在已验证释放前消失: {}", expected.id);
        }
        Ok(())
    }
}

pub(crate) fn release_managed_cargo_target_lease(
    root: &Path,
    expected: &ManagedCargoTargetLease,
) -> Result<()> {
    #[cfg(windows)]
    {
        release_managed_cargo_target_lease_windows(root, expected)
    }

    #[cfg(not(windows))]
    {
        release_cargo_target_lease(root, &expected.lease)
    }
}

#[cfg(windows)]
fn release_cargo_target_lease_windows<F>(
    root: &Path,
    expected: &CargoTargetLease,
    after_verified: F,
) -> Result<()>
where
    F: FnOnce(&Path) -> Result<()>,
{
    release_cargo_target_lease_windows_guarded(root, expected, None, after_verified)
}

#[cfg(windows)]
fn release_cargo_target_lease_windows_guarded<F>(
    root: &Path,
    expected: &CargoTargetLease,
    directory_guard: Option<&state_paths::WindowsDirectoryObjectGuard>,
    after_verified: F,
) -> Result<()>
where
    F: FnOnce(&Path) -> Result<()>,
{
    let relative = cargo_target_lease_relative(&expected.id)?;
    let removed = state_paths::remove_managed_state_dir_all_windows_verified_with_snapshot(
        root,
        &relative,
        |leaf, lease_root| {
            if let Some(directory_guard) = directory_guard {
                state_paths::verify_windows_directory_object(
                    directory_guard,
                    leaf,
                    lease_root,
                    "Cargo target lease 创建目录",
                )?;
            }
            let bytes = state_paths::read_windows_file_from_directory_handle(
                leaf,
                std::ffi::OsStr::new(CARGO_TARGET_LEASE_MANIFEST),
                lease_root,
                "Cargo target lease manifest",
            )?;
            let actual = parse_cargo_target_lease_manifest(&expected.id, lease_root, &bytes)?;
            if actual != *expected {
                anyhow::bail!(
                    "Cargo target lease 释放身份与 session 不一致: {}",
                    expected.id
                );
            }
            after_verified(lease_root)
        },
        |snapshot_path, snapshot_leaf| {
            if let Some(directory_guard) = directory_guard {
                state_paths::verify_windows_directory_object(
                    directory_guard,
                    snapshot_leaf,
                    snapshot_path,
                    "Cargo target lease 创建目录",
                )?;
            }
            Ok(())
        },
    )
    .with_context(|| {
        format!(
            "无法释放 Cargo target lease: {}",
            display_path(Path::new(&expected.root))
        )
    })?;
    if !removed {
        anyhow::bail!("Cargo target lease 在已验证释放前消失: {}", expected.id);
    }
    Ok(())
}

#[cfg(windows)]
fn release_managed_cargo_target_lease_windows(
    root: &Path,
    expected: &ManagedCargoTargetLease,
) -> Result<()> {
    expected.verify_current()?;
    let manifest = &expected.lease;
    let relative = cargo_target_lease_relative(&manifest.id)?;
    let removed = state_paths::remove_managed_state_dir_all_windows_verified_with_snapshot(
        root,
        &relative,
        |leaf, lease_root| {
            state_paths::verify_windows_directory_object(
                &expected.directory_guards.root,
                leaf,
                lease_root,
                "Cargo target lease 创建目录",
            )?;
            let bytes = state_paths::read_windows_file_from_directory_handle(
                leaf,
                OsStr::new(CARGO_TARGET_LEASE_MANIFEST),
                lease_root,
                "Cargo target lease manifest",
            )?;
            let actual = parse_cargo_target_lease_manifest(&manifest.id, lease_root, &bytes)?;
            if actual != *manifest {
                anyhow::bail!(
                    "Cargo target lease 释放身份与 session 不一致: {}",
                    manifest.id
                );
            }
            expected.verify_current()
        },
        |snapshot_path, snapshot_leaf| {
            state_paths::verify_windows_directory_object(
                &expected.directory_guards.root,
                snapshot_leaf,
                snapshot_path,
                "Cargo target lease 创建目录",
            )?;
            expected.verify_current()
        },
    )
    .with_context(|| {
        format!(
            "无法释放 Cargo target lease: {}",
            display_path(Path::new(&manifest.root))
        )
    })?;
    if !removed {
        anyhow::bail!("Cargo target lease 在已验证释放前消失: {}", manifest.id);
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub struct TempStatus {
    pub root: String,
    pub exists: bool,
    /// 向后兼容：托管根下的直接条目数。
    pub entry_count: usize,
    /// 递归文件数（不含符号链接/reparse point）。
    pub file_count: usize,
    /// 递归子目录数（不含根目录）。
    pub directory_count: usize,
    pub total_bytes: u64,
    pub traversal_error_count: usize,
    pub traversal_errors: Vec<String>,
}

/// 递归审计托管临时目录。所有不可遍历或链接/reparse 条目都会被计数并报告，
/// 而不是被默默当作零字节或零文件。
pub fn audit(root: &Path) -> TempStatus {
    let display_root = temp_root(root);
    let mut status = TempStatus {
        root: display_path(&display_root),
        exists: false,
        entry_count: 0,
        file_count: 0,
        directory_count: 0,
        total_bytes: 0,
        traversal_error_count: 0,
        traversal_errors: Vec::new(),
    };
    let dir = match state_paths::managed_state_dir(root, Path::new("tmp"), false) {
        Ok(None) => return status,
        Ok(Some(dir)) => {
            status.exists = true;
            dir
        }
        Err(error) => {
            record_error(
                &mut status,
                format!("{}: {error:#}", display_path(&display_root)),
            );
            return status;
        }
    };
    if let Err(error) = collect_metrics(&dir, &mut status, true) {
        record_error(&mut status, format!("{}: {error}", display_path(&dir)));
    }
    status
}

/// 兼容旧调用；现在返回的结构同时携带递归指标。
pub fn status(root: &Path) -> TempStatus {
    audit(root)
}

fn collect_metrics(dir: &Path, status: &mut TempStatus, is_root: bool) -> Result<()> {
    ensure_real_directory(dir)?;
    let entries =
        fs::read_dir(dir).with_context(|| format!("无法读取临时目录: {}", dir.display()))?;
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                record_error(status, format!("{}: {error}", display_path(dir)));
                continue;
            }
        };
        if is_root {
            status.entry_count += 1;
        }
        let path = entry.path();
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) => {
                record_error(status, format!("{}: {error}", display_path(&path)));
                continue;
            }
        };
        if is_link_or_reparse(&metadata) {
            record_error(
                status,
                format!("拒绝遍历链接/reparse 临时条目: {}", display_path(&path)),
            );
            continue;
        }
        if metadata.file_type().is_dir() {
            status.directory_count += 1;
            if let Err(error) = collect_metrics(&path, status, false) {
                record_error(status, format!("{}: {error}", display_path(&path)));
            }
        } else if metadata.file_type().is_file() {
            status.file_count += 1;
            status.total_bytes = status.total_bytes.saturating_add(metadata.len());
        } else {
            record_error(
                status,
                format!("不支持的临时条目类型: {}", display_path(&path)),
            );
        }
    }
    Ok(())
}

fn record_error(status: &mut TempStatus, error: String) {
    status.traversal_error_count += 1;
    if status.traversal_errors.len() < MAX_REPORTED_ERRORS {
        status.traversal_errors.push(error);
    }
}

fn ensure_real_directory(path: &Path) -> Result<()> {
    crate::file_io::ensure_real_directory_labeled(path, "临时目录")
}

/// 清理整个托管临时根。只删 `.RaymanCodingSkill/tmp`，绝不触碰其它用户数据。
pub fn cleanup(root: &Path) -> Result<bool> {
    let Some(dir) = state_paths::managed_state_dir(root, Path::new("tmp"), false)? else {
        return Ok(false);
    };
    fs::remove_dir_all(&dir).with_context(|| format!("无法清理临时目录: {}", dir.display()))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scratch_label_avoids_windows_reserved_names() {
        let dir = tempfile::tempdir().unwrap();
        let scratch = scratch_dir(dir.path(), "nul").unwrap();
        assert_ne!(scratch.file_name().unwrap(), "nul");
        assert!(scratch.is_dir());
    }

    #[test]
    fn scratch_label_preserves_safe_unicode_and_sanitizes_path_syntax() {
        let dir = tempfile::tempdir().unwrap();
        let unicode = scratch_dir(dir.path(), "中文-資料-🙂").unwrap();
        assert_eq!(unicode.file_name().unwrap(), "中文-資料-🙂");

        let unsafe_label = scratch_dir(dir.path(), "../危险\\路径:*?").unwrap();
        let name = unsafe_label.file_name().unwrap().to_string_lossy();
        assert!(!name.contains('/') && !name.contains('\\'));
        assert!(!name.contains(':') && !name.contains('*') && !name.contains('?'));
    }

    #[test]
    fn scratch_label_rejects_reserved_device_stems_with_extensions() {
        let dir = tempfile::tempdir().unwrap();
        let scratch = scratch_dir(dir.path(), "NUL.txt").unwrap();
        assert_ne!(scratch.file_name().unwrap(), "NUL.txt");
        assert!(scratch.is_dir());
    }

    #[test]
    fn scratch_create_status_and_cleanup() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let scratch = scratch_dir(root, "build cache").unwrap();
        fs::create_dir_all(scratch.join("nested")).unwrap();
        fs::write(scratch.join("f.txt"), "x").unwrap();
        fs::write(scratch.join("nested/g.txt"), "xyz").unwrap();
        let report = status(root);
        assert!(report.exists);
        assert_eq!(report.entry_count, 1);
        assert_eq!(report.file_count, 2);
        assert_eq!(report.directory_count, 2, "scratch + nested");
        assert_eq!(report.total_bytes, 4);
        assert_eq!(report.traversal_error_count, 0);
        assert!(cleanup(root).unwrap());
        assert!(!status(root).exists);
    }

    #[test]
    fn pytest_lease_is_unique_probed_and_manifest_owned() {
        let dir = tempfile::tempdir().unwrap();
        let first = create_pytest_lease(dir.path(), "focused tests").unwrap();
        let second = create_pytest_lease(dir.path(), "focused tests").unwrap();
        assert_ne!(first.id, second.id);
        assert_eq!(first.schema, "rayman.pytest-lease.v1");
        assert!(Path::new(&first.basetemp).is_dir());
        assert_eq!(verify_pytest_lease(dir.path(), &first.id).unwrap(), first);
        release_pytest_lease(dir.path(), &first.id).unwrap();
        assert!(verify_pytest_lease(dir.path(), &first.id).is_err());
        assert!(verify_pytest_lease(dir.path(), "../outside").is_err());
        assert!(Path::new(&second.root).is_dir());
    }

    #[test]
    fn external_pytest_lease_uses_a_marker_free_sibling_tree() {
        let external = tempfile::tempdir().unwrap();
        let managed = create_external_managed_pytest_lease(external.path(), "validation").unwrap();
        let lease = managed.lease();
        let lease_root = PathBuf::from(&lease.root);

        assert!(
            lease_root
                .canonicalize()
                .unwrap()
                .starts_with(external.path().join("p").canonicalize().unwrap())
        );
        let layout = pytest_lease_layout(PytestLeaseStorage::External);
        assert_eq!(
            PathBuf::from(&lease.nested_validation_root),
            lease_root.join(layout.nested_validation_root)
        );
        assert_eq!(
            lease.environment.get("RAYMAN_VALIDATION_TEMP_ROOT"),
            Some(&lease.nested_validation_root)
        );
        assert_ne!(
            lease.environment.get("TEMP"),
            lease.environment.get("RAYMAN_VALIDATION_TEMP_ROOT")
        );
        assert!(!external.path().join(".RaymanCodingSkill").exists());
        assert_eq!(
            verify_pytest_lease_with_storage(
                external.path(),
                &lease.id,
                PytestLeaseStorage::External,
            )
            .unwrap(),
            *lease
        );

        release_external_managed_pytest_lease(external.path(), &managed).unwrap();
        assert!(!lease_root.exists());
        assert!(!external.path().join(".RaymanCodingSkill").exists());
    }

    #[test]
    fn pytest_leaf_creation_never_adopts_an_existing_candidate() {
        let parent = tempfile::tempdir().unwrap();
        let existing = parent.path().join("preplanted-candidate");
        fs::create_dir(&existing).unwrap();
        fs::write(existing.join("sentinel.txt"), b"external").unwrap();

        assert!(
            create_exclusive_pytest_lease_leaf(parent.path(), "preplanted-candidate")
                .unwrap()
                .is_none()
        );
        assert_eq!(
            fs::read(existing.join("sentinel.txt")).unwrap(),
            b"external"
        );
    }

    #[test]
    fn pytest_lease_creation_failure_removes_the_partial_lease() {
        let dir = tempfile::tempdir().unwrap();
        let mut probes = 0;
        let error = create_pytest_lease_with_probe(dir.path(), "creation-failure", |_| {
            probes += 1;
            if probes == 2 {
                anyhow::bail!("injected probe failure");
            }
            Ok(())
        })
        .unwrap_err();
        assert!(error.to_string().contains("injected probe failure"));
        let leases = dir.path().join(".RaymanCodingSkill/tmp/leases");
        assert!(
            !leases.exists() || fs::read_dir(leases).unwrap().next().is_none(),
            "partial create left an orphan lease"
        );
    }

    #[cfg(windows)]
    #[test]
    fn pytest_creation_failure_preserves_a_replacement_leaf() {
        let dir = tempfile::tempdir().unwrap();
        let mut observed = None;

        let error = create_pytest_lease_with_probe(dir.path(), "creation-replacement", |path| {
            let original = path.parent().unwrap().to_path_buf();
            let displaced = original.with_file_name(format!(
                "{}-displaced",
                original.file_name().unwrap().to_string_lossy()
            ));
            fs::rename(&original, &displaced).context("replace pytest lease: rename original")?;
            fs::create_dir(&original).context("replace pytest lease: create replacement")?;
            fs::write(original.join("replacement-sentinel.txt"), b"replacement")?;
            observed = Some((displaced, original));
            anyhow::bail!("injected pytest probe failure")
        })
        .unwrap_err();

        assert!(
            format!("{error:#}").contains("injected pytest probe failure"),
            "{error:#}"
        );
        let (displaced, replacement) = observed.expect("replacement paths");
        assert!(displaced.is_dir());
        assert_eq!(
            fs::read(replacement.join("replacement-sentinel.txt")).unwrap(),
            b"replacement"
        );
    }

    #[test]
    fn pytest_lease_release_tolerates_runtime_owned_child_removal() {
        let dir = tempfile::tempdir().unwrap();
        let lease = create_pytest_lease(dir.path(), "runtime-removal").unwrap();
        fs::remove_dir_all(&lease.basetemp).unwrap();

        release_pytest_lease(dir.path(), &lease.id).unwrap();
        assert!(!Path::new(&lease.root).exists());
    }

    #[test]
    fn pytest_lease_paths_do_not_leak_the_windows_verbatim_prefix() {
        // Regression: path_text used raw Path::display(), so canonicalize()'s `\\?\` prefix
        // leaked into pytest --basetemp/cache_dir, TMP/TEMP/TMPDIR/PYTHONPYCACHEPREFIX, and
        // scratch output. On non-Windows the prefix never appears, so this also proves the
        // normalization is a no-op there; on Windows it proves the strip + verify consistency.
        let dir = tempfile::tempdir().unwrap();
        let lease = create_pytest_lease(dir.path(), "verbatim").unwrap();
        for value in [
            &lease.root,
            &lease.basetemp,
            &lease.cache_dir,
            &lease.temp_dir,
            &lease.pycache_dir,
        ] {
            assert!(
                !value.starts_with(r"\\?\"),
                "verbatim prefix leaked: {value}"
            );
        }
        for value in lease.environment.values() {
            assert!(
                !value.starts_with(r"\\?\"),
                "verbatim prefix leaked into env: {value}"
            );
        }
        for argument in &lease.pytest_args {
            assert!(
                !argument.contains(r"\\?\"),
                "verbatim prefix leaked into pytest_args: {argument}"
            );
        }
        // Normalizing the stored strings must keep the manifest self-comparison consistent.
        assert_eq!(verify_pytest_lease(dir.path(), &lease.id).unwrap(), lease);
    }

    #[test]
    fn pytest_lease_rejects_a_tampered_manifest_without_deleting_anything() {
        let dir = tempfile::tempdir().unwrap();
        let lease = create_pytest_lease(dir.path(), "tamper").unwrap();
        let outside = dir.path().join("outside");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("keep.txt"), "keep").unwrap();
        let manifest = Path::new(&lease.root).join(LEASE_MANIFEST);
        let mut tampered = lease.clone();
        tampered.basetemp = Path::new(&lease.root)
            .join("..")
            .join("outside")
            .display()
            .to_string();
        crate::file_io::write_json(&manifest, &tampered).unwrap();
        assert!(verify_pytest_lease(dir.path(), &lease.id).is_err());
        assert!(release_pytest_lease(dir.path(), &lease.id).is_err());
        assert_eq!(
            fs::read_to_string(outside.join("keep.txt")).unwrap(),
            "keep"
        );
        assert!(Path::new(&lease.root).is_dir());
    }

    #[cfg(windows)]
    #[test]
    fn pytest_lease_release_binds_manifest_to_the_guarded_leaf() {
        let dir = tempfile::tempdir().unwrap();
        let managed = create_managed_pytest_lease(dir.path(), "guarded-pytest-manifest").unwrap();
        let lease = managed.lease();
        let lease_root = PathBuf::from(&lease.root);
        fs::write(
            PathBuf::from(&lease.temp_dir).join("original-sentinel.txt"),
            b"original",
        )
        .unwrap();
        let displaced = lease_root.with_file_name(format!("{}-displaced", lease.id));

        // A held guard permits namespace rename; production release reopens
        // every edge and rejects the replacement before cleanup.  Inject the
        // swap immediately after the same pre-release verification boundary.
        managed.verify_current().unwrap();
        fs::rename(&lease_root, &displaced).unwrap();
        fs::create_dir(&lease_root).unwrap();
        crate::file_io::write_json(&lease_root.join(LEASE_MANIFEST), &lease).unwrap();
        fs::write(lease_root.join("replacement-sentinel.txt"), b"replacement").unwrap();
        let error = release_managed_pytest_lease_with_storage(
            dir.path(),
            &managed,
            PytestLeaseStorage::Workspace,
        )
        .unwrap_err();

        assert!(format!("{error:#}").contains("身份"), "{error:#}");
        assert_eq!(
            fs::read(displaced.join("temp").join("original-sentinel.txt")).unwrap(),
            b"original"
        );
        assert_eq!(
            fs::read(lease_root.join("replacement-sentinel.txt")).unwrap(),
            b"replacement"
        );
    }

    #[cfg(windows)]
    #[test]
    fn external_pytest_release_binds_manifest_to_the_guarded_leaf() {
        let external = tempfile::tempdir().unwrap();
        let managed =
            create_external_managed_pytest_lease(external.path(), "guarded-external").unwrap();
        let lease = managed.lease();
        let lease_root = PathBuf::from(&lease.root);
        fs::write(
            PathBuf::from(&lease.temp_dir).join("original-sentinel.txt"),
            b"original",
        )
        .unwrap();
        let displaced = lease_root.with_file_name(format!("{}-displaced", lease.id));

        managed.verify_current().unwrap();
        fs::rename(&lease_root, &displaced).unwrap();
        fs::create_dir(&lease_root).unwrap();
        crate::file_io::write_json(&lease_root.join(LEASE_MANIFEST), &lease).unwrap();
        fs::write(lease_root.join("replacement-sentinel.txt"), b"replacement").unwrap();
        let error = release_managed_pytest_lease_with_storage(
            external.path(),
            &managed,
            PytestLeaseStorage::External,
        )
        .unwrap_err();

        assert!(format!("{error:#}").contains("身份"), "{error:#}");
        assert_eq!(
            fs::read(displaced.join("t").join("original-sentinel.txt")).unwrap(),
            b"original"
        );
        assert_eq!(
            fs::read(lease_root.join("replacement-sentinel.txt")).unwrap(),
            b"replacement"
        );
        assert!(!external.path().join(".RaymanCodingSkill").exists());
    }

    #[cfg(windows)]
    #[test]
    fn external_pytest_pre_release_replacement_fails_closed() {
        let external = tempfile::tempdir().unwrap();
        let managed =
            create_external_managed_pytest_lease(external.path(), "pre-release-replacement")
                .unwrap();
        let lease = managed.lease();
        let original = PathBuf::from(&lease.root);
        let displaced = original.with_file_name(format!("{}-displaced", lease.id));
        fs::rename(&original, &displaced).unwrap();
        fs::create_dir(&original).unwrap();
        let layout = pytest_lease_layout(PytestLeaseStorage::External);
        for name in [
            layout.basetemp,
            layout.cache_dir,
            layout.temp_dir,
            layout.pycache_dir,
            layout.nested_validation_root,
        ] {
            fs::create_dir(original.join(name)).unwrap();
        }
        crate::file_io::write_json(&original.join(LEASE_MANIFEST), lease).unwrap();

        let error = release_external_managed_pytest_lease(external.path(), &managed).unwrap_err();
        assert!(format!("{error:#}").contains("创建目录"), "{error:#}");
        assert!(
            displaced.is_dir() && original.is_dir(),
            "identity mismatch must preserve both the original object and replacement"
        );
        assert!(!external.path().join(".RaymanCodingSkill").exists());
    }

    #[test]
    fn cargo_target_lease_is_unique_probed_and_manifest_owned() {
        let dir = tempfile::tempdir().unwrap();
        let first = create_cargo_target_lease(dir.path(), "validation cargo").unwrap();
        let second = create_cargo_target_lease(dir.path(), "validation cargo").unwrap();
        assert_ne!(first.id, second.id);
        assert_eq!(first.schema, "rayman.cargo-target-lease.v1");
        assert!(Path::new(&first.target_dir).is_dir());
        assert_eq!(
            verify_cargo_target_lease(dir.path(), &first.id).unwrap(),
            first
        );
        release_cargo_target_lease(dir.path(), &first).unwrap();
        assert!(!Path::new(&first.root).exists());
        assert!(Path::new(&second.root).is_dir());
    }

    #[test]
    fn cargo_target_lease_creation_failure_removes_the_partial_lease() {
        let dir = tempfile::tempdir().unwrap();
        let error = create_cargo_target_lease_with_probe(dir.path(), "creation-failure", |_| {
            anyhow::bail!("injected cargo target probe failure")
        })
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("injected cargo target probe failure")
        );
        let leases = dir.path().join(".RaymanCodingSkill/tmp/c");
        assert!(
            !leases.exists() || fs::read_dir(leases).unwrap().next().is_none(),
            "partial Cargo target create left an orphan lease"
        );
    }

    #[cfg(windows)]
    #[test]
    fn cargo_target_creation_failure_preserves_a_replacement_leaf() {
        let dir = tempfile::tempdir().unwrap();
        let mut observed = None;

        let error = create_cargo_target_lease_with_probe(
            dir.path(),
            "creation-replacement",
            |target_dir| {
                let original = target_dir.parent().unwrap().to_path_buf();
                let displaced = original.with_file_name(format!(
                    "{}-displaced",
                    original.file_name().unwrap().to_string_lossy()
                ));
                fs::rename(&original, &displaced)?;
                fs::create_dir(&original)?;
                fs::write(original.join("replacement-sentinel.txt"), b"replacement")?;
                observed = Some((displaced, original));
                anyhow::bail!("injected Cargo target probe failure")
            },
        )
        .unwrap_err();

        assert!(
            format!("{error:#}").contains("injected Cargo target probe failure"),
            "{error:#}"
        );
        let (displaced, replacement) = observed.expect("replacement paths");
        assert!(displaced.is_dir());
        assert_eq!(
            fs::read(replacement.join("replacement-sentinel.txt")).unwrap(),
            b"replacement"
        );
    }

    #[test]
    fn cargo_target_lease_rejects_tampering_without_deleting_external_data() {
        let dir = tempfile::tempdir().unwrap();
        let lease = create_cargo_target_lease(dir.path(), "tamper").unwrap();
        let outside = dir.path().join("outside-cargo-target");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("keep.txt"), "keep").unwrap();
        let manifest = Path::new(&lease.root).join(CARGO_TARGET_LEASE_MANIFEST);
        let mut tampered = lease.clone();
        tampered.target_dir = outside.display().to_string();
        crate::file_io::write_json(&manifest, &tampered).unwrap();

        assert!(verify_cargo_target_lease(dir.path(), &lease.id).is_err());
        assert!(release_cargo_target_lease(dir.path(), &lease).is_err());
        assert_eq!(
            fs::read_to_string(outside.join("keep.txt")).unwrap(),
            "keep"
        );
        assert!(Path::new(&lease.root).is_dir());
    }

    #[cfg(windows)]
    #[test]
    fn cargo_target_lease_release_binds_manifest_to_the_guarded_leaf() {
        let dir = tempfile::tempdir().unwrap();
        let lease = create_cargo_target_lease(dir.path(), "guarded-manifest").unwrap();
        let lease_root = PathBuf::from(&lease.root);
        fs::write(
            PathBuf::from(&lease.target_dir).join("artifact.bin"),
            b"original",
        )
        .unwrap();
        let displaced = lease_root.with_file_name(format!("{}-displaced", lease.id));

        let error = release_cargo_target_lease_windows(dir.path(), &lease, |verified_leaf| {
            assert_eq!(display_path(verified_leaf), lease.root);
            fs::rename(verified_leaf, &displaced)?;
            fs::create_dir_all(verified_leaf.join(CARGO_TARGET_DIR))?;
            crate::file_io::write_json(&verified_leaf.join(CARGO_TARGET_LEASE_MANIFEST), &lease)?;
            fs::write(
                verified_leaf.join("replacement-sentinel.txt"),
                b"replacement",
            )?;
            Ok(())
        })
        .unwrap_err();

        assert!(format!("{error:#}").contains("身份"), "{error:#}");
        assert_eq!(
            fs::read(displaced.join(CARGO_TARGET_DIR).join("artifact.bin")).unwrap(),
            b"original"
        );
        assert_eq!(
            fs::read(lease_root.join("replacement-sentinel.txt")).unwrap(),
            b"replacement"
        );
    }

    #[cfg(unix)]
    #[test]
    fn audit_and_cleanup_refuse_a_symlinked_temp_root() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join(".RaymanCodingSkill")).unwrap();
        symlink(outside.path(), temp_root(dir.path())).unwrap();

        let report = audit(dir.path());
        assert_eq!(report.traversal_error_count, 1);
        assert!(cleanup(dir.path()).is_err());
        assert!(outside.path().exists(), "cleanup must not follow the link");
    }

    /// id 从原始 label 派生、manifest 却存 trim 后的 label，会让 create 自己
    /// 造出的 lease 通不过自己的 verify，留下一个 release 无法指名的孤儿。
    #[test]
    fn a_label_needing_trimming_still_creates_a_verifiable_lease() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join(".RaymanCodingSkill")).unwrap();
        let lease = create_pytest_lease(root, "  spaced label  ").unwrap();
        assert_eq!(lease.label, "spaced label");
        assert!(verify_pytest_lease(root, &lease.id).is_ok());
        release_pytest_lease(root, &lease.id).unwrap();
    }

    /// manifest 门禁必须整体比对 environment：逐键查五个已知项是子集比较，
    /// 注入的额外变量能穿过门禁，再被 `temp pytest-probe` 当作"已核验"发布。
    #[test]
    fn an_injected_environment_variable_fails_the_lease_manifest_gate() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join(".RaymanCodingSkill")).unwrap();
        let lease = create_pytest_lease(root, "probe-env").unwrap();
        assert!(verify_pytest_lease(root, &lease.id).is_ok());

        let manifest_path = lease_path(root, &lease.id, false)
            .unwrap()
            .unwrap()
            .join(LEASE_MANIFEST);
        let mut tampered = lease.clone();
        tampered
            .environment
            .insert("PYTHONPATH".into(), "C:/injected".into());
        crate::file_io::write_json(&manifest_path, &tampered).unwrap();
        assert!(
            verify_pytest_lease(root, &lease.id).is_err(),
            "注入的额外环境变量必须被门禁挡下"
        );

        let mut relabelled = lease.clone();
        relabelled.label = "totally-different".into();
        crate::file_io::write_json(&manifest_path, &relabelled).unwrap();
        assert!(
            verify_pytest_lease(root, &lease.id).is_err(),
            "label 必须与 lease id 绑定"
        );
    }

    /// 纯点 id 会被 `Path::components()` 归一化塌缩到 leases 根（release 的
    /// remove_dir_all 会删光所有兄弟 lease）；Windows 还会剥掉尾部 `.` 形成
    /// 跨 lease 别名。两类都必须在 id 门口拒绝。
    #[test]
    fn lease_ids_that_collapse_or_alias_are_rejected() {
        for bad in [".", "..", "...", "-", "_", "-_-", "a.", "alpha."] {
            assert!(lease_relative(bad).is_err(), "must reject {bad:?}");
        }
        for good in ["alpha", "alpha-1.2_x", ".hidden1"] {
            assert!(lease_relative(good).is_ok(), "must accept {good:?}");
        }
    }
}
