//! Safe workspace-local state paths.
//!
//! All managed state lives below `.RaymanCodingSkill/`.  A normal `create_dir_all`
//! or `exists` call follows an ancestor symlink/junction before the caller gets a
//! chance to inspect the final path, which can turn a workspace-local write or
//! cleanup into an external one.  These helpers canonicalize the workspace root
//! once and then create/check every managed-state component without following a
//! symlink or Windows reparse point below that root.

use std::fs;
use std::io;
#[cfg(windows)]
use std::io::Write as _;
use std::path::{Component, Path, PathBuf};
#[cfg(windows)]
use std::sync::atomic::{AtomicU64, Ordering};

use crate::file_io::is_link_or_reparse;
#[cfg(windows)]
use crate::file_io::{
    FileIdentity, file_identity_from_handle, has_strong_file_identity, read_bytes_from_handle,
};
// Managed-state paths are canonicalized, so raw `display()` here leaked the
// Windows `\\?\` verbatim prefix into the write probe and into every diagnostic
// — the prefix the rest of the codebase strips by rule.
use crate::pathfmt::display_path;
use anyhow::{Context, Result, bail};

pub const STATE_DIR_NAME: &str = ".RaymanCodingSkill";

#[cfg(windows)]
static WINDOWS_DIRECTORY_PROBE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Return a verified managed-state root, creating it only when requested.
pub fn managed_state_root(root: &Path, create: bool) -> Result<Option<PathBuf>> {
    let workspace = canonical_workspace_root(root)?;
    let state = workspace.join(STATE_DIR_NAME);
    match fs::symlink_metadata(&state) {
        Ok(metadata) => {
            ensure_real_directory_metadata(&state, &metadata)?;
            ensure_real_directory_within(&workspace, &state)?;
            Ok(Some(state))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound && !create => Ok(None),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            create_real_directory(&state)?;
            ensure_real_directory_within(&workspace, &state)?;
            Ok(Some(state))
        }
        Err(error) => {
            Err(error).with_context(|| format!("无法读取受管状态根: {}", display_path(&state)))
        }
    }
}

/// Return a verified managed-state directory.  `relative` is relative to
/// `.RaymanCodingSkill`; if it is absent and `create` is false, return `None`.
pub fn managed_state_dir(root: &Path, relative: &Path, create: bool) -> Result<Option<PathBuf>> {
    let workspace = canonical_workspace_root(root)?;
    let Some(mut current) = managed_state_root(root, create)? else {
        return Ok(None);
    };
    for component in normal_components(relative)? {
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(metadata) => {
                ensure_real_directory_metadata(&current, &metadata)?;
                // `symlink_metadata(current)` only sees the final component.
                // Re-canonicalize after every step so an ancestor swapped to a
                // link between iterations cannot redirect the next child into
                // a directory outside this workspace.
                ensure_real_directory_within(&workspace, &current)?;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound && !create => return Ok(None),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                create_real_directory(&current)?;
                ensure_real_directory_within(&workspace, &current)?;
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("无法读取受管状态目录: {}", display_path(&current)));
            }
        }
    }
    Ok(Some(current))
}

/// Return a verified directory directly below an already dedicated external
/// root. Unlike `managed_state_dir`, this deliberately does not create a
/// `.RaymanCodingSkill` component: validation TEMP descendants must not inherit
/// a false workspace marker from their lease authority root.
pub(crate) fn managed_external_dir(
    root: &Path,
    relative: &Path,
    create: bool,
) -> Result<Option<PathBuf>> {
    let external_root = canonical_workspace_root(root)?;
    let components = normal_components(relative)?;
    if components.is_empty() {
        bail!("外部受管目录路径不能为空");
    }
    let mut current = external_root.clone();
    for component in components {
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(metadata) => {
                ensure_real_directory_metadata(&current, &metadata)?;
                ensure_real_directory_within(&external_root, &current)?;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound && !create => return Ok(None),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                create_real_directory(&current)?;
                ensure_real_directory_within(&external_root, &current)?;
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("无法读取受管状态目录: {}", display_path(&current)));
            }
        }
    }
    Ok(Some(current))
}

/// Return a verified managed-state file path.  Existing files must not be a
/// symlink/reparse point.  Missing parent directories are created only when
/// `create_parents` is true.
pub fn managed_state_file(root: &Path, relative: &Path, create_parents: bool) -> Result<PathBuf> {
    let workspace = canonical_workspace_root(root)?;
    let components = normal_components(relative)?;
    let (name, parents) = components
        .split_last()
        .ok_or_else(|| anyhow::anyhow!("受管状态文件路径不能为空"))?;
    let parent_relative = parents.iter().fold(PathBuf::new(), |mut path, component| {
        path.push(*component);
        path
    });
    let parent = match managed_state_dir(root, &parent_relative, create_parents)? {
        Some(parent) => parent,
        None => canonical_workspace_root(root)?
            .join(STATE_DIR_NAME)
            .join(relative),
    };
    if parent.ends_with(relative) {
        // A missing ancestor is safe to represent lexically for a read; read_json
        // will report it as absent.  It must not be used as a writable parent.
        return Ok(parent);
    }
    ensure_real_directory_within(&workspace, &parent)?;
    let path = parent.join(*name);
    match fs::symlink_metadata(&path) {
        Ok(metadata) if is_link_or_reparse(&metadata) => {
            bail!("拒绝链接/reparse 受管状态文件: {}", display_path(&path));
        }
        Ok(metadata) if !metadata.file_type().is_file() => {
            bail!("受管状态路径不是普通文件: {}", display_path(&path));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error)
                .with_context(|| format!("无法读取受管状态文件: {}", display_path(&path)));
        }
    }
    Ok(path)
}

/// Check an already-created directory immediately before a state transaction.
pub fn ensure_real_directory(path: &Path) -> Result<()> {
    crate::file_io::ensure_real_directory_labeled(path, "受管状态目录")
}

/// Remove one managed-state directory without allowing an ancestor swap to
/// redirect the cleanup outside the workspace. On Windows every component is
/// opened relative to its held parent, directories are enumerated from their
/// handles, and files/directories are disposed through their verified handles.
/// Namespace renames remain permitted and are detected by strong-identity
/// revalidation, so a replacement fails closed as an orphan instead of widening
/// the deletion target.
pub fn remove_managed_state_dir_all(root: &Path, relative: &Path) -> Result<bool> {
    #[cfg(windows)]
    {
        remove_managed_state_dir_all_windows(root, relative, |_, _, _| Ok(()), |_, _| Ok(()))
    }

    #[cfg(not(windows))]
    {
        let Some(path) = managed_state_dir(root, relative, false)? else {
            return Ok(false);
        };
        ensure_real_directory(&path)?;
        fs::remove_dir_all(&path)
            .with_context(|| format!("无法删除受管状态目录树: {}", display_path(&path)))?;
        Ok(true)
    }
}

/// Remove one manifest-owned directory directly below a dedicated external
/// root on non-Windows hosts. Windows creation and release paths require a
/// held-object verifier instead of this unbound helper.
#[cfg(any(not(windows), test))]
pub(crate) fn remove_managed_external_dir_all(root: &Path, relative: &Path) -> Result<bool> {
    let Some(path) = managed_external_dir(root, relative, false)? else {
        return Ok(false);
    };
    ensure_real_directory(&path)?;
    fs::remove_dir_all(&path)
        .with_context(|| format!("无法删除受管状态目录树: {}", display_path(&path)))?;
    Ok(true)
}

/// Remove a Windows managed-state tree after validating its held leaf, then
/// revalidate the caller's lease contract immediately after the deletion
/// snapshot of that leaf.  The snapshot verifier closes the interval between
/// a caller's pre-release validation and the handle-bound enumerator: if a
/// direct child is renamed or replaced before the snapshot it is rejected by
/// the verifier; if it is replaced afterwards, the snapshot's file id will no
/// longer match the handle opened for deletion.
#[cfg(windows)]
pub(crate) fn remove_managed_state_dir_all_windows_verified_with_snapshot<F, G>(
    root: &Path,
    relative: &Path,
    verifier: F,
    snapshot_verifier: G,
) -> Result<bool>
where
    F: FnOnce(&fs::File, &Path) -> Result<()>,
    G: FnOnce(&Path, &fs::File) -> Result<()>,
{
    let mut snapshot_verifier = Some(snapshot_verifier);
    remove_managed_state_dir_all_windows(
        root,
        relative,
        |_, target, leaf| verifier(leaf, target),
        |snapshot_path, snapshot_leaf| {
            // `empty_windows_directory_handle` starts at the verified lease
            // leaf, so the first snapshot is the exact root whose contract we
            // must revalidate.  Do not run a lease-root verifier for arbitrary
            // nested runtime output directories.
            if let Some(verifier) = snapshot_verifier.take() {
                verifier(snapshot_path, snapshot_leaf)?;
            }
            Ok(())
        },
    )
}

/// External-root counterpart of the managed-state snapshot verifier.
/// External validation leases are marker-free but need the same held-leaf and
/// post-snapshot identity boundary before cleanup can delete any child.
#[cfg(windows)]
pub(crate) fn remove_managed_external_dir_all_windows_verified_with_snapshot<F, G>(
    root: &Path,
    relative: &Path,
    verifier: F,
    snapshot_verifier: G,
) -> Result<bool>
where
    F: FnOnce(&fs::File, &Path) -> Result<()>,
    G: FnOnce(&Path, &fs::File) -> Result<()>,
{
    let mut snapshot_verifier = Some(snapshot_verifier);
    remove_managed_dir_all_windows(
        root,
        relative,
        false,
        |_, target, leaf| verifier(leaf, target),
        |snapshot_path, snapshot_leaf| {
            if let Some(verifier) = snapshot_verifier.take() {
                verifier(snapshot_path, snapshot_leaf)?;
            }
            Ok(())
        },
    )
}

#[cfg(windows)]
fn remove_managed_state_dir_all_windows<F, G>(
    root: &Path,
    relative: &Path,
    before_remove: F,
    after_snapshot: G,
) -> Result<bool>
where
    F: FnOnce(&Path, &Path, &fs::File) -> Result<()>,
    G: FnMut(&Path, &fs::File) -> Result<()>,
{
    remove_managed_dir_all_windows(root, relative, true, before_remove, after_snapshot)
}

#[cfg(windows)]
fn remove_managed_dir_all_windows<F, G>(
    root: &Path,
    relative: &Path,
    include_state_dir: bool,
    before_remove: F,
    mut after_snapshot: G,
) -> Result<bool>
where
    F: FnOnce(&Path, &Path, &fs::File) -> Result<()>,
    G: FnMut(&Path, &fs::File) -> Result<()>,
{
    use std::ffi::OsString;

    let components = normal_components(relative)?;
    if components.is_empty() {
        bail!("受管状态删除路径不能为空");
    }
    let workspace = canonical_workspace_root(root)?;
    let workspace_file = open_windows_directory_guard(&workspace)?;
    let workspace_identity =
        windows_handle_identity(&workspace_file, &workspace, true, "受管状态工作区根")?;
    let mut chain = vec![WindowsDirectoryHandle {
        file: workspace_file,
        identity: workspace_identity,
        logical_path: workspace.clone(),
        name: None,
    }];
    let mut names = Vec::with_capacity(components.len() + usize::from(include_state_dir));
    if include_state_dir {
        names.push(OsString::from(STATE_DIR_NAME));
    }
    names.extend(components.into_iter().map(OsString::from));

    for (index, name) in names.iter().enumerate() {
        let logical_path = chain
            .last()
            .expect("workspace handle exists")
            .logical_path
            .join(name);
        let opened = open_windows_relative(
            &chain.last().expect("parent handle exists").file,
            name,
            true,
            if index + 1 == names.len() {
                WINDOWS_DIRECTORY_DELETE_ACCESS
            } else {
                WINDOWS_DIRECTORY_GUARD_ACCESS
            },
            WINDOWS_NAMESPACE_SHARE,
        );
        let file = match opened {
            Ok(file) => file,
            Err(error) if windows_not_found(&error) => return Ok(false),
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "无法按父目录句柄打开受管状态删除分量: {}",
                        display_path(&logical_path)
                    )
                });
            }
        };
        let identity = windows_handle_identity(&file, &logical_path, true, "受管状态删除目录")?;
        ensure_same_windows_volume(
            &chain.last().expect("parent handle exists").identity,
            &identity,
            &logical_path,
        )?;
        chain.push(WindowsDirectoryHandle {
            file,
            identity,
            logical_path,
            name: Some(name.clone()),
        });
    }

    let leaf_index = chain.len() - 1;
    let parent_index = leaf_index - 1;
    let parent = chain[parent_index].logical_path.clone();
    let target = chain[leaf_index].logical_path.clone();
    before_remove(&parent, &target, &chain[leaf_index].file)?;

    // The verifier above is allowed to inspect the held leaf. Re-open every
    // namespace edge afterwards so a verifier-time rename/replacement cannot
    // turn "verified A" into "delete B".
    revalidate_windows_directory_chain(&chain)?;
    let mut no_cleanup_revalidation = || Ok(());
    empty_windows_directory_handle(
        &chain[leaf_index].file,
        &chain[leaf_index].identity,
        &target,
        &mut after_snapshot,
        &mut no_cleanup_revalidation,
    )?;
    // A parent may move while children are being emptied. Never dispose the
    // leaf unless the complete workspace-relative chain still names the same
    // strong file objects.
    revalidate_windows_directory_chain(&chain)?;
    let leaf_name = chain[leaf_index]
        .name
        .clone()
        .expect("leaf has a relative name");
    no_cleanup_revalidation()?;
    delete_windows_bound_entry(
        &chain[parent_index].file,
        &leaf_name,
        &chain[leaf_index].file,
        &chain[leaf_index].identity,
        true,
        &target,
    )?;
    drop(chain.pop().expect("leaf handle exists"));
    ensure_windows_name_absent(&chain[parent_index].file, &leaf_name, true, &target)?;
    drop(chain);
    Ok(true)
}

#[cfg(windows)]
#[derive(Debug)]
pub(crate) struct WindowsDirectoryObjectGuard {
    directory: fs::File,
    identity: FileIdentity,
    logical_path: PathBuf,
}

/// Creation-bound evidence for one direct directory child.
///
/// Unlike `WindowsDirectoryObjectGuard`, this deliberately retains no open
/// handle after creation. Windows refuses a parent-directory rename while any
/// direct child directory handle remains open, even when that handle grants
/// `FILE_SHARE_DELETE`. The name is instead reopened through the still-held
/// parent for every use and compared with this strong identity, so a rename or
/// replacement remains fail-closed rather than being prevented by a lock.
#[cfg(windows)]
#[derive(Debug, Clone)]
pub(crate) struct WindowsDirectoryChildIdentity {
    identity: FileIdentity,
    logical_path: PathBuf,
}

#[cfg(windows)]
impl WindowsDirectoryObjectGuard {
    /// The absolute namespace path that was bound to this held directory object.
    pub(crate) fn path(&self) -> &Path {
        &self.logical_path
    }

    /// Capture this object's strong identity as a direct-child token. The
    /// caller must retain the actual parent guard and use that guard to
    /// re-open the name; the token deliberately retains no directory handle.
    pub(crate) fn child_identity(&self) -> WindowsDirectoryChildIdentity {
        WindowsDirectoryChildIdentity {
            identity: self.identity.clone(),
            logical_path: self.logical_path.clone(),
        }
    }

    /// Atomically create one ordinary direct child and retain only its
    /// creation-bound identity. The child handle is closed before returning so
    /// Windows can rename the parent namespace. Every later use must re-open
    /// the child relative to this held parent and compare that identity.
    pub(crate) fn create_child_exclusive(
        &self,
        name: &std::ffi::OsStr,
        label: &str,
    ) -> Result<WindowsDirectoryChildIdentity> {
        ensure_windows_single_component(name)?;
        verify_windows_directory_object_at_path(self, self.path(), "原子目录创建父目录")?;
        let logical_path = self.path().join(name);
        let directory = create_windows_relative_directory(
            &self.directory,
            name,
            WINDOWS_DIRECTORY_GUARD_ACCESS
                | windows_sys::Win32::Storage::FileSystem::FILE_ADD_SUBDIRECTORY,
            WINDOWS_GUARD_SHARE,
        )
        .with_context(|| {
            format!(
                "无法原子创建{label}；目录可能已存在: {}",
                display_path(&logical_path)
            )
        })?;
        let identity = windows_handle_identity(&directory, &logical_path, true, label)?;
        let child = WindowsDirectoryChildIdentity {
            identity,
            logical_path,
        };
        verify_windows_directory_child_identity(self, &child, name, label)?;
        // The only authority retained for a child is the identity captured
        // from its exclusive creation handle. Keeping that handle live would
        // make a parent rename fail with Win32 5, which is precisely the
        // namespace move we must allow and then reject on revalidation.
        drop(directory);
        Ok(child)
    }

    /// Re-open a direct child through this held parent handle and compare it
    /// with the child's creation-bound identity. This is the publication check
    /// for paths handed to an untrusted validation child.
    pub(crate) fn verify_direct_child(
        &self,
        child: &WindowsDirectoryChildIdentity,
        name: &std::ffi::OsStr,
        label: &str,
    ) -> Result<()> {
        verify_windows_directory_child_identity(self, child, name, label)
    }

    /// Prove one creation-bound direct child is writable without resolving it
    /// through an absolute namespace path. The probe file is created, read and
    /// deleted relative to a short-lived no-follow child handle, then the
    /// creation identity is checked again before the path can be published.
    ///
    /// The short-lived handle shares DELETE deliberately: namespace moves are
    /// allowed and detected by the identity revalidation rather than prevented
    /// by a share-mode lock.
    pub(crate) fn probe_direct_child(
        &self,
        child: &WindowsDirectoryChildIdentity,
        name: &std::ffi::OsStr,
        label: &str,
    ) -> Result<()> {
        const PROBE_BYTES: &[u8] = b"rayman-bound-directory-probe";

        ensure_windows_single_component(name)?;
        self.verify_direct_child(child, name, label)?;
        let child_path = child.path();
        let child_handle = open_windows_relative(
            &self.directory,
            name,
            true,
            WINDOWS_DIRECTORY_GUARD_ACCESS | windows_sys::Win32::Storage::FileSystem::FILE_ADD_FILE,
            WINDOWS_NAMESPACE_SHARE,
        )
        .with_context(|| {
            format!(
                "无法通过持有父目录打开{label}以执行写探针: {}",
                display_path(child_path)
            )
        })?;
        verify_windows_directory_child_identity_from_open(child, &child_handle, child_path, label)?;

        let sequence = WINDOWS_DIRECTORY_PROBE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let probe_name = std::ffi::OsString::from(format!(
            ".rayman-probe-{}-{sequence}.tmp",
            std::process::id()
        ));
        let probe_path = child_path.join(&probe_name);
        let mut probe = create_windows_relative_file(
            &child_handle,
            &probe_name,
            WINDOWS_FILE_PROBE_ACCESS,
            WINDOWS_NAMESPACE_SHARE,
        )
        .with_context(|| format!("{label}写探针失败: {}", display_path(&probe_path)))?;
        let probe_identity = windows_handle_identity(&probe, &probe_path, false, label)?;
        probe
            .write_all(PROBE_BYTES)
            .with_context(|| format!("{label}写探针失败: {}", display_path(&probe_path)))?;
        probe
            .sync_all()
            .with_context(|| format!("{label}同步探针失败: {}", display_path(&probe_path)))?;
        let after_write = windows_handle_identity(&probe, &probe_path, false, label)?;
        if !same_windows_file_object(&probe_identity, &after_write)
            || after_write.len != PROBE_BYTES.len() as u64
        {
            bail!(
                "{label}探针写入期间发生身份或长度变化: {}",
                display_path(&probe_path)
            );
        }
        let bytes = read_bytes_from_handle(&probe, after_write.len, &probe_path, label)?;
        let after_read = windows_handle_identity(&probe, &probe_path, false, label)?;
        if bytes != PROBE_BYTES || !same_windows_file_object(&after_write, &after_read) {
            bail!("{label}探针内容或身份不一致: {}", display_path(&probe_path));
        }
        revalidate_windows_named_entry(
            &child_handle,
            &probe_name,
            &after_read,
            false,
            &probe_path,
        )?;
        delete_windows_file_by_handle(&probe)
            .with_context(|| format!("{label}清理探针失败: {}", display_path(&probe_path)))?;
        drop(probe);
        ensure_windows_name_absent(&child_handle, &probe_name, false, &probe_path)?;
        drop(child_handle);
        self.verify_direct_child(child, name, label)
    }

    /// Prove this held directory itself is writable through its retained
    /// no-follow object handle. This is the host-root counterpart of
    /// `probe_direct_child`: no absolute path is re-resolved between identity
    /// checks, so a replacement directory cannot receive probe I/O.
    pub(crate) fn probe_self(&self, label: &str) -> Result<()> {
        const PROBE_BYTES: &[u8] = b"rayman-bound-directory-probe";

        verify_windows_directory_object_at_path(self, self.path(), label)?;
        let sequence = WINDOWS_DIRECTORY_PROBE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let probe_name = std::ffi::OsString::from(format!(
            ".rayman-probe-{}-{sequence}.tmp",
            std::process::id()
        ));
        let probe_path = self.path().join(&probe_name);
        let mut probe = create_windows_relative_file(
            &self.directory,
            &probe_name,
            WINDOWS_FILE_PROBE_ACCESS,
            WINDOWS_NAMESPACE_SHARE,
        )
        .with_context(|| format!("{label}写探针失败: {}", display_path(&probe_path)))?;
        let probe_identity = windows_handle_identity(&probe, &probe_path, false, label)?;
        probe
            .write_all(PROBE_BYTES)
            .with_context(|| format!("{label}写探针失败: {}", display_path(&probe_path)))?;
        probe
            .sync_all()
            .with_context(|| format!("{label}同步探针失败: {}", display_path(&probe_path)))?;
        let after_write = windows_handle_identity(&probe, &probe_path, false, label)?;
        if !same_windows_file_object(&probe_identity, &after_write)
            || after_write.len != PROBE_BYTES.len() as u64
        {
            bail!(
                "{label}探针写入期间发生身份或长度变化: {}",
                display_path(&probe_path)
            );
        }
        let bytes = read_bytes_from_handle(&probe, after_write.len, &probe_path, label)?;
        let after_read = windows_handle_identity(&probe, &probe_path, false, label)?;
        if bytes != PROBE_BYTES || !same_windows_file_object(&after_write, &after_read) {
            bail!("{label}探针内容或身份不一致: {}", display_path(&probe_path));
        }
        revalidate_windows_named_entry(
            &self.directory,
            &probe_name,
            &after_read,
            false,
            &probe_path,
        )?;
        delete_windows_file_by_handle(&probe)
            .with_context(|| format!("{label}清理探针失败: {}", display_path(&probe_path)))?;
        drop(probe);
        ensure_windows_name_absent(&self.directory, &probe_name, false, &probe_path)?;
        verify_windows_directory_object_at_path(self, self.path(), label)
    }

    /// Write one new direct child through the held directory handle.  The
    /// leaf is created exclusively, flushed, read back through the same
    /// handle, and reopened by name for strong-identity revalidation before
    /// its namespace path is returned to an external installer process.
    pub(crate) fn write_file_exclusive(
        &self,
        name: &std::ffi::OsStr,
        bytes: &[u8],
        label: &str,
    ) -> Result<PathBuf> {
        ensure_windows_single_component(name)?;
        verify_windows_directory_object_at_path(self, self.path(), label)?;
        let path = self.path().join(name);
        let mut file = create_windows_relative_file(
            &self.directory,
            name,
            WINDOWS_FILE_CREATE_ACCESS,
            WINDOWS_NAMESPACE_SHARE,
        )
        .with_context(|| format!("cannot exclusively create {label}: {}", display_path(&path)))?;
        let created = windows_handle_identity(&file, &path, false, label)?;
        file.write_all(bytes)
            .with_context(|| format!("cannot write {label}: {}", display_path(&path)))?;
        file.sync_all()
            .with_context(|| format!("cannot flush {label}: {}", display_path(&path)))?;
        let written = windows_handle_identity(&file, &path, false, label)?;
        if !same_windows_file_object(&created, &written) || written.len != bytes.len() as u64 {
            bail!(
                "{label} changed identity or length while being written: {}",
                display_path(&path)
            );
        }
        let captured = read_bytes_from_handle(&file, written.len, &path, label)?;
        let final_identity = windows_handle_identity(&file, &path, false, label)?;
        if captured != bytes || !same_windows_file_object(&written, &final_identity) {
            bail!(
                "{label} changed while being verified: {}",
                display_path(&path)
            );
        }
        revalidate_windows_named_entry(&self.directory, name, &final_identity, false, &path)?;
        verify_windows_directory_object_at_path(self, self.path(), label)?;
        drop(file);
        Ok(path)
    }

    /// Open (or create) a persistent direct container through this held
    /// directory object. The returned guard is intended only for a short
    /// nested creation/cleanup scope: callers must not retain a direct-child
    /// handle as a substitute for identity revalidation.
    pub(crate) fn open_or_create_direct_child(
        &self,
        name: &std::ffi::OsStr,
        label: &str,
    ) -> Result<WindowsDirectoryObjectGuard> {
        use windows_sys::Win32::Storage::FileSystem::{FILE_ADD_FILE, FILE_ADD_SUBDIRECTORY};

        ensure_windows_single_component(name)?;
        verify_windows_directory_object_at_path(self, self.path(), label)?;
        let logical_path = self.path().join(name);
        let access = WINDOWS_DIRECTORY_GUARD_ACCESS | FILE_ADD_SUBDIRECTORY | FILE_ADD_FILE;
        let open =
            || open_windows_relative(&self.directory, name, true, access, WINDOWS_NAMESPACE_SHARE);
        let directory = match open() {
            Ok(directory) => directory,
            Err(error) if windows_not_found(&error) => {
                match create_windows_relative_directory(
                    &self.directory,
                    name,
                    access,
                    WINDOWS_NAMESPACE_SHARE,
                ) {
                    Ok(directory) => directory,
                    Err(create_error)
                        if create_error.kind() == io::ErrorKind::AlreadyExists
                            || matches!(create_error.raw_os_error(), Some(80 | 183)) =>
                    {
                        open().with_context(|| {
                            format!(
                                "无法通过持有父目录打开并发创建后的{label}: {}",
                                display_path(&logical_path)
                            )
                        })?
                    }
                    Err(create_error) => {
                        return Err(create_error).with_context(|| {
                            format!(
                                "无法通过持有父目录创建{label}: {}",
                                display_path(&logical_path)
                            )
                        });
                    }
                }
            }
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "无法通过持有父目录打开{label}: {}",
                        display_path(&logical_path)
                    )
                });
            }
        };
        let identity = windows_handle_identity(&directory, &logical_path, true, label)?;
        ensure_same_windows_volume(&self.identity, &identity, &logical_path)?;
        let child = WindowsDirectoryObjectGuard {
            directory,
            identity,
            logical_path,
        };
        verify_windows_directory_object_at_path(self, self.path(), label)?;
        verify_windows_directory_object_at_path(&child, child.path(), label)?;
        verify_windows_directory_object_at_path(self, self.path(), label)?;
        Ok(child)
    }

    /// Short-lived handle bridge for a direct child whose creation identity is
    /// retained without a live child handle. This is the only way nested
    /// external leases reopen `v/<id>` or `p/<id>`: the parent name and child
    /// strong identity are checked before the handle is returned, and callers
    /// drop it again before publishing a path.
    pub(crate) fn open_verified_direct_child(
        &self,
        child: &WindowsDirectoryChildIdentity,
        name: &std::ffi::OsStr,
        label: &str,
    ) -> Result<WindowsDirectoryObjectGuard> {
        use windows_sys::Win32::Storage::FileSystem::{FILE_ADD_FILE, FILE_ADD_SUBDIRECTORY};

        ensure_windows_single_component(name)?;
        self.verify_direct_child(child, name, label)?;
        let logical_path = child.path().to_path_buf();
        let directory = open_windows_relative(
            &self.directory,
            name,
            true,
            WINDOWS_DIRECTORY_GUARD_ACCESS | FILE_ADD_SUBDIRECTORY | FILE_ADD_FILE,
            WINDOWS_NAMESPACE_SHARE,
        )
        .with_context(|| {
            format!(
                "无法通过持有父目录短暂打开{label}: {}",
                display_path(&logical_path)
            )
        })?;
        verify_windows_directory_child_identity_from_open(child, &directory, &logical_path, label)?;
        let opened = WindowsDirectoryObjectGuard {
            directory,
            identity: child.identity.clone(),
            logical_path,
        };
        self.verify_direct_child(child, name, label)?;
        Ok(opened)
    }

    /// Remove one descendant tree by reopening every component from this held
    /// root. The root identity is checked before the lease verifier, after its
    /// deletion snapshot, and before each handle-bound disposal. A host-root
    /// replacement therefore leaves both namespaces untouched by this cleanup
    /// transaction and prevents an evidence-producing caller from succeeding.
    pub(crate) fn remove_relative_tree_verified_with_snapshot<F, G>(
        &self,
        relative: &Path,
        verifier: F,
        snapshot_verifier: G,
    ) -> Result<bool>
    where
        F: FnOnce(&fs::File, &Path) -> Result<()>,
        G: FnOnce(&Path, &fs::File) -> Result<()>,
    {
        let mut snapshot_verifier = Some(snapshot_verifier);
        remove_windows_relative_tree_from_guard(
            self,
            relative,
            |_, target, leaf| {
                verify_windows_directory_object_at_path(self, self.path(), "外部受管目录清理根")?;
                verifier(leaf, target)
            },
            |snapshot_path, snapshot_leaf| {
                verify_windows_directory_object_at_path(self, self.path(), "外部受管目录清理根")?;
                if let Some(verifier) = snapshot_verifier.take() {
                    verifier(snapshot_path, snapshot_leaf)?;
                }
                verify_windows_directory_object_at_path(self, self.path(), "外部受管目录清理根")
            },
            || verify_windows_directory_object_at_path(self, self.path(), "外部受管目录清理根"),
        )
    }

    /// Read one direct ordinary file only through this creation-bound directory
    /// object and return the strong identity captured from that same read
    /// handle. This lets callers seal a manifest without reopening its parent
    /// through an absolute namespace path.
    pub(crate) fn read_file_with_identity(
        &self,
        name: &std::ffi::OsStr,
        label: &str,
    ) -> Result<(Vec<u8>, FileIdentity)> {
        ensure_windows_single_component(name)?;
        verify_windows_directory_object_at_path(self, self.path(), "目录文件读取父目录")?;
        let result = read_windows_file_from_directory_handle_with_identity(
            &self.directory,
            name,
            self.path(),
            label,
        )?;
        verify_windows_directory_object_at_path(self, self.path(), "目录文件读取父目录")?;
        Ok(result)
    }

    /// Create one new ordinary direct child file through this held parent,
    /// write and flush its complete contents, and return the identity captured
    /// from the *creation* handle.  A manifest publisher uses this instead of
    /// path-based `write_json`: a same-content replacement between an
    /// independent path write and its later seal would otherwise acquire the
    /// replacement's identity as if it were the created ownership record.
    ///
    /// `FILE_CREATE` refuses a pre-existing entry.  The bytes are read back
    /// through that exact handle after `sync_all`, then the name is reopened
    /// relative to the still-held parent and compared before publication.
    pub(crate) fn create_file_exclusive_and_seal(
        &self,
        name: &std::ffi::OsStr,
        bytes: &[u8],
        label: &str,
    ) -> Result<FileIdentity> {
        const MAX_MANIFEST_BYTES: usize = 1024 * 1024;

        ensure_windows_single_component(name)?;
        if bytes.len() > MAX_MANIFEST_BYTES {
            bail!(
                "{label}超过 1 MiB 安全上限: {}",
                display_path(&self.path().join(name))
            );
        }
        verify_windows_directory_object_at_path(self, self.path(), "原子文件创建父目录")?;
        let logical_path = self.path().join(name);
        let mut file = create_windows_relative_file(
            &self.directory,
            name,
            WINDOWS_FILE_CREATE_ACCESS,
            WINDOWS_NAMESPACE_SHARE,
        )
        .with_context(|| {
            format!(
                "无法原子创建{label}；文件可能已存在: {}",
                display_path(&logical_path)
            )
        })?;
        use std::io::Write as _;
        file.write_all(bytes)
            .with_context(|| format!("无法写入{label}: {}", display_path(&logical_path)))?;
        file.sync_all()
            .with_context(|| format!("无法同步{label}: {}", display_path(&logical_path)))?;

        let before = windows_handle_identity(&file, &logical_path, false, label)?;
        if before.len != bytes.len() as u64 {
            bail!(
                "{label}写入后的长度与预期不一致: {}",
                display_path(&logical_path)
            );
        }
        let observed = read_bytes_from_handle(&file, before.len, &logical_path, label)?;
        let after = windows_handle_identity(&file, &logical_path, false, label)?;
        if before != after || observed != bytes {
            bail!(
                "{label}在创建句柄封存期间发生变化: {}",
                display_path(&logical_path)
            );
        }

        // The seal's identity above intentionally comes from the creation
        // handle.  A separate parent-relative read only proves that the name
        // still resolves to that same just-created object before callers can
        // publish its path to a child process.
        let (named_bytes, named_identity) = self.read_file_with_identity(name, label)?;
        if named_bytes != bytes || named_identity != after {
            bail!(
                "{label}在创建后名称或内容发生替换: {}",
                display_path(&logical_path)
            );
        }
        Ok(after)
    }
}

/// Verify that `name` still resolves through `parent_guard` to the exact child
/// object held by `child_guard`.
///
/// The child is reopened relative to the held parent. The parent namespace is
/// checked before and after the child identity check, so a concurrent parent or
/// child replacement fails closed before the path can be published.
#[cfg(windows)]
pub(crate) fn verify_windows_directory_child_identity(
    parent_guard: &WindowsDirectoryObjectGuard,
    child_identity: &WindowsDirectoryChildIdentity,
    name: &std::ffi::OsStr,
    label: &str,
) -> Result<()> {
    ensure_windows_single_component(name)?;
    let expected_path = parent_guard.path().join(name);
    if child_identity.path() != expected_path {
        bail!(
            "{label}子目录身份路径与其持有父目录不匹配: child={} expected={}",
            display_path(child_identity.path()),
            display_path(&expected_path)
        );
    }
    verify_windows_directory_object_at_path(parent_guard, parent_guard.path(), "发布子目录父目录")?;
    let current = open_windows_relative(
        &parent_guard.directory,
        name,
        true,
        WINDOWS_DIRECTORY_GUARD_ACCESS,
        WINDOWS_GUARD_SHARE,
    )
    .with_context(|| {
        format!(
            "无法通过持有父目录重新打开{label}: {}",
            display_path(&expected_path)
        )
    })?;
    verify_windows_directory_child_identity_from_open(
        child_identity,
        &current,
        &expected_path,
        label,
    )?;
    // Recheck the namespace edges after child inspection: both objects must
    // still be named by the paths about to be published.
    verify_windows_directory_object_at_path(parent_guard, parent_guard.path(), "发布子目录父目录")?;
    let named = open_windows_relative(
        &parent_guard.directory,
        name,
        true,
        WINDOWS_DIRECTORY_GUARD_ACCESS,
        WINDOWS_GUARD_SHARE,
    )
    .with_context(|| {
        format!(
            "父目录复验后无法重新打开{label}: {}",
            display_path(&expected_path)
        )
    })?;
    verify_windows_directory_child_identity_from_open(
        child_identity,
        &named,
        &expected_path,
        label,
    )?;
    // The preceding child re-open was relative to the held parent. Check the
    // parent name one final time before publishing success, so a replacement
    // during the child re-open cannot leave the caller with a path below a
    // different parent namespace.
    verify_windows_directory_object_at_path(parent_guard, parent_guard.path(), "发布子目录父目录")
}

#[cfg(windows)]
impl WindowsDirectoryChildIdentity {
    /// The absolute namespace path bound to the identity captured at creation.
    pub(crate) fn path(&self) -> &Path {
        &self.logical_path
    }
}

#[cfg(windows)]
pub(crate) fn verify_windows_directory_child_identity_from_open(
    expected: &WindowsDirectoryChildIdentity,
    current_leaf: &fs::File,
    current_leaf_path: &Path,
    label: &str,
) -> Result<()> {
    let current_identity = windows_handle_identity(
        current_leaf,
        current_leaf_path,
        true,
        &format!("{label}当前 child 复验"),
    )?;
    if !same_windows_file_object(&expected.identity, &current_identity) {
        bail!(
            "{label}释放拒绝目录对象替换；当前 child 与创建时强身份不一致: 创建={} 当前={}",
            display_path(expected.path()),
            display_path(current_leaf_path)
        );
    }
    Ok(())
}

/// Hold one absolute, no-follow Windows directory object across an operation.
///
/// The guard allows `FILE_SHARE_DELETE`: a namespace rename or replacement is
/// not prevented by a share-mode lock, but is detected fail-closed when the
/// published path is re-opened relative to its held parent and its strong
/// identity is compared. This keeps cleanup compatible with DELETE-capable
/// handle opens while preserving reparse/replacement protection.
/// The fields stay private so callers cannot separate the handle from the
/// strong legacy and 128-bit identity captured from that same handle.
#[cfg(windows)]
pub(crate) fn hold_windows_directory_object(
    path: &Path,
    label: &str,
) -> Result<WindowsDirectoryObjectGuard> {
    if !path.is_absolute() {
        bail!(
            "{label}必须使用绝对路径持有 Windows 目录对象: {}",
            display_path(path)
        );
    }
    // Every public directory-object guard can atomically create a direct child.
    // Request only the parent-side capability that `NtCreateFile(FILE_CREATE)`
    // needs; ordinary cleanup opens continue to use the narrower guard helper.
    let directory = open_windows_directory_creation_parent(path)
        .with_context(|| format!("无法持有{label}的 Windows 目录对象: {}", display_path(path)))?;
    let identity = windows_handle_identity(&directory, path, true, label)?;
    Ok(WindowsDirectoryObjectGuard {
        directory,
        identity,
        logical_path: path.to_path_buf(),
    })
}

/// Atomically create one new ordinary directory below an absolute parent and
/// return a guard whose strong identity was captured from the same
/// `NtCreateFile(FILE_CREATE)` handle that created it.
///
/// Existing leaves are never adopted: `FILE_CREATE` returns an error on a
/// collision. The parent is opened with `FILE_ADD_SUBDIRECTORY` and held while
/// the relative no-follow create runs. Both the parent and returned leaf are
/// then re-opened by name, so a concurrent namespace replacement leaves the
/// newly created object as a safe orphan instead of publishing a substituted
/// directory.
#[cfg(windows)]
pub(crate) fn create_windows_directory_object_exclusive(
    parent_path: &Path,
    name: &std::ffi::OsStr,
    label: &str,
) -> Result<WindowsDirectoryObjectGuard> {
    if !parent_path.is_absolute() {
        bail!(
            "{label}必须使用绝对 Windows 父目录: {}",
            display_path(parent_path)
        );
    }

    let parent_directory =
        open_windows_directory_creation_parent(parent_path).with_context(|| {
            format!(
                "cannot hold the Windows parent directory for atomic {label} creation: {}",
                display_path(parent_path)
            )
        })?;
    let parent_identity = windows_handle_identity(
        &parent_directory,
        parent_path,
        true,
        "atomic Windows directory creation parent",
    )?;
    let parent_guard = WindowsDirectoryObjectGuard {
        directory: parent_directory,
        identity: parent_identity,
        logical_path: parent_path.to_path_buf(),
    };
    verify_windows_directory_object_at_path(
        &parent_guard,
        parent_path,
        "atomic Windows directory creation parent",
    )?;

    ensure_windows_single_component(name)?;
    let logical_path = parent_path.join(name);
    let directory = create_windows_relative_directory(
        &parent_guard.directory,
        name,
        WINDOWS_DIRECTORY_GUARD_ACCESS
            | windows_sys::Win32::Storage::FileSystem::FILE_ADD_SUBDIRECTORY,
        WINDOWS_GUARD_SHARE,
    )
    .with_context(|| {
        format!(
            "无法原子创建{label}；目录可能已存在: {}",
            display_path(&logical_path)
        )
    })?;
    let identity = windows_handle_identity(&directory, &logical_path, true, label)?;
    let guard = WindowsDirectoryObjectGuard {
        directory,
        identity,
        logical_path,
    };

    verify_windows_directory_object_at_path(
        &parent_guard,
        parent_path,
        "atomic Windows directory creation parent",
    )?;
    verify_windows_directory_object_at_path(&guard, guard.path(), label)?;
    Ok(guard)
}

/// Verify that a parent-handle-relative leaf still names the held directory.
///
/// Both the original handle and the current leaf must independently reproduce
/// the strong identity captured at creation. This fails closed if either the
/// legacy file index or the 128-bit file id is absent or has changed.
#[cfg(windows)]
pub(crate) fn verify_windows_directory_object(
    guard: &WindowsDirectoryObjectGuard,
    current_leaf: &fs::File,
    current_leaf_path: &Path,
    label: &str,
) -> Result<()> {
    if !current_leaf_path.is_absolute() {
        bail!(
            "{label}释放复验必须使用绝对路径: {}",
            display_path(current_leaf_path)
        );
    }

    let held_identity = windows_handle_identity(
        &guard.directory,
        &guard.logical_path,
        true,
        &format!("{label}持有对象复验"),
    )?;
    if !same_windows_file_object(&guard.identity, &held_identity) {
        bail!(
            "{label}持有的 Windows 目录对象强身份在释放前发生变化: {}",
            display_path(&guard.logical_path)
        );
    }

    let current_identity = windows_handle_identity(
        current_leaf,
        current_leaf_path,
        true,
        &format!("{label}当前 leaf 复验"),
    )?;
    if !same_windows_file_object(&guard.identity, &current_identity) {
        bail!(
            "{label}释放拒绝目录对象替换；当前 leaf 与创建时强身份不一致: 创建={} 当前={}",
            display_path(&guard.logical_path),
            display_path(current_leaf_path)
        );
    }
    Ok(())
}

/// Re-open an absolute leaf without following a reparse point and compare it
/// with a directory object held from creation. Callers use this immediately
/// before publishing a path to a child process, so a clone replacement cannot
/// receive a managed TEMP or target path merely because its manifest matches.
#[cfg(windows)]
pub(crate) fn verify_windows_directory_object_at_path(
    guard: &WindowsDirectoryObjectGuard,
    current_path: &Path,
    label: &str,
) -> Result<()> {
    let current = open_windows_directory_guard(current_path).with_context(|| {
        format!(
            "无法重新打开{label}当前 Windows 目录对象: {}",
            display_path(current_path)
        )
    })?;
    verify_windows_directory_object(guard, &current, current_path, label)
}

#[cfg(windows)]
fn open_windows_directory_guard(path: &Path) -> Result<fs::File> {
    use std::os::windows::fs::OpenOptionsExt as _;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
    };

    let directory = fs::OpenOptions::new()
        .access_mode(WINDOWS_DIRECTORY_GUARD_ACCESS)
        .share_mode(WINDOWS_GUARD_SHARE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .with_context(|| format!("无法锁定受管状态目录身份: {}", display_path(path)))?;
    let metadata = directory
        .metadata()
        .with_context(|| format!("无法读取已锁定受管状态目录元数据: {}", display_path(path)))?;
    ensure_real_directory_metadata(path, &metadata)?;
    Ok(directory)
}

#[cfg(windows)]
fn open_windows_directory_creation_parent(path: &Path) -> Result<fs::File> {
    use std::os::windows::fs::OpenOptionsExt as _;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ADD_FILE, FILE_ADD_SUBDIRECTORY, FILE_FLAG_BACKUP_SEMANTICS,
        FILE_FLAG_OPEN_REPARSE_POINT,
    };

    let directory = fs::OpenOptions::new()
        .access_mode(WINDOWS_DIRECTORY_GUARD_ACCESS | FILE_ADD_SUBDIRECTORY | FILE_ADD_FILE)
        .share_mode(WINDOWS_GUARD_SHARE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .with_context(|| {
            format!(
                "cannot open atomic directory-creation parent: {}",
                display_path(path)
            )
        })?;
    let metadata = directory.metadata().with_context(|| {
        format!(
            "cannot read atomic directory-creation parent metadata: {}",
            display_path(path)
        )
    })?;
    ensure_real_directory_metadata(path, &metadata)?;
    Ok(directory)
}

#[cfg(windows)]
const WINDOWS_DIRECTORY_GUARD_ACCESS: u32 =
    windows_sys::Win32::Storage::FileSystem::FILE_LIST_DIRECTORY
        | windows_sys::Win32::Storage::FileSystem::FILE_TRAVERSE
        | windows_sys::Win32::Storage::FileSystem::FILE_READ_ATTRIBUTES
        | windows_sys::Win32::Storage::FileSystem::SYNCHRONIZE;

#[cfg(windows)]
const WINDOWS_DIRECTORY_DELETE_ACCESS: u32 =
    WINDOWS_DIRECTORY_GUARD_ACCESS | windows_sys::Win32::Storage::FileSystem::DELETE;

#[cfg(windows)]
const WINDOWS_FILE_DELETE_ACCESS: u32 = windows_sys::Win32::Storage::FileSystem::DELETE
    | windows_sys::Win32::Storage::FileSystem::FILE_READ_ATTRIBUTES
    | windows_sys::Win32::Storage::FileSystem::SYNCHRONIZE;

#[cfg(windows)]
const WINDOWS_FILE_GUARD_ACCESS: u32 = windows_sys::Win32::Storage::FileSystem::FILE_READ_ATTRIBUTES
    | windows_sys::Win32::Storage::FileSystem::SYNCHRONIZE;

#[cfg(windows)]
const WINDOWS_FILE_READ_ACCESS: u32 = windows_sys::Win32::Storage::FileSystem::FILE_READ_DATA
    | windows_sys::Win32::Storage::FileSystem::FILE_READ_ATTRIBUTES
    | windows_sys::Win32::Storage::FileSystem::SYNCHRONIZE;

#[cfg(windows)]
const WINDOWS_FILE_CREATE_ACCESS: u32 = windows_sys::Win32::Storage::FileSystem::FILE_READ_DATA
    | windows_sys::Win32::Storage::FileSystem::FILE_WRITE_DATA
    | windows_sys::Win32::Storage::FileSystem::FILE_READ_ATTRIBUTES
    | windows_sys::Win32::Storage::FileSystem::SYNCHRONIZE;

// Probe cleanup is bound to the same newly-created file handle, so this is
// the one narrow place that also needs DELETE. Directory creation parents do
// not receive DELETE or WRITE_DAC.
#[cfg(windows)]
const WINDOWS_FILE_PROBE_ACCESS: u32 =
    WINDOWS_FILE_CREATE_ACCESS | windows_sys::Win32::Storage::FileSystem::DELETE;

#[cfg(windows)]
// Namespace changes are deliberately allowed and detected by the mandatory
// handle-relative strong-identity revalidation. A share-mode lock would both
// contradict that contract and block the DELETE-capable cleanup handles.
const WINDOWS_NAMESPACE_SHARE: u32 = windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ
    | windows_sys::Win32::Storage::FileSystem::FILE_SHARE_WRITE
    | windows_sys::Win32::Storage::FileSystem::FILE_SHARE_DELETE;

#[cfg(windows)]
const WINDOWS_GUARD_SHARE: u32 = WINDOWS_NAMESPACE_SHARE;

#[cfg(windows)]
const WINDOWS_QUERY_BUFFER_BYTES: usize = 64 * 1024;

#[cfg(windows)]
struct WindowsDirectoryHandle {
    file: fs::File,
    identity: FileIdentity,
    logical_path: PathBuf,
    name: Option<std::ffi::OsString>,
}

/// Handle-relative counterpart of `remove_managed_dir_all_windows` for a
/// caller that already owns the external authority root. It never resolves
/// that root from a path while traversing or deleting a lease descendant.
#[cfg(windows)]
fn remove_windows_relative_tree_from_guard<F, G, H>(
    root_guard: &WindowsDirectoryObjectGuard,
    relative: &Path,
    before_remove: F,
    mut after_snapshot: G,
    mut before_delete: H,
) -> Result<bool>
where
    F: FnOnce(&Path, &Path, &fs::File) -> Result<()>,
    G: FnMut(&Path, &fs::File) -> Result<()>,
    H: FnMut() -> Result<()>,
{
    let components = normal_components(relative)?;
    if components.is_empty() {
        bail!("外部受管目录删除路径不能为空");
    }
    before_delete()?;
    let root_file = root_guard.directory.try_clone().with_context(|| {
        format!(
            "无法复制外部受管目录清理根句柄: {}",
            display_path(root_guard.path())
        )
    })?;
    let mut chain = vec![WindowsDirectoryHandle {
        file: root_file,
        identity: root_guard.identity.clone(),
        logical_path: root_guard.path().to_path_buf(),
        name: None,
    }];
    let names = components
        .into_iter()
        .map(std::ffi::OsString::from)
        .collect::<Vec<_>>();

    for (index, name) in names.iter().enumerate() {
        before_delete()?;
        let logical_path = chain
            .last()
            .expect("external parent handle exists")
            .logical_path
            .join(name);
        let opened = open_windows_relative(
            &chain.last().expect("external parent handle exists").file,
            name,
            true,
            if index + 1 == names.len() {
                WINDOWS_DIRECTORY_DELETE_ACCESS
            } else {
                WINDOWS_DIRECTORY_GUARD_ACCESS
            },
            WINDOWS_NAMESPACE_SHARE,
        );
        let file = match opened {
            Ok(file) => file,
            Err(error) if windows_not_found(&error) => return Ok(false),
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "无法按持有外部根目录句柄打开受管删除分量: {}",
                        display_path(&logical_path)
                    )
                });
            }
        };
        let identity = windows_handle_identity(&file, &logical_path, true, "外部受管目录删除目录")?;
        ensure_same_windows_volume(
            &chain
                .last()
                .expect("external parent handle exists")
                .identity,
            &identity,
            &logical_path,
        )?;
        chain.push(WindowsDirectoryHandle {
            file,
            identity,
            logical_path,
            name: Some(name.clone()),
        });
    }

    let leaf_index = chain.len() - 1;
    let parent_index = leaf_index - 1;
    let parent = chain[parent_index].logical_path.clone();
    let target = chain[leaf_index].logical_path.clone();
    before_delete()?;
    before_remove(&parent, &target, &chain[leaf_index].file)?;
    before_delete()?;
    revalidate_windows_directory_chain(&chain)?;
    empty_windows_directory_handle(
        &chain[leaf_index].file,
        &chain[leaf_index].identity,
        &target,
        &mut after_snapshot,
        &mut before_delete,
    )?;
    before_delete()?;
    revalidate_windows_directory_chain(&chain)?;
    let leaf_name = chain[leaf_index]
        .name
        .clone()
        .expect("external leaf has a relative name");
    before_delete()?;
    delete_windows_bound_entry(
        &chain[parent_index].file,
        &leaf_name,
        &chain[leaf_index].file,
        &chain[leaf_index].identity,
        true,
        &target,
    )?;
    drop(chain.pop().expect("external leaf handle exists"));
    before_delete()?;
    ensure_windows_name_absent(&chain[parent_index].file, &leaf_name, true, &target)?;
    before_delete()?;
    drop(chain);
    Ok(true)
}

#[cfg(windows)]
#[derive(Debug, Clone, PartialEq, Eq)]
struct WindowsDirectoryEntry {
    name: std::ffi::OsString,
    name_wide: Vec<u16>,
    file_id: u64,
    attributes: u32,
}

#[cfg(windows)]
fn windows_nt_error(status: windows_sys::Win32::Foundation::NTSTATUS) -> io::Error {
    let code = unsafe { windows_sys::Win32::Foundation::RtlNtStatusToDosError(status) };
    io::Error::from_raw_os_error(code as i32)
}

#[cfg(windows)]
fn windows_not_found(error: &io::Error) -> bool {
    matches!(error.raw_os_error(), Some(2 | 3))
}

#[cfg(windows)]
fn open_windows_relative(
    parent: &fs::File,
    name: &std::ffi::OsStr,
    directory: bool,
    desired_access: u32,
    share_access: u32,
) -> io::Result<fs::File> {
    open_windows_relative_with_disposition(
        parent,
        name,
        directory,
        desired_access,
        share_access,
        windows_sys::Wdk::Storage::FileSystem::FILE_OPEN,
    )
}

#[cfg(windows)]
fn create_windows_relative_directory(
    parent: &fs::File,
    name: &std::ffi::OsStr,
    desired_access: u32,
    share_access: u32,
) -> io::Result<fs::File> {
    open_windows_relative_with_disposition(
        parent,
        name,
        true,
        desired_access,
        share_access,
        windows_sys::Wdk::Storage::FileSystem::FILE_CREATE,
    )
}

#[cfg(windows)]
fn create_windows_relative_file(
    parent: &fs::File,
    name: &std::ffi::OsStr,
    desired_access: u32,
    share_access: u32,
) -> io::Result<fs::File> {
    open_windows_relative_with_disposition(
        parent,
        name,
        false,
        desired_access,
        share_access,
        windows_sys::Wdk::Storage::FileSystem::FILE_CREATE,
    )
}

#[cfg(windows)]
fn ensure_windows_single_component(name: &std::ffi::OsStr) -> Result<()> {
    use std::os::windows::ffi::OsStrExt as _;

    let components = normal_components(Path::new(name))?;
    if components.len() != 1 || components[0] != name {
        bail!("原子目录创建使用了不安全的相对 Windows 分量");
    }
    let wide: Vec<u16> = name.encode_wide().collect();
    if wide.is_empty()
        || wide.ends_with(&[32])
        || wide.ends_with(&[46])
        || wide.iter().any(|value| matches!(*value, 0 | 47 | 58 | 92))
    {
        bail!("原子目录创建使用了不安全的相对 Windows 分量");
    }
    Ok(())
}

#[cfg(windows)]
fn open_windows_relative_with_disposition(
    parent: &fs::File,
    name: &std::ffi::OsStr,
    directory: bool,
    desired_access: u32,
    share_access: u32,
    create_disposition: u32,
) -> io::Result<fs::File> {
    use std::mem::size_of;
    use std::os::windows::ffi::OsStrExt as _;
    use std::os::windows::io::{AsRawHandle as _, FromRawHandle as _};
    use windows_sys::Wdk::Foundation::OBJECT_ATTRIBUTES;
    use windows_sys::Wdk::Storage::FileSystem::{
        FILE_DIRECTORY_FILE, FILE_NON_DIRECTORY_FILE, FILE_OPEN_FOR_BACKUP_INTENT,
        FILE_OPEN_REPARSE_POINT, FILE_SYNCHRONOUS_IO_NONALERT, NtCreateFile,
    };
    use windows_sys::Win32::Foundation::{
        HANDLE, OBJ_CASE_INSENSITIVE, OBJ_DONT_REPARSE, UNICODE_STRING,
    };
    use windows_sys::Win32::System::IO::IO_STATUS_BLOCK;

    let mut wide: Vec<u16> = name.encode_wide().collect();
    if wide.is_empty() || wide.iter().any(|value| matches!(*value, 0 | 47 | 58 | 92)) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "原子目录创建使用了不安全的相对 Windows 分量",
        ));
    }
    let byte_len = wide
        .len()
        .checked_mul(size_of::<u16>())
        .and_then(|value| u16::try_from(value).ok())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "Windows name is too long"))?;
    let unicode = UNICODE_STRING {
        Length: byte_len,
        MaximumLength: byte_len,
        Buffer: wide.as_mut_ptr(),
    };
    let attributes = OBJECT_ATTRIBUTES {
        Length: size_of::<OBJECT_ATTRIBUTES>() as u32,
        RootDirectory: parent.as_raw_handle() as HANDLE,
        ObjectName: &unicode,
        Attributes: OBJ_CASE_INSENSITIVE | OBJ_DONT_REPARSE,
        SecurityDescriptor: std::ptr::null(),
        SecurityQualityOfService: std::ptr::null(),
    };
    let mut handle: HANDLE = std::ptr::null_mut();
    let mut io_status = IO_STATUS_BLOCK::default();
    let type_option = if directory {
        FILE_DIRECTORY_FILE
    } else {
        FILE_NON_DIRECTORY_FILE
    };
    let status = unsafe {
        NtCreateFile(
            &mut handle,
            desired_access,
            &attributes,
            &mut io_status,
            std::ptr::null(),
            0,
            share_access,
            create_disposition,
            type_option
                | FILE_OPEN_REPARSE_POINT
                | FILE_OPEN_FOR_BACKUP_INTENT
                | FILE_SYNCHRONOUS_IO_NONALERT,
            std::ptr::null(),
            0,
        )
    };
    // NT_SUCCESS is `status >= 0`, not equality with STATUS_SUCCESS.  The
    // synchronous create path still requires a concrete handle below.
    if status < 0 {
        return Err(windows_nt_error(status));
    }
    if handle.is_null() {
        return Err(io::Error::other("NtCreateFile succeeded without a handle"));
    }
    // `IO_STATUS_BLOCK.Information` reports the create disposition result.
    // NT's FILE_CREATED value is 2.  Do not accept an informational success
    // that opened/adopted a pre-existing path when the caller asked FILE_CREATE.
    if create_disposition == windows_sys::Wdk::Storage::FileSystem::FILE_CREATE
        && io_status.Information != 2
    {
        drop(unsafe { fs::File::from_raw_handle(handle) });
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "NtCreateFile(FILE_CREATE) did not create a new object",
        ));
    }
    Ok(unsafe { fs::File::from_raw_handle(handle) })
}

#[cfg(windows)]
fn windows_handle_identity(
    file: &fs::File,
    logical_path: &Path,
    directory: bool,
    label: &str,
) -> Result<FileIdentity> {
    let metadata = file
        .metadata()
        .with_context(|| format!("无法读取{label}句柄元数据: {}", display_path(logical_path)))?;
    if is_link_or_reparse(&metadata)
        || (directory && !metadata.file_type().is_dir())
        || (!directory && !metadata.file_type().is_file())
    {
        bail!(
            "{label}不是预期的普通{}: {}",
            if directory { "目录" } else { "文件" },
            display_path(logical_path)
        );
    }
    let identity = file_identity_from_handle(file, &metadata, logical_path, label)?;
    if !has_strong_file_identity(&identity) {
        bail!(
            "{label}缺少 Windows 强文件身份: {}",
            display_path(logical_path)
        );
    }
    Ok(identity)
}

#[cfg(windows)]
fn same_windows_file_object(expected: &FileIdentity, actual: &FileIdentity) -> bool {
    has_strong_file_identity(expected)
        && has_strong_file_identity(actual)
        && expected.volume_serial_number == actual.volume_serial_number
        && expected.file_index == actual.file_index
        && expected.volume_serial_number_64 == actual.volume_serial_number_64
        && expected.file_id_128 == actual.file_id_128
}

#[cfg(windows)]
fn ensure_same_windows_volume(
    parent: &FileIdentity,
    child: &FileIdentity,
    logical_path: &Path,
) -> Result<()> {
    if parent.volume_serial_number != child.volume_serial_number
        || parent.volume_serial_number_64 != child.volume_serial_number_64
    {
        bail!(
            "受管状态删除分量跨越了父目录卷身份: {}",
            display_path(logical_path)
        );
    }
    Ok(())
}

#[cfg(windows)]
fn revalidate_windows_directory_chain(chain: &[WindowsDirectoryHandle]) -> Result<()> {
    for index in 1..chain.len() {
        let current = &chain[index];
        let reopened = open_windows_relative(
            &chain[index - 1].file,
            current.name.as_deref().expect("child has a name"),
            true,
            WINDOWS_DIRECTORY_GUARD_ACCESS,
            WINDOWS_NAMESPACE_SHARE,
        )
        .with_context(|| {
            format!(
                "受管状态删除祖先名称已消失或不可重开: {}",
                display_path(&current.logical_path)
            )
        })?;
        let actual = windows_handle_identity(
            &reopened,
            &current.logical_path,
            true,
            "受管状态删除祖先复验",
        )?;
        if !same_windows_file_object(&current.identity, &actual) {
            bail!(
                "受管状态删除祖先发生身份替换: {}",
                display_path(&current.logical_path)
            );
        }
    }
    Ok(())
}

#[cfg(windows)]
fn query_windows_directory(
    directory: &fs::File,
    logical_path: &Path,
) -> Result<Vec<WindowsDirectoryEntry>> {
    use std::mem::size_of;
    use std::os::windows::io::AsRawHandle as _;
    use windows_sys::Wdk::Storage::FileSystem::{
        FileIdBothDirectoryInformation, NtQueryDirectoryFile,
    };
    use windows_sys::Win32::Foundation::{STATUS_NO_MORE_FILES, STATUS_SUCCESS};
    use windows_sys::Win32::System::IO::IO_STATUS_BLOCK;

    let mut storage = vec![0_u64; WINDOWS_QUERY_BUFFER_BYTES / size_of::<u64>()];
    let capacity = storage.len() * size_of::<u64>();
    let mut restart = true;
    let mut entries = Vec::new();
    loop {
        let mut io_status = IO_STATUS_BLOCK::default();
        let status = unsafe {
            NtQueryDirectoryFile(
                directory.as_raw_handle() as _,
                std::ptr::null_mut(),
                None,
                std::ptr::null(),
                &mut io_status,
                storage.as_mut_ptr().cast(),
                capacity as u32,
                FileIdBothDirectoryInformation,
                false,
                std::ptr::null(),
                restart,
            )
        };
        if status == STATUS_NO_MORE_FILES {
            break;
        }
        if status != STATUS_SUCCESS {
            return Err(windows_nt_error(status)).with_context(|| {
                format!(
                    "无法从句柄枚举受管状态删除目录: {} (NTSTATUS=0x{:08x})",
                    display_path(logical_path),
                    status as u32
                )
            });
        }
        let used = io_status.Information;
        if used == 0 || used > capacity {
            bail!(
                "NtQueryDirectoryFile 返回无效字节数 {used}/{capacity}: {}",
                display_path(logical_path)
            );
        }
        let bytes = unsafe { std::slice::from_raw_parts(storage.as_ptr().cast::<u8>(), used) };
        entries.extend(parse_windows_directory_records(bytes)?);
        restart = false;
    }
    entries.sort_by(|left, right| {
        left.name_wide
            .cmp(&right.name_wide)
            .then(left.file_id.cmp(&right.file_id))
            .then(left.attributes.cmp(&right.attributes))
    });
    if entries
        .windows(2)
        .any(|pair| pair[0].name_wide == pair[1].name_wide)
    {
        bail!(
            "受管状态删除目录枚举出现重复名称: {}",
            display_path(logical_path)
        );
    }
    Ok(entries)
}

#[cfg(windows)]
fn parse_windows_directory_records(bytes: &[u8]) -> Result<Vec<WindowsDirectoryEntry>> {
    use std::mem::{align_of, offset_of};
    use std::os::windows::ffi::OsStringExt as _;
    use windows_sys::Wdk::Storage::FileSystem::FILE_ID_BOTH_DIR_INFORMATION;

    const NAME_OFFSET: usize = offset_of!(FILE_ID_BOTH_DIR_INFORMATION, FileName);
    const NEXT_OFFSET: usize = offset_of!(FILE_ID_BOTH_DIR_INFORMATION, NextEntryOffset);
    const ATTRIBUTES_OFFSET: usize = offset_of!(FILE_ID_BOTH_DIR_INFORMATION, FileAttributes);
    const NAME_LENGTH_OFFSET: usize = offset_of!(FILE_ID_BOTH_DIR_INFORMATION, FileNameLength);
    const FILE_ID_OFFSET: usize = offset_of!(FILE_ID_BOTH_DIR_INFORMATION, FileId);

    let mut entries = Vec::new();
    let mut record_offset = 0usize;
    loop {
        if !record_offset.is_multiple_of(align_of::<FILE_ID_BOTH_DIR_INFORMATION>()) {
            bail!("Windows 目录枚举记录未按结构体对齐");
        }
        let remaining = bytes
            .len()
            .checked_sub(record_offset)
            .ok_or_else(|| anyhow::anyhow!("Windows 目录枚举记录偏移越界"))?;
        if remaining < NAME_OFFSET {
            bail!("Windows 目录枚举记录短于固定头");
        }
        let base = unsafe { bytes.as_ptr().add(record_offset) };
        let next =
            unsafe { std::ptr::read_unaligned(base.add(NEXT_OFFSET).cast::<u32>()) as usize };
        let attributes =
            unsafe { std::ptr::read_unaligned(base.add(ATTRIBUTES_OFFSET).cast::<u32>()) };
        let name_bytes = unsafe {
            std::ptr::read_unaligned(base.add(NAME_LENGTH_OFFSET).cast::<u32>()) as usize
        };
        let file_id =
            unsafe { std::ptr::read_unaligned(base.add(FILE_ID_OFFSET).cast::<i64>()) as u64 };
        if name_bytes == 0 || name_bytes % 2 != 0 {
            bail!("Windows 目录枚举名称长度无效: {name_bytes}");
        }
        let name_end_in_record = NAME_OFFSET
            .checked_add(name_bytes)
            .ok_or_else(|| anyhow::anyhow!("Windows 目录枚举名称长度溢出"))?;
        let record_end = if next == 0 {
            bytes.len()
        } else {
            if next % 8 != 0 || next < name_end_in_record {
                bail!("Windows 目录枚举 NextEntryOffset 无效: {next}");
            }
            record_offset
                .checked_add(next)
                .ok_or_else(|| anyhow::anyhow!("Windows 目录枚举下一记录偏移溢出"))?
        };
        if record_end > bytes.len()
            || record_offset
                .checked_add(name_end_in_record)
                .is_none_or(|name_end| name_end > record_end)
        {
            bail!("Windows 目录枚举名称越过有效返回字节");
        }
        let name_start = record_offset + NAME_OFFSET;
        let name_end = name_start + name_bytes;
        let mut wide = Vec::with_capacity(name_bytes / 2);
        for chunk in bytes[name_start..name_end].as_chunks::<2>().0 {
            wide.push(u16::from_ne_bytes([chunk[0], chunk[1]]));
        }
        if wide != [46] && wide != [46, 46] {
            if wide.iter().any(|value| matches!(*value, 0 | 47 | 58 | 92)) {
                bail!("Windows 目录枚举返回不安全的单组件名称");
            }
            entries.push(WindowsDirectoryEntry {
                name: std::ffi::OsString::from_wide(&wide),
                name_wide: wide,
                file_id,
                attributes,
            });
        }
        if next == 0 {
            break;
        }
        record_offset = record_end;
    }
    Ok(entries)
}

#[cfg(windows)]
fn stable_windows_directory_snapshot(
    directory: &fs::File,
    logical_path: &Path,
) -> Result<Vec<WindowsDirectoryEntry>> {
    let first = query_windows_directory(directory, logical_path)?;
    let second = query_windows_directory(directory, logical_path)?;
    if first != second {
        bail!(
            "受管状态删除目录在稳定枚举期间发生变化: {}",
            display_path(logical_path)
        );
    }
    Ok(second)
}

#[cfg(windows)]
fn empty_windows_directory_handle<G, H>(
    directory: &fs::File,
    directory_identity: &FileIdentity,
    logical_path: &Path,
    after_snapshot: &mut G,
    before_delete: &mut H,
) -> Result<()>
where
    G: FnMut(&Path, &fs::File) -> Result<()>,
    H: FnMut() -> Result<()>,
{
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT,
    };

    let entries = stable_windows_directory_snapshot(directory, logical_path)?;
    after_snapshot(logical_path, directory)?;
    for entry in entries {
        before_delete()?;
        let child_path = logical_path.join(&entry.name);
        if entry.attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            bail!(
                "受管状态删除拒绝链接/reparse 条目: {}",
                display_path(&child_path)
            );
        }
        let is_directory = entry.attributes & FILE_ATTRIBUTE_DIRECTORY != 0;
        let child = open_windows_relative(
            directory,
            &entry.name,
            is_directory,
            if is_directory {
                WINDOWS_DIRECTORY_DELETE_ACCESS
            } else {
                WINDOWS_FILE_DELETE_ACCESS
            },
            WINDOWS_NAMESPACE_SHARE,
        )
        .with_context(|| {
            format!(
                "无法按目录句柄打开受管状态删除条目: {}",
                display_path(&child_path)
            )
        })?;
        let child_identity =
            windows_handle_identity(&child, &child_path, is_directory, "受管状态删除条目")?;
        ensure_same_windows_volume(directory_identity, &child_identity, &child_path)?;
        if child_identity.file_index != Some(entry.file_id) {
            bail!(
                "受管状态删除条目枚举 ID 与句柄 ID 不一致: {}",
                display_path(&child_path)
            );
        }
        revalidate_windows_named_entry(
            directory,
            &entry.name,
            &child_identity,
            is_directory,
            &child_path,
        )?;
        if is_directory {
            empty_windows_directory_handle(
                &child,
                &child_identity,
                &child_path,
                after_snapshot,
                before_delete,
            )?;
        }
        before_delete()?;
        delete_windows_bound_entry(
            directory,
            &entry.name,
            &child,
            &child_identity,
            is_directory,
            &child_path,
        )?;
        drop(child);
        ensure_windows_name_absent(directory, &entry.name, is_directory, &child_path)?;
    }
    before_delete()?;
    let refill = query_windows_directory(directory, logical_path)?;
    if !refill.is_empty() {
        bail!(
            "受管状态删除检测到并发 refill；保留 orphan: {}",
            display_path(logical_path)
        );
    }
    Ok(())
}

#[cfg(windows)]
fn revalidate_windows_named_entry(
    parent: &fs::File,
    name: &std::ffi::OsStr,
    expected: &FileIdentity,
    directory: bool,
    logical_path: &Path,
) -> Result<()> {
    let reopened = open_windows_relative(
        parent,
        name,
        directory,
        if directory {
            WINDOWS_DIRECTORY_GUARD_ACCESS
        } else {
            WINDOWS_FILE_GUARD_ACCESS
        },
        WINDOWS_NAMESPACE_SHARE,
    )
    .with_context(|| {
        format!(
            "受管状态删除条目名称在删除前已消失或换位: {}",
            display_path(logical_path)
        )
    })?;
    let actual =
        windows_handle_identity(&reopened, logical_path, directory, "受管状态删除条目复验")?;
    if !same_windows_file_object(expected, &actual) {
        bail!(
            "受管状态删除条目名称指向了 replacement: {}",
            display_path(logical_path)
        );
    }
    Ok(())
}

#[cfg(windows)]
fn delete_windows_bound_entry(
    parent: &fs::File,
    name: &std::ffi::OsStr,
    entry: &fs::File,
    identity: &FileIdentity,
    directory: bool,
    logical_path: &Path,
) -> Result<()> {
    // The caller already holds `entry` with DELETE access.  A separate
    // name-reopen immediately before disposition catches an adversarial
    // replacement; after that, the delete itself is bound to this exact
    // handle, never to the current pathname.
    revalidate_windows_named_entry(parent, name, identity, directory, logical_path)?;
    delete_windows_file_by_handle(entry).with_context(|| {
        format!(
            "无法按句柄删除受管状态{}: {}",
            if directory { "目录" } else { "文件" },
            display_path(logical_path)
        )
    })
}

#[cfg(windows)]
fn ensure_windows_name_absent(
    parent: &fs::File,
    name: &std::ffi::OsStr,
    directory: bool,
    logical_path: &Path,
) -> Result<()> {
    match open_windows_relative(
        parent,
        name,
        directory,
        if directory {
            WINDOWS_DIRECTORY_GUARD_ACCESS
        } else {
            WINDOWS_FILE_GUARD_ACCESS
        },
        WINDOWS_NAMESPACE_SHARE,
    ) {
        Err(error) if windows_not_found(&error) => Ok(()),
        Ok(_) => bail!(
            "受管状态删除后同名 replacement 仍存在；保留并失败: {}",
            display_path(logical_path)
        ),
        Err(error) => Err(error).with_context(|| {
            format!(
                "无法确认受管状态条目已从父目录移除: {}",
                display_path(logical_path)
            )
        }),
    }
}

#[cfg(windows)]
fn delete_windows_file_by_handle(file: &fs::File) -> io::Result<()> {
    use std::os::windows::io::AsRawHandle as _;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_DISPOSITION_FLAG_DELETE, FILE_DISPOSITION_FLAG_IGNORE_READONLY_ATTRIBUTE,
        FILE_DISPOSITION_FLAG_POSIX_SEMANTICS, FILE_DISPOSITION_INFO_EX, FileDispositionInfoEx,
        SetFileInformationByHandle,
    };

    let information = FILE_DISPOSITION_INFO_EX {
        Flags: FILE_DISPOSITION_FLAG_DELETE
            | FILE_DISPOSITION_FLAG_POSIX_SEMANTICS
            | FILE_DISPOSITION_FLAG_IGNORE_READONLY_ATTRIBUTE,
    };
    let deleted = unsafe {
        SetFileInformationByHandle(
            file.as_raw_handle() as _,
            FileDispositionInfoEx,
            (&raw const information).cast(),
            std::mem::size_of::<FILE_DISPOSITION_INFO_EX>() as u32,
        )
    };
    if deleted == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// Read one ordinary child file using only a held directory handle. This is
/// the manifest verifier bridge for Windows managed-state deletion; callers
/// must not reopen the directory by path between verification and cleanup.
#[cfg(windows)]
pub(crate) fn read_windows_file_from_directory_handle(
    directory: &fs::File,
    name: &std::ffi::OsStr,
    logical_directory: &Path,
    label: &str,
) -> Result<Vec<u8>> {
    Ok(read_windows_file_from_directory_handle_with_identity(
        directory,
        name,
        logical_directory,
        label,
    )?
    .0)
}

/// Read one ordinary child file using only a held directory handle and return
/// the strong identity observed from that same read handle.
#[cfg(windows)]
pub(crate) fn read_windows_file_from_directory_handle_with_identity(
    directory: &fs::File,
    name: &std::ffi::OsStr,
    logical_directory: &Path,
    label: &str,
) -> Result<(Vec<u8>, FileIdentity)> {
    let logical_path = logical_directory.join(name);
    let file = open_windows_relative(
        directory,
        name,
        false,
        WINDOWS_FILE_READ_ACCESS,
        WINDOWS_NAMESPACE_SHARE,
    )
    .with_context(|| {
        format!(
            "无法从受管目录句柄打开{label}: {}",
            display_path(&logical_path)
        )
    })?;
    let before = windows_handle_identity(&file, &logical_path, false, label)?;
    if before.len > 1024 * 1024 {
        bail!(
            "{label}超过 1 MiB 安全上限: {}",
            display_path(&logical_path)
        );
    }
    let bytes = read_bytes_from_handle(&file, before.len, &logical_path, label)?;
    let after = windows_handle_identity(&file, &logical_path, false, label)?;
    if before != after || after.len != bytes.len() as u64 {
        bail!(
            "{label}在句柄读取期间发生变化: {}",
            display_path(&logical_path)
        );
    }
    let reopened = open_windows_relative(
        directory,
        name,
        false,
        windows_sys::Win32::Storage::FileSystem::FILE_READ_ATTRIBUTES
            | windows_sys::Win32::Storage::FileSystem::SYNCHRONIZE,
        WINDOWS_NAMESPACE_SHARE,
    )
    .with_context(|| {
        format!(
            "{label}在句柄读取后无法复验: {}",
            display_path(&logical_path)
        )
    })?;
    let named = windows_handle_identity(&reopened, &logical_path, false, label)?;
    if !same_windows_file_object(&after, &named) {
        bail!(
            "{label}名称在句柄读取后发生替换: {}",
            display_path(&logical_path)
        );
    }
    Ok((bytes, after))
}

/// Result of the non-destructive state-write capability probe.
#[derive(Debug, serde::Serialize)]
pub struct StateWriteProbe {
    pub state_dir_present: bool,
    pub probed: bool,
    pub writable: bool,
    pub path: Option<String>,
    pub error: Option<String>,
}

/// Read-only status used unless the caller explicitly requests the transient
/// write/remove probe.
pub fn state_write_probe_not_requested(root: &Path) -> StateWriteProbe {
    match managed_state_root(root, false) {
        Ok(Some(_)) => StateWriteProbe {
            state_dir_present: true,
            probed: false,
            writable: false,
            path: None,
            error: None,
        },
        Ok(None) => StateWriteProbe {
            state_dir_present: false,
            probed: false,
            writable: false,
            path: None,
            error: None,
        },
        Err(error) => StateWriteProbe {
            state_dir_present: true,
            probed: false,
            writable: false,
            path: None,
            error: Some(format!("{error:#}")),
        },
    }
}

/// Probe whether this process can write workspace state right now.
///
/// Restricted host sandboxes deny `.RaymanCodingSkill/` lock and state writes
/// with ACL errors that otherwise surface only mid-transaction. The probe
/// writes and removes one transient file inside an existing state root; a
/// workspace without a state root is reported unprobed instead of mutated.
pub fn state_write_probe(root: &Path) -> StateWriteProbe {
    match managed_state_root(root, false) {
        Ok(Some(_)) => {}
        Ok(None) => {
            return StateWriteProbe {
                state_dir_present: false,
                probed: false,
                writable: false,
                path: None,
                error: None,
            };
        }
        Err(error) => {
            return StateWriteProbe {
                state_dir_present: true,
                probed: false,
                writable: false,
                path: None,
                error: Some(format!("{error:#}")),
            };
        }
    }
    // Probe inside the managed `tmp` entry: `state audit` allows it, so a
    // probe file leaked by a failed cleanup can never fail that gate, and the
    // verified directory chain rejects link/reparse ancestors.
    let tmp = match managed_state_dir(root, Path::new("tmp"), true) {
        Ok(Some(tmp)) => tmp,
        Ok(None) => {
            return StateWriteProbe {
                state_dir_present: true,
                probed: true,
                writable: false,
                path: None,
                error: Some("managed tmp directory unavailable".into()),
            };
        }
        Err(error) => {
            return StateWriteProbe {
                state_dir_present: true,
                probed: true,
                writable: false,
                path: None,
                error: Some(format!("{error:#}")),
            };
        }
    };
    // The probe file name stays unique per process to avoid concurrent
    // clobbering; the reported path is the stable directory so identical
    // inspections stay byte-identical across runs and languages.
    //
    // `create_new` maps to POSIX `O_CREAT|O_EXCL`, which refuses a pre-planted
    // symlink at the final component even when it dangles. Win32 `CREATE_NEW`
    // gives no such guarantee — it resolves a reparse point first and can fail
    // with ALREADY_EXISTS pointing elsewhere. That is why the probe is only a
    // diagnostic: the authority for state-path safety is `managed_state_*`,
    // which verifies links and reparse points explicitly.
    let probe = tmp.join(format!(".rayman-state-probe-{}.tmp", std::process::id()));
    let write = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
        .and_then(|mut file| {
            use std::io::Write;
            file.write_all(b"rayman-state-probe")
        });
    match write {
        Ok(()) => {
            let cleanup = fs::remove_file(&probe);
            StateWriteProbe {
                state_dir_present: true,
                probed: true,
                writable: true,
                path: Some(display_path(&tmp)),
                error: cleanup.err().map(|error| error.to_string()),
            }
        }
        Err(error) => StateWriteProbe {
            state_dir_present: true,
            probed: true,
            writable: false,
            path: Some(display_path(&tmp)),
            error: Some(error.to_string()),
        },
    }
}

fn canonical_workspace_root(root: &Path) -> Result<PathBuf> {
    let workspace = root
        .canonicalize()
        .with_context(|| format!("无法规范化工作区根: {}", display_path(root)))?;
    ensure_real_directory(&workspace)?;
    Ok(workspace)
}

/// Confirm a real directory is still rooted in this workspace.  Checking only
/// the final component is insufficient after an attacker swaps an already
/// checked ancestor for a link: `symlink_metadata(child)` would then report a
/// perfectly ordinary external child.  Canonicalization turns that redirection
/// into an explicit escape instead.
fn ensure_real_directory_within(workspace: &Path, path: &Path) -> Result<()> {
    ensure_real_directory(path)?;
    let canonical = path
        .canonicalize()
        .with_context(|| format!("无法规范化受管状态目录: {}", display_path(path)))?;
    if !canonical.starts_with(workspace) {
        bail!(
            "受管状态目录逃逸工作区: {} -> {} (工作区: {})",
            display_path(path),
            display_path(&canonical),
            display_path(workspace)
        );
    }
    Ok(())
}

fn normal_components(relative: &Path) -> Result<Vec<&std::ffi::OsStr>> {
    let mut components = Vec::new();
    for component in relative.components() {
        let Component::Normal(part) = component else {
            bail!("不安全的受管状态相对路径: {}", display_path(relative));
        };
        components.push(part);
    }
    Ok(components)
}

fn create_real_directory(path: &Path) -> Result<()> {
    match fs::create_dir(path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => {
            return Err(error)
                .with_context(|| format!("无法创建受管状态目录: {}", display_path(path)));
        }
    }
    ensure_real_directory(path)
}

fn ensure_real_directory_metadata(path: &Path, metadata: &fs::Metadata) -> Result<()> {
    if is_link_or_reparse(metadata) {
        bail!("拒绝链接/reparse 受管状态目录: {}", display_path(path));
    }
    if !metadata.file_type().is_dir() {
        bail!("受管状态路径不是目录: {}", display_path(path));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_and_returns_verified_state_paths() {
        let workspace = tempfile::tempdir().unwrap();
        let path =
            managed_state_file(workspace.path(), Path::new("context/index.json"), true).unwrap();
        assert!(path.ends_with(".RaymanCodingSkill/context/index.json"));
        assert!(path.parent().unwrap().is_dir());
    }

    #[test]
    fn state_write_probe_reports_a_writable_state_root_and_cleans_up() {
        let workspace = tempfile::tempdir().unwrap();
        fs::create_dir(workspace.path().join(STATE_DIR_NAME)).unwrap();

        let probe = state_write_probe(workspace.path());
        assert!(probe.state_dir_present);
        assert!(probe.probed);
        assert!(probe.writable);
        assert!(probe.error.is_none());
        // The probe works inside the state-audit-allowed `tmp` entry and must
        // leave it empty; nothing else may appear at the state root.
        let tmp = workspace.path().join(STATE_DIR_NAME).join("tmp");
        assert!(fs::read_dir(&tmp).unwrap().next().is_none());
        let root_entries: Vec<String> = fs::read_dir(workspace.path().join(STATE_DIR_NAME))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(root_entries, ["tmp"]);
    }

    #[cfg(unix)]
    #[test]
    fn state_write_probe_refuses_a_planted_link_at_the_probe_path() {
        use std::os::unix::fs::symlink;

        let workspace = tempfile::tempdir().unwrap();
        let tmp = workspace.path().join(STATE_DIR_NAME).join("tmp");
        fs::create_dir_all(&tmp).unwrap();
        let outside = tempfile::tempdir().unwrap();
        let victim = outside.path().join("victim.txt");
        fs::write(&victim, "user data").unwrap();
        symlink(
            &victim,
            tmp.join(format!(".rayman-state-probe-{}.tmp", std::process::id())),
        )
        .unwrap();

        let probe = state_write_probe(workspace.path());
        assert!(probe.probed);
        assert!(!probe.writable, "planted link must fail the probe closed");
        assert!(probe.error.is_some());
        assert_eq!(fs::read(&victim).unwrap(), b"user data");
    }

    #[test]
    fn state_write_probe_skips_a_workspace_without_a_state_root() {
        let workspace = tempfile::tempdir().unwrap();

        let probe = state_write_probe(workspace.path());
        assert!(!probe.state_dir_present);
        assert!(!probe.probed);
        assert!(!probe.writable);
        assert!(!workspace.path().join(STATE_DIR_NAME).exists());
    }

    #[cfg(unix)]
    #[test]
    fn state_write_probe_reports_a_denied_state_root_without_failing() {
        use std::os::unix::fs::PermissionsExt;

        let workspace = tempfile::tempdir().unwrap();
        let state = workspace.path().join(STATE_DIR_NAME);
        fs::create_dir(&state).unwrap();
        fs::set_permissions(&state, fs::Permissions::from_mode(0o555)).unwrap();

        let probe = state_write_probe(workspace.path());
        fs::set_permissions(&state, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(probe.state_dir_present);
        assert!(probe.probed);
        assert!(!probe.writable);
        assert!(probe.error.is_some());
    }

    #[test]
    fn rejects_a_non_directory_state_root_without_treating_it_as_missing() {
        let workspace = tempfile::tempdir().unwrap();
        fs::write(workspace.path().join(STATE_DIR_NAME), "not a directory").unwrap();
        assert!(managed_state_file(workspace.path(), Path::new("pending.json"), true).is_err());
        assert!(managed_state_root(workspace.path(), false).is_err());
    }

    #[test]
    fn rejects_a_directory_where_a_managed_state_file_is_expected() {
        let workspace = tempfile::tempdir().unwrap();
        let path = workspace.path().join(STATE_DIR_NAME).join("pending.json");
        fs::create_dir_all(&path).unwrap();

        assert!(managed_state_file(workspace.path(), Path::new("pending.json"), false).is_err());
    }

    #[test]
    fn verified_directory_must_stay_under_the_workspace_root() {
        let workspace = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let canonical_workspace = canonical_workspace_root(workspace.path()).unwrap();

        assert!(ensure_real_directory_within(&canonical_workspace, outside.path()).is_err());
    }

    #[test]
    fn external_managed_directory_rejects_an_empty_path_without_touching_the_root() {
        let external = tempfile::tempdir().unwrap();
        let sentinel = external.path().join("sentinel.txt");
        fs::write(&sentinel, b"preserve").unwrap();

        assert!(managed_external_dir(external.path(), Path::new(""), false).is_err());
        assert!(remove_managed_external_dir_all(external.path(), Path::new("")).is_err());
        assert_eq!(fs::read(&sentinel).unwrap(), b"preserve");
    }

    #[cfg(windows)]
    #[test]
    fn atomic_windows_directory_creation_binds_the_created_handle_and_rejects_collisions() {
        use std::ffi::OsStr;

        let parent = tempfile::tempdir().unwrap();
        let guard = create_windows_directory_object_exclusive(
            parent.path(),
            OsStr::new("lease-leaf"),
            "atomic lease leaf",
        )
        .unwrap();
        let created = guard.path().to_path_buf();
        assert_eq!(created, parent.path().join("lease-leaf"));
        assert!(created.is_dir());

        let held = windows_handle_identity(
            &guard.directory,
            guard.path(),
            true,
            "atomic lease leaf test",
        )
        .unwrap();
        assert!(same_windows_file_object(&guard.identity, &held));
        verify_windows_directory_object_at_path(&guard, guard.path(), "atomic lease leaf").unwrap();

        let collision = create_windows_directory_object_exclusive(
            parent.path(),
            OsStr::new("lease-leaf"),
            "atomic lease leaf",
        )
        .unwrap_err();
        assert!(
            format!("{collision:#}").contains("目录可能已存在"),
            "{collision:#}"
        );

        let displaced = parent.path().join("lease-leaf-displaced");
        fs::rename(&created, &displaced).unwrap();
        fs::create_dir(&created).unwrap();
        let replacement_error =
            verify_windows_directory_object_at_path(&guard, &created, "atomic lease leaf")
                .unwrap_err();
        assert!(
            format!("{replacement_error:#}").contains("强身份不一致"),
            "{replacement_error:#}"
        );
        drop(guard);
        fs::remove_dir_all(&created).unwrap();
        fs::remove_dir_all(&displaced).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn held_parent_creates_and_revalidates_a_direct_child_exclusively() {
        use std::ffi::OsStr;

        let container = tempfile::tempdir().unwrap();
        let parent_path = container.path().join("lease-root");
        fs::create_dir(&parent_path).unwrap();
        let parent = hold_windows_directory_object(&parent_path, "atomic lease root").unwrap();
        let child = parent
            .create_child_exclusive(OsStr::new("t"), "atomic lease temp child")
            .unwrap();
        let child_path = child.path().to_path_buf();
        assert_eq!(child_path, parent.path().join("t"));
        assert!(child_path.is_dir());

        verify_windows_directory_child_identity(
            &parent,
            &child,
            OsStr::new("t"),
            "atomic lease temp child",
        )
        .unwrap();
        parent
            .verify_direct_child(&child, OsStr::new("t"), "atomic lease temp child")
            .unwrap();

        let collision = parent
            .create_child_exclusive(OsStr::new("t"), "atomic lease temp child")
            .unwrap_err();
        assert!(
            format!("{collision:#}").contains("目录可能已存在"),
            "{collision:#}"
        );

        let displaced = parent.path().join("t-displaced");
        fs::rename(&child_path, &displaced).unwrap();
        fs::create_dir(&child_path).unwrap();
        let replacement = verify_windows_directory_child_identity(
            &parent,
            &child,
            OsStr::new("t"),
            "atomic lease temp child",
        )
        .unwrap_err();
        assert!(
            format!("{replacement:#}").contains("强身份不一致"),
            "{replacement:#}"
        );

        drop(child);
        drop(parent);
        fs::remove_dir_all(container.path().join("lease-root")).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn bound_probe_closes_child_handles_before_the_parent_namespace_moves() {
        use std::ffi::OsStr;

        let container = tempfile::tempdir().unwrap();
        let original = container.path().join("host-root");
        fs::create_dir(&original).unwrap();
        let root = hold_windows_directory_object(&original, "movable host root").unwrap();
        root.probe_self("movable host root").unwrap();
        let child = root
            .create_child_exclusive(OsStr::new("t"), "movable host-temp child")
            .unwrap();
        root.probe_direct_child(&child, OsStr::new("t"), "movable host-temp child")
            .unwrap();

        let displaced = container.path().join("host-root-displaced");
        fs::rename(&original, &displaced).unwrap();
        fs::create_dir(&original).unwrap();

        let root_error = root
            .verify_direct_child(&child, OsStr::new("t"), "movable host-temp child")
            .unwrap_err();
        assert!(
            format!("{root_error:#}").contains("强身份不一致"),
            "{root_error:#}"
        );
        assert!(displaced.join("t").is_dir());
        drop(root);
        fs::remove_dir_all(&original).unwrap();
        fs::remove_dir_all(&displaced).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn short_lived_external_namespace_handle_does_not_lock_the_host_root() {
        use std::ffi::OsStr;

        let container = tempfile::tempdir().unwrap();
        let original = container.path().join("host-root");
        fs::create_dir(&original).unwrap();
        let root = hold_windows_directory_object(&original, "host root").unwrap();
        let namespace = root
            .open_or_create_direct_child(OsStr::new("v"), "host lease namespace")
            .unwrap();
        let _lease = namespace
            .create_child_exclusive(OsStr::new("lease"), "host lease")
            .unwrap();
        drop(namespace);

        let displaced = container.path().join("host-root-displaced");
        fs::rename(&original, &displaced).unwrap();
        fs::create_dir(&original).unwrap();
        let error =
            verify_windows_directory_object_at_path(&root, &original, "host root").unwrap_err();
        assert!(format!("{error:#}").contains("强身份不一致"), "{error:#}");
        assert!(displaced.join("v").join("lease").is_dir());

        drop(root);
        fs::remove_dir_all(&original).unwrap();
        fs::remove_dir_all(&displaced).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn held_lease_root_and_children_detect_namespace_moves_while_live() {
        use std::ffi::OsStr;

        let container = tempfile::tempdir().unwrap();
        let root = create_windows_directory_object_exclusive(
            container.path(),
            OsStr::new("lease-root"),
            "movable lease root",
        )
        .unwrap();
        let temp = root
            .create_child_exclusive(OsStr::new("t"), "movable lease temp")
            .unwrap();
        let nested = root
            .create_child_exclusive(OsStr::new("n"), "movable lease nested")
            .unwrap();
        let original = root.path().to_path_buf();
        let displaced = container.path().join("lease-root-displaced");

        fs::rename(&original, &displaced).unwrap();
        fs::create_dir(&original).unwrap();
        let root_error =
            verify_windows_directory_object_at_path(&root, &original, "movable lease root")
                .unwrap_err();
        assert!(
            format!("{root_error:#}").contains("强身份不一致"),
            "{root_error:#}"
        );

        let temp_error = verify_windows_directory_child_identity(
            &root,
            &temp,
            OsStr::new("t"),
            "movable lease temp",
        )
        .unwrap_err();
        assert!(
            format!("{temp_error:#}").contains("强身份不一致"),
            "{temp_error:#}"
        );

        drop(nested);
        drop(temp);
        drop(root);
        fs::remove_dir_all(&original).unwrap();
        fs::remove_dir_all(&displaced).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn held_directory_file_read_returns_the_same_strong_identity() {
        use std::ffi::OsStr;

        let container = tempfile::tempdir().unwrap();
        let path = container.path().join("lease-root");
        fs::create_dir(&path).unwrap();
        let root = hold_windows_directory_object(&path, "manifest root").unwrap();
        fs::write(path.join("lease.json"), b"manifest-bytes").unwrap();

        let (bytes, held_identity) = root
            .read_file_with_identity(OsStr::new("lease.json"), "lease manifest")
            .unwrap();
        assert_eq!(bytes, b"manifest-bytes");
        let named = fs::OpenOptions::new()
            .read(true)
            .open(path.join("lease.json"))
            .unwrap();
        let named_metadata = named.metadata().unwrap();
        let named_identity = file_identity_from_handle(
            &named,
            &named_metadata,
            &path.join("lease.json"),
            "lease manifest",
        )
        .unwrap();
        assert!(same_windows_file_object(&held_identity, &named_identity));
        drop(root);
    }

    #[cfg(windows)]
    #[test]
    fn guarded_tree_removal_prevents_parent_identity_swaps() {
        let workspace = tempfile::tempdir().unwrap();
        let relative = Path::new("tmp/c/lease-one");
        let target = managed_state_dir(workspace.path(), relative, true)
            .unwrap()
            .unwrap();
        fs::create_dir(target.join("nested")).unwrap();
        fs::write(target.join("nested/artifact.bin"), b"artifact").unwrap();
        let parent = target.parent().unwrap().to_path_buf();
        let displaced = target.with_file_name("lease-one-displaced");

        let error = remove_managed_state_dir_all_windows(
            workspace.path(),
            relative,
            |guarded_parent, guarded_target, _leaf| {
                assert_eq!(guarded_parent, parent);
                assert_eq!(guarded_target, target);
                fs::rename(guarded_target, &displaced).unwrap();
                fs::create_dir_all(guarded_target.join("replacement")).unwrap();
                fs::write(
                    guarded_target.join("replacement/sentinel.txt"),
                    b"replacement",
                )
                .unwrap();
                Ok(())
            },
            |_, _| Ok(()),
        )
        .unwrap_err();

        assert!(format!("{error:#}").contains("祖先"), "{error:#}");
        assert_eq!(
            fs::read(displaced.join("nested/artifact.bin")).unwrap(),
            b"artifact"
        );
        assert_eq!(
            fs::read(target.join("replacement/sentinel.txt")).unwrap(),
            b"replacement"
        );
    }

    #[cfg(windows)]
    #[test]
    fn guarded_tree_removal_deletes_a_nested_readonly_tree_by_handle() {
        let workspace = tempfile::tempdir().unwrap();
        let relative = Path::new("tmp/c/lease-nested");
        let target = managed_state_dir(workspace.path(), relative, true)
            .unwrap()
            .unwrap();
        fs::create_dir_all(target.join("one/two")).unwrap();
        let readonly = target.join("one/two/artifact.bin");
        fs::write(&readonly, b"artifact").unwrap();
        let mut permissions = fs::metadata(&readonly).unwrap().permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&readonly, permissions).unwrap();

        assert!(remove_managed_state_dir_all(workspace.path(), relative).unwrap());
        assert!(!target.exists());
    }

    #[cfg(windows)]
    #[test]
    fn guarded_tree_removal_refuses_concurrent_refill_and_leaves_an_orphan() {
        let workspace = tempfile::tempdir().unwrap();
        let relative = Path::new("tmp/c/lease-refill");
        let target = managed_state_dir(workspace.path(), relative, true)
            .unwrap()
            .unwrap();
        fs::write(target.join("original.bin"), b"original").unwrap();
        let mut injected = false;

        let error = remove_managed_state_dir_all_windows(
            workspace.path(),
            relative,
            |_, _, _| Ok(()),
            |snapshot_path, _| {
                if snapshot_path == target && !injected {
                    injected = true;
                    fs::write(snapshot_path.join("refill.bin"), b"refill")?;
                }
                Ok(())
            },
        )
        .unwrap_err();

        assert!(format!("{error:#}").contains("并发 refill"), "{error:#}");
        assert!(target.is_dir());
        assert_eq!(fs::read(target.join("refill.bin")).unwrap(), b"refill");
    }

    #[cfg(windows)]
    #[test]
    fn windows_directory_record_parser_enforces_returned_byte_boundaries() {
        use std::mem::{offset_of, size_of};
        use windows_sys::Wdk::Storage::FileSystem::FILE_ID_BOTH_DIR_INFORMATION;

        const NAME_OFFSET: usize = offset_of!(FILE_ID_BOTH_DIR_INFORMATION, FileName);
        let name = [b'a' as u16, b'.' as u16, b'b' as u16];
        let used = (NAME_OFFSET + name.len() * size_of::<u16>()).next_multiple_of(8);
        let mut storage = vec![0_u64; used / size_of::<u64>()];
        let bytes =
            unsafe { std::slice::from_raw_parts_mut(storage.as_mut_ptr().cast::<u8>(), used) };
        unsafe {
            std::ptr::write_unaligned(
                bytes
                    .as_mut_ptr()
                    .add(offset_of!(FILE_ID_BOTH_DIR_INFORMATION, FileNameLength))
                    .cast::<u32>(),
                (name.len() * size_of::<u16>()) as u32,
            );
            std::ptr::write_unaligned(
                bytes
                    .as_mut_ptr()
                    .add(offset_of!(FILE_ID_BOTH_DIR_INFORMATION, FileId))
                    .cast::<i64>(),
                -7,
            );
        }
        for (index, value) in name.iter().enumerate() {
            let raw = value.to_ne_bytes();
            bytes[NAME_OFFSET + index * 2..NAME_OFFSET + index * 2 + 2].copy_from_slice(&raw);
        }

        let parsed = parse_windows_directory_records(bytes).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].file_id, (-7_i64) as u64);

        unsafe {
            std::ptr::write_unaligned(
                bytes
                    .as_mut_ptr()
                    .add(offset_of!(FILE_ID_BOTH_DIR_INFORMATION, FileNameLength))
                    .cast::<u32>(),
                3,
            );
        }
        assert!(parse_windows_directory_records(bytes).is_err());

        unsafe {
            std::ptr::write_unaligned(
                bytes
                    .as_mut_ptr()
                    .add(offset_of!(FILE_ID_BOTH_DIR_INFORMATION, FileNameLength))
                    .cast::<u32>(),
                (name.len() * size_of::<u16>()) as u32,
            );
            std::ptr::write_unaligned(
                bytes
                    .as_mut_ptr()
                    .add(offset_of!(FILE_ID_BOTH_DIR_INFORMATION, NextEntryOffset))
                    .cast::<u32>(),
                4,
            );
        }
        assert!(parse_windows_directory_records(bytes).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_a_symlinked_state_ancestor_before_creating_children() {
        use std::os::unix::fs::symlink;

        let workspace = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        symlink(outside.path(), workspace.path().join(STATE_DIR_NAME)).unwrap();

        assert!(managed_state_file(workspace.path(), Path::new("tmp/new/file"), true).is_err());
        assert!(!outside.path().join("tmp/new/file").exists());
    }
}
