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
use std::path::{Component, Path, PathBuf};

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

#[cfg(windows)]
pub(crate) fn remove_managed_state_dir_all_windows_verified<F>(
    root: &Path,
    relative: &Path,
    verifier: F,
) -> Result<bool>
where
    F: FnOnce(&fs::File, &Path) -> Result<()>,
{
    remove_managed_state_dir_all_windows(
        root,
        relative,
        |_, target, leaf| verifier(leaf, target),
        |_, _| Ok(()),
    )
}

#[cfg(windows)]
fn remove_managed_state_dir_all_windows<F, G>(
    root: &Path,
    relative: &Path,
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
    let mut names = Vec::with_capacity(components.len() + 1);
    names.push(OsString::from(STATE_DIR_NAME));
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
    empty_windows_directory_handle(
        &chain[leaf_index].file,
        &chain[leaf_index].identity,
        &target,
        &mut after_snapshot,
    )?;
    // A parent may move while children are being emptied. Never dispose the
    // leaf unless the complete workspace-relative chain still names the same
    // strong file objects.
    revalidate_windows_directory_chain(&chain)?;
    let leaf_name = chain[leaf_index]
        .name
        .clone()
        .expect("leaf has a relative name");
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
fn open_windows_directory_guard(path: &Path) -> Result<fs::File> {
    use std::os::windows::fs::OpenOptionsExt as _;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
    };

    let directory = fs::OpenOptions::new()
        .access_mode(WINDOWS_DIRECTORY_GUARD_ACCESS)
        // Ancestors are identity anchors, not namespace locks. Allow delete
        // sharing so a rename can occur and be detected by the mandatory
        // handle-relative revalidation instead of relying on share-mode luck.
        .share_mode(WINDOWS_NAMESPACE_SHARE)
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
const WINDOWS_NAMESPACE_SHARE: u32 = windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ
    | windows_sys::Win32::Storage::FileSystem::FILE_SHARE_WRITE
    | windows_sys::Win32::Storage::FileSystem::FILE_SHARE_DELETE;

#[cfg(windows)]
const WINDOWS_QUERY_BUFFER_BYTES: usize = 64 * 1024;

#[cfg(windows)]
struct WindowsDirectoryHandle {
    file: fs::File,
    identity: FileIdentity,
    logical_path: PathBuf,
    name: Option<std::ffi::OsString>,
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
    use std::mem::size_of;
    use std::os::windows::ffi::OsStrExt as _;
    use std::os::windows::io::{AsRawHandle as _, FromRawHandle as _};
    use windows_sys::Wdk::Foundation::OBJECT_ATTRIBUTES;
    use windows_sys::Wdk::Storage::FileSystem::{
        FILE_DIRECTORY_FILE, FILE_NON_DIRECTORY_FILE, FILE_OPEN, FILE_OPEN_FOR_BACKUP_INTENT,
        FILE_OPEN_REPARSE_POINT, FILE_SYNCHRONOUS_IO_NONALERT, NtCreateFile,
    };
    use windows_sys::Win32::Foundation::{
        HANDLE, OBJ_CASE_INSENSITIVE, OBJ_DONT_REPARSE, STATUS_SUCCESS, UNICODE_STRING,
    };
    use windows_sys::Win32::System::IO::IO_STATUS_BLOCK;

    let mut wide: Vec<u16> = name.encode_wide().collect();
    if wide.is_empty() || wide.iter().any(|value| matches!(*value, 0 | 47 | 58 | 92)) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "unsafe relative Windows component",
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
            FILE_OPEN,
            type_option
                | FILE_OPEN_REPARSE_POINT
                | FILE_OPEN_FOR_BACKUP_INTENT
                | FILE_SYNCHRONOUS_IO_NONALERT,
            std::ptr::null(),
            0,
        )
    };
    if status != STATUS_SUCCESS {
        return Err(windows_nt_error(status));
    }
    if handle.is_null() {
        return Err(io::Error::other("NtCreateFile succeeded without a handle"));
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
        for chunk in bytes[name_start..name_end].chunks_exact(2) {
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
fn empty_windows_directory_handle<G>(
    directory: &fs::File,
    directory_identity: &FileIdentity,
    logical_path: &Path,
    after_snapshot: &mut G,
) -> Result<()>
where
    G: FnMut(&Path, &fs::File) -> Result<()>,
{
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT,
    };

    let entries = stable_windows_directory_snapshot(directory, logical_path)?;
    after_snapshot(logical_path, directory)?;
    for entry in entries {
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
            empty_windows_directory_handle(&child, &child_identity, &child_path, after_snapshot)?;
        }
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
    Ok(bytes)
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

    #[cfg(windows)]
    #[test]
    fn guarded_tree_removal_prevents_parent_identity_swaps() {
        let workspace = tempfile::tempdir().unwrap();
        let relative = Path::new("tmp/cargo-target-leases/lease-one");
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
        let relative = Path::new("tmp/cargo-target-leases/lease-nested");
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
        let relative = Path::new("tmp/cargo-target-leases/lease-refill");
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
