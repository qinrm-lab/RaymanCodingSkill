//! Windows protected-directory attestation. A filename or caller-provided
//! registration is never sufficient to obtain a worker-owned store.
use anyhow::{Context, Result, bail};
use std::{fs::File, path::Path};

#[cfg(windows)]
pub struct ProtectedDirectory {
    _ancestors: Vec<File>,
    root: std::path::PathBuf,
    identity: String,
    owner_sid: String,
}

#[cfg(windows)]
impl ProtectedDirectory {
    pub fn open(path: &Path, owner_sid: &str) -> Result<Self> {
        Self::open_inner(path, Some(owner_sid))
    }

    fn open_inner(path: &Path, owner_sid: Option<&str>) -> Result<Self> {
        use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_LIST_DIRECTORY,
            FILE_READ_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, READ_CONTROL,
        };
        if !path.is_absolute()
            || path.components().any(|c| {
                matches!(
                    c,
                    std::path::Component::ParentDir | std::path::Component::CurDir
                )
            })
        {
            bail!("protected root must be absolute");
        }
        // Inspect the supplied spelling before canonicalize can erase a
        // junction/symlink. The held opens below close later rename windows.
        let mut original = std::path::PathBuf::new();
        let mut original_root_seen = false;
        for component in path.components() {
            original.push(component);
            original_root_seen |= matches!(component, std::path::Component::RootDir);
            if original_root_seen {
                let metadata = std::fs::symlink_metadata(&original).with_context(|| {
                    format!("inspect protected ancestor {}", original.display())
                })?;
                if crate::file_io::is_link_or_reparse(&metadata) || !metadata.is_dir() {
                    bail!("protected path contains a reparse or non-directory component");
                }
            }
        }
        let canonical = std::fs::canonicalize(path)?;
        let spelling = |p: &Path| {
            crate::pathfmt::display_path(p)
                .replace('/', "\\")
                .trim_end_matches('\\')
                .to_lowercase()
        };
        if spelling(path) != spelling(&canonical) {
            bail!("protected namespace resolved through an alias or changed during inspection");
        }
        let mut cursor = std::path::PathBuf::new();
        let mut ancestors = Vec::new();
        let mut root_seen = false;
        for component in canonical.components() {
            cursor.push(component);
            root_seen |= matches!(component, std::path::Component::RootDir);
            if !root_seen {
                continue;
            }
            let handle = std::fs::OpenOptions::new()
                // Metadata-only opens are exempt from parts of Windows share
                // checking. Directory data access is needed to deny rename.
                .access_mode(FILE_READ_ATTRIBUTES | READ_CONTROL | FILE_LIST_DIRECTORY)
                .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
                .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
                .open(&cursor)
                .with_context(|| format!("open protected ancestor {}", cursor.display()))?;
            let metadata = handle.metadata()?;
            if !metadata.is_dir() || crate::file_io::is_link_or_reparse(&metadata) {
                bail!("protected ancestor is not an ordinary directory");
            }
            ancestors.push(handle);
        }
        let handle = ancestors
            .last()
            .ok_or_else(|| anyhow::anyhow!("protected root handle missing"))?;
        if let Some(owner_sid) = owner_sid {
            attest_owner_and_dacl(handle.as_raw_handle() as _, owner_sid, true)?;
        }
        let metadata = handle.metadata()?;
        let identity = crate::file_io::file_identity_from_handle(
            handle,
            &metadata,
            &canonical,
            "global protected root",
        )?;
        let stable = format!(
            "{:016x}:{:?}",
            identity
                .volume_serial_number_64
                .ok_or_else(|| anyhow::anyhow!("missing volume identity"))?,
            identity
                .file_id_128
                .ok_or_else(|| anyhow::anyhow!("missing file identity"))?
        );
        Ok(Self {
            _ancestors: ancestors,
            root: canonical,
            identity: crate::hash::sha256_bytes(stable.as_bytes()),
            owner_sid: owner_sid.unwrap_or_default().into(),
        })
    }

    pub fn path(&self) -> &Path {
        &self.root
    }
    pub fn identity(&self) -> &str {
        &self.identity
    }

    /// Pin an owned mutable database against namespace replacement without
    /// copying its retained history into memory. SQLite retains its own data locks.
    pub(super) fn pin_data_file(&self, name: &str) -> Result<File> {
        use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
        use windows_sys::Win32::Storage::FileSystem::{
            BY_HANDLE_FILE_INFORMATION, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ,
            FILE_SHARE_WRITE, GetFileInformationByHandle,
        };
        if name.is_empty() || name.contains(['/', '\\', ':']) || name == "." || name == ".." {
            bail!("protected database name is invalid");
        }
        let file = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(self.path().join(name))?;
        let metadata = file.metadata()?;
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        if unsafe { GetFileInformationByHandle(file.as_raw_handle() as _, &mut info) } == 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        if !metadata.is_file()
            || crate::file_io::is_link_or_reparse(&metadata)
            || info.nNumberOfLinks != 1
        {
            bail!("protected database is not an unlinked ordinary file");
        }
        attest_owner_and_dacl(file.as_raw_handle() as _, &self.owner_sid, false)?;
        Ok(file)
    }

    /// Read an ordinary direct child without allowing writes/renames while
    /// reading. Effective leaf ACLs are checked against the protected parent.
    pub fn read_file(&self, name: &str, limit: u64) -> Result<Vec<u8>> {
        use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ,
        };
        if name.is_empty() || name.contains(['/', '\\', ':']) || name == "." || name == ".." {
            bail!("protected child name is invalid");
        }
        let path = self.root.join(name);
        let file = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(&path)?;
        let metadata = file.metadata()?;
        if !metadata.is_file()
            || crate::file_io::is_link_or_reparse(&metadata)
            || metadata.len() > limit
        {
            bail!("protected child is not a bounded ordinary file");
        }
        attest_owner_and_dacl(file.as_raw_handle() as _, &self.owner_sid, false)?;
        let bytes = crate::file_io::read_bytes_from_handle(
            &file,
            metadata.len(),
            &path,
            "protected global child",
        )?;
        if bytes.len() as u64 > limit {
            bail!("protected child grew beyond its limit");
        }
        Ok(bytes)
    }
}

/// Pins source namespace identity without claiming its contents are protected
/// from editing. It cannot be used to open privileged state files.
pub(crate) struct SourceDirectory(ProtectedDirectory);

/// A source database pinned against writers/replacement for a migration window.
pub(super) struct SourceFile {
    pub(super) file: File,
    pub(super) path: std::path::PathBuf,
    _parent: SourceDirectory,
}

pub(super) fn require_unelevated_worker() -> Result<()> {
    use windows_sys::Win32::{
        Foundation::CloseHandle,
        Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation},
        System::Threading::{GetCurrentProcess, OpenProcessToken},
    };
    let mut token = std::ptr::null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let mut elevation = TOKEN_ELEVATION::default();
    let mut length = 0;
    let ok = unsafe {
        GetTokenInformation(
            token,
            TokenElevation,
            (&raw mut elevation).cast(),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut length,
        )
    };
    unsafe {
        CloseHandle(token);
    }
    if ok == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    if elevation.TokenIsElevated != 0 {
        bail!("global worker must run with the desktop user's least-privilege token");
    }
    Ok(())
}
impl SourceDirectory {
    pub(super) fn raw_handle(&self) -> windows_sys::Win32::Foundation::HANDLE {
        use std::os::windows::io::AsRawHandle;
        self.0
            ._ancestors
            .last()
            .expect("pinned root")
            .as_raw_handle() as _
    }
    pub(crate) fn open(path: &Path) -> Result<Self> {
        Ok(Self(ProtectedDirectory::open_inner(path, None)?))
    }
    pub(super) fn identity(&self) -> &str {
        self.0.identity()
    }
    pub(crate) fn path(&self) -> &Path {
        self.0.path()
    }

    pub(super) fn pin_file(&self, relative: &Path) -> Result<SourceFile> {
        use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
        use windows_sys::Win32::Storage::FileSystem::{
            BY_HANDLE_FILE_INFORMATION, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ,
            GetFileInformationByHandle,
        };
        if relative.is_absolute()
            || relative
                .components()
                .any(|c| !matches!(c, std::path::Component::Normal(_)))
        {
            bail!("invalid source pin path");
        }
        let path = self.path().join(relative);
        let parent = Self::open(
            path.parent()
                .ok_or_else(|| anyhow::anyhow!("source pin parent missing"))?,
        )?;
        let file = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(&path)
            .with_context(|| {
                format!("pin source against writers/replacement {}", path.display())
            })?;
        let metadata = file.metadata()?;
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        if unsafe { GetFileInformationByHandle(file.as_raw_handle() as _, &mut info) } == 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        if !metadata.is_file()
            || crate::file_io::is_link_or_reparse(&metadata)
            || info.nNumberOfLinks != 1
        {
            bail!("source pin is not an ordinary unlinked file");
        }
        Ok(SourceFile {
            file,
            path,
            _parent: parent,
        })
    }

    /// Consume only the same bounded packet read by the worker. Keep its
    /// handle exclusive against replacement from comparison through deletion.
    pub(super) fn consume_packet(&self, name: &str, expected: &[u8]) -> Result<bool> {
        use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
        use windows_sys::Win32::Storage::FileSystem::{
            BY_HANDLE_FILE_INFORMATION, DELETE, FILE_DISPOSITION_INFO,
            FILE_FLAG_OPEN_REPARSE_POINT, FILE_GENERIC_READ, FILE_SHARE_READ, FileDispositionInfo,
            GetFileInformationByHandle, SetFileInformationByHandle,
        };
        if name.is_empty() || name.contains(['/', '\\', ':']) || name == "." || name == ".." {
            bail!("queue packet name is invalid");
        }
        let path = self.path().join(name);
        let file = std::fs::OpenOptions::new()
            .access_mode(FILE_GENERIC_READ | DELETE)
            .share_mode(FILE_SHARE_READ)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(&path)?;
        let metadata = file.metadata()?;
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        if unsafe { GetFileInformationByHandle(file.as_raw_handle() as _, &mut info) } == 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        if !metadata.is_file()
            || crate::file_io::is_link_or_reparse(&metadata)
            || info.nNumberOfLinks != 1
            || metadata.len() != expected.len() as u64
        {
            return Ok(false);
        }
        if crate::file_io::read_bytes_from_handle(
            &file,
            metadata.len(),
            &path,
            "queue consumption",
        )? != expected
        {
            return Ok(false);
        }
        let disposition = FILE_DISPOSITION_INFO { DeleteFile: true };
        if unsafe {
            SetFileInformationByHandle(
                file.as_raw_handle() as _,
                FileDispositionInfo,
                (&raw const disposition).cast(),
                std::mem::size_of_val(&disposition) as u32,
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(true)
    }

    pub(crate) fn read_file(&self, relative: &Path, limit: u64) -> Result<Vec<u8>> {
        use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
        use windows_sys::Win32::Storage::FileSystem::{
            BY_HANDLE_FILE_INFORMATION, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ,
            GetFileInformationByHandle,
        };
        if relative.is_absolute()
            || relative
                .components()
                .any(|c| !matches!(c, std::path::Component::Normal(_)))
        {
            bail!("source file must use ordinary relative components");
        }
        let target = self.path().join(relative);
        let parent = Self::open(
            target
                .parent()
                .ok_or_else(|| anyhow::anyhow!("missing source parent"))?,
        )?;
        let file = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(&target)?;
        let metadata = file.metadata()?;
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        if unsafe { GetFileInformationByHandle(file.as_raw_handle() as _, &mut info) } == 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        if !metadata.is_file()
            || crate::file_io::is_link_or_reparse(&metadata)
            || info.nNumberOfLinks != 1
            || metadata.len() > limit
        {
            bail!("source file is linked, unsafe or exceeds size limit");
        }
        let bytes = crate::file_io::read_bytes_from_handle(
            &file,
            metadata.len(),
            &target,
            "held source file",
        )?;
        drop(parent);
        Ok(bytes)
    }
}

#[cfg(test)]
pub(super) fn protect_fixture_directory(
    path: &Path,
    owner_sid: &str,
    extra_ace: &str,
) -> Result<()> {
    let sddl = format!("D:P(A;OICI;FA;;;{owner_sid})(A;OICI;FA;;;SY)(A;OICI;FA;;;BA){extra_ace}");
    set_fixture_dacl(path, &sddl)
}

#[cfg(test)]
pub(super) fn set_fixture_dacl(path: &Path, sddl: &str) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::{
        Foundation::LocalFree,
        Security::{
            Authorization::{
                ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
                SE_FILE_OBJECT, SetNamedSecurityInfoW,
            },
            DACL_SECURITY_INFORMATION, GetSecurityDescriptorDacl,
            PROTECTED_DACL_SECURITY_INFORMATION,
        },
    };
    let text: Vec<u16> = sddl.encode_utf16().chain(Some(0)).collect();
    let mut descriptor = std::ptr::null_mut();
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            text.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            std::ptr::null_mut(),
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error().into());
    }
    let result = (|| {
        let mut present = 0;
        let mut defaulted = 0;
        let mut dacl = std::ptr::null_mut();
        if unsafe { GetSecurityDescriptorDacl(descriptor, &mut present, &mut dacl, &mut defaulted) }
            == 0
            || present == 0
        {
            bail!("fixture DACL conversion failed");
        }
        let name: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let status = unsafe {
            SetNamedSecurityInfoW(
                name.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                dacl,
                std::ptr::null_mut(),
            )
        };
        if status != 0 {
            return Err(std::io::Error::from_raw_os_error(status as i32).into());
        }
        Ok(())
    })();
    unsafe {
        LocalFree(descriptor as _);
    }
    result
}

#[cfg(windows)]
fn attest_owner_and_dacl(
    handle: windows_sys::Win32::Foundation::HANDLE,
    owner_sid: &str,
    require_protected: bool,
) -> Result<()> {
    use windows_sys::Win32::{
        Foundation::LocalFree,
        Security::{
            ACCESS_ALLOWED_ACE, ACE_HEADER, ACL,
            Authorization::{GetSecurityInfo, SE_FILE_OBJECT},
            DACL_SECURITY_INFORMATION, GetAce, GetSecurityDescriptorControl, INHERIT_ONLY_ACE,
            OWNER_SECURITY_INFORMATION, SE_DACL_PROTECTED,
        },
        System::SystemServices::{ACCESS_ALLOWED_ACE_TYPE, ACCESS_DENIED_ACE_TYPE},
    };
    let mut owner = std::ptr::null_mut();
    let mut dacl: *mut ACL = std::ptr::null_mut();
    let mut descriptor = std::ptr::null_mut();
    let status = unsafe {
        GetSecurityInfo(
            handle,
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            std::ptr::null_mut(),
            &mut dacl,
            std::ptr::null_mut(),
            &mut descriptor,
        )
    };
    if status != 0 {
        return Err(std::io::Error::from_raw_os_error(status as i32).into());
    }
    struct Descriptor(windows_sys::Win32::Security::PSECURITY_DESCRIPTOR);
    impl Drop for Descriptor {
        fn drop(&mut self) {
            unsafe {
                LocalFree(self.0 as _);
            }
        }
    }
    let _descriptor = Descriptor(descriptor);
    if owner.is_null() || dacl.is_null() || sid_string(owner)? != owner_sid {
        bail!("protected directory owner or DACL is invalid");
    }
    let mut control = 0u16;
    let mut revision = 0;
    if unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) } == 0
        || (require_protected && control & SE_DACL_PROTECTED == 0)
    {
        bail!("protected directory DACL must disable inheritance");
    }
    // Any right capable of mutation, deletion, owner/DACL changes, or generic
    // all/write access must be restricted to the enrolled owner/System/Admin.
    const WRITE_MASK: u32 = 0x1000_0000 | 0x4000_0000 | 0x000d_0156;
    let mut owner_writes = false;
    for index in 0..unsafe { (*dacl).AceCount } {
        let mut ace = std::ptr::null_mut();
        if unsafe { GetAce(dacl, index as u32, &mut ace) } == 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        let header = unsafe { &*(ace as *const ACE_HEADER) };
        if u32::from(header.AceType) == ACCESS_DENIED_ACE_TYPE {
            continue;
        }
        if u32::from(header.AceType) != ACCESS_ALLOWED_ACE_TYPE
            || (header.AceSize as usize) < std::mem::size_of::<ACCESS_ALLOWED_ACE>()
        {
            bail!("unsupported protected directory ACE");
        }
        let allowed = unsafe { &*(ace as *const ACCESS_ALLOWED_ACE) };
        if allowed.Mask & WRITE_MASK == 0 {
            continue;
        }
        let sid = sid_string((&raw const allowed.SidStart).cast_mut().cast())?;
        if sid != owner_sid && sid != "S-1-5-18" && sid != "S-1-5-32-544" {
            bail!("protected directory grants mutation to an unenrolled principal");
        }
        if sid == owner_sid && u32::from(header.AceFlags) & INHERIT_ONLY_ACE == 0 {
            owner_writes = true;
        }
    }
    if !owner_writes {
        bail!("protected directory owner lacks a direct write grant");
    }
    Ok(())
}

#[cfg(windows)]
pub(super) fn sid_string(sid: windows_sys::Win32::Security::PSID) -> Result<String> {
    use windows_sys::Win32::{
        Foundation::LocalFree, Security::Authorization::ConvertSidToStringSidW,
    };
    let mut text = std::ptr::null_mut();
    if unsafe { ConvertSidToStringSidW(sid, &mut text) } == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let mut length = 0;
    while length < 184 && unsafe { *text.add(length) } != 0 {
        length += 1;
    }
    let result = if length == 184 {
        Err(anyhow::anyhow!("SID exceeds limit"))
    } else {
        Ok(String::from_utf16_lossy(unsafe {
            std::slice::from_raw_parts(text, length)
        }))
    };
    unsafe {
        LocalFree(text as _);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn protected_directory_checks_actual_owner_dacl_and_prevents_rename() {
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().join("owned");
        std::fs::create_dir(&root).unwrap();
        let sid = crate::execution_context::execution_context_probe()
            .principal_sid
            .unwrap();
        protect_fixture_directory(&root, &sid, "").unwrap();
        let bound = ProtectedDirectory::open(&root, &sid).unwrap();
        assert_eq!(bound.identity().len(), 64);
        assert!(std::fs::rename(&root, parent.path().join("renamed")).is_err());
        assert!(ProtectedDirectory::open(&root, "S-1-5-21-1-2-3-1001").is_err());
        drop(bound);
        std::fs::rename(&root, parent.path().join("renamed")).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn protected_directory_rejects_inherited_or_extra_writer_permissions() {
        let temp = tempfile::tempdir().unwrap();
        let sid = crate::execution_context::execution_context_probe()
            .principal_sid
            .unwrap();
        assert!(ProtectedDirectory::open(temp.path(), &sid).is_err());
        protect_fixture_directory(temp.path(), &sid, "(A;OICI;GW;;;WD)").unwrap();
        assert!(ProtectedDirectory::open(temp.path(), &sid).is_err());
        protect_fixture_directory(temp.path(), &sid, "(A;OICI;GRGX;;;WD)").unwrap();
        assert!(ProtectedDirectory::open(temp.path(), &sid).is_ok());
    }

    #[cfg(windows)]
    #[test]
    fn source_file_reader_rejects_hardlink_aliases() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("source"), b"sensitive fixture").unwrap();
        let source = SourceDirectory::open(temp.path()).unwrap();
        assert_eq!(
            source.read_file(Path::new("source"), 1024).unwrap(),
            b"sensitive fixture"
        );
        std::fs::hard_link(temp.path().join("source"), temp.path().join("alias")).unwrap();
        assert!(source.read_file(Path::new("alias"), 1024).is_err());
        assert!(source.read_file(Path::new("source"), 1024).is_err());
    }
    #[cfg(windows)]
    #[test]
    fn queue_consumption_preserves_changed_and_linked_packets() {
        let temp = tempfile::tempdir().unwrap();
        let root = SourceDirectory::open(temp.path()).unwrap();
        let packet = temp.path().join("packet");
        std::fs::write(&packet, b"original").unwrap();
        assert!(!root.consume_packet("packet", b"replaced").unwrap());
        assert_eq!(std::fs::read(&packet).unwrap(), b"original");
        std::fs::hard_link(&packet, temp.path().join("alias")).unwrap();
        assert!(!root.consume_packet("packet", b"original").unwrap());
        std::fs::remove_file(temp.path().join("alias")).unwrap();
        assert!(root.consume_packet("packet", b"original").unwrap());
        assert!(!packet.exists());
        assert!(root.consume_packet("../escape", b"").is_err());
    }
}
