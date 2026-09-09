//! Handle-bound, same-directory publication slots. A unique seed is created
//! before its identity is journaled; claiming a standard Git lock is a later
//! no-replace rename, so a crash cannot create an unidentifiable blocking lock.
use super::*;
use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
use std::{fs::File, io::Write, path::Path};
use windows_sys::Win32::Storage::FileSystem::{
    DELETE, FILE_FLAG_OPEN_REPARSE_POINT, FILE_GENERIC_READ, FILE_GENERIC_WRITE, FILE_RENAME_INFO,
    FILE_SHARE_READ, READ_CONTROL, WRITE_DAC, WRITE_OWNER,
};

pub(super) struct PublicationSlot {
    parent: super::native::SourceDirectory,
    file: File,
    name: String,
    identity: String,
    sha256: String,
}

fn safe_leaf(name: &str) -> Result<()> {
    if name.is_empty()
        || name.contains(['/', '\\', ':'])
        || name.ends_with(['.', ' '])
        || matches!(name, "." | "..")
        || name.chars().any(char::is_control)
    {
        bail!("invalid publication leaf");
    }
    Ok(())
}
pub(super) fn identity(file: &File, path: &Path) -> Result<String> {
    let metadata = file.metadata()?;
    let id = crate::file_io::file_identity_from_handle(file, &metadata, path, "publication slot")?;
    Ok(crate::hash::sha256_bytes(
        format!("{:?}:{:?}", id.volume_serial_number_64, id.file_id_128).as_bytes(),
    ))
}

impl PublicationSlot {
    pub(super) fn create(parent: &Path, seed: &str, bytes: &[u8]) -> Result<Self> {
        Self::create_stream(
            parent,
            seed,
            &mut std::io::Cursor::new(bytes),
            bytes.len() as u64,
        )
    }
    pub(super) fn create_stream(
        parent: &Path,
        seed: &str,
        reader: &mut impl std::io::Read,
        limit: u64,
    ) -> Result<Self> {
        safe_leaf(seed)?;
        let parent = super::native::SourceDirectory::open(parent)?;
        let path = parent.path().join(seed);
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .access_mode(
                FILE_GENERIC_READ
                    | FILE_GENERIC_WRITE
                    | DELETE
                    | WRITE_DAC
                    | WRITE_OWNER
                    | READ_CONTROL,
            )
            .create_new(true)
            .share_mode(FILE_SHARE_READ)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(&path)?;
        use sha2::{Digest, Sha256};
        let mut digest = Sha256::new();
        let mut buffer = [0u8; 65536];
        let mut total = 0u64;
        loop {
            let count = reader.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            total = total
                .checked_add(count as u64)
                .ok_or_else(|| anyhow::anyhow!("publication size overflow"))?;
            if total > limit {
                bail!("publication input exceeds limit");
            }
            file.write_all(&buffer[..count])?;
            digest.update(&buffer[..count]);
        }
        file.sync_all()?;
        let id = identity(&file, &path)?;
        Ok(Self {
            parent,
            file,
            name: seed.into(),
            identity: id,
            sha256: format!("{:x}", digest.finalize()),
        })
    }
    pub(super) fn identity(&self) -> &str {
        &self.identity
    }
    pub(super) fn digest(&self) -> &str {
        &self.sha256
    }

    pub(super) fn preserve_security_from(&self, source: &Path) -> Result<String> {
        use windows_sys::Win32::Security::{
            DACL_SECURITY_INFORMATION, GROUP_SECURITY_INFORMATION, GetKernelObjectSecurity,
            GetSecurityDescriptorControl, OWNER_SECURITY_INFORMATION,
            PROTECTED_DACL_SECURITY_INFORMATION, SE_DACL_PROTECTED, SetKernelObjectSecurity,
            UNPROTECTED_DACL_SECURITY_INFORMATION,
        };
        let handle = std::fs::OpenOptions::new()
            .read(true)
            .access_mode(FILE_GENERIC_READ | READ_CONTROL)
            .share_mode(FILE_SHARE_READ)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(source)?;
        let flags =
            OWNER_SECURITY_INFORMATION | GROUP_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION;
        let read = |file: &File| -> Result<Vec<u8>> {
            let mut size = 0;
            unsafe {
                GetKernelObjectSecurity(
                    file.as_raw_handle() as _,
                    flags,
                    std::ptr::null_mut(),
                    0,
                    &mut size,
                );
            }
            if size == 0 || size > 1024 * 1024 {
                bail!("invalid publication security descriptor size");
            }
            let mut bytes = vec![0u8; size as usize];
            if unsafe {
                GetKernelObjectSecurity(
                    file.as_raw_handle() as _,
                    flags,
                    bytes.as_mut_ptr().cast(),
                    size,
                    &mut size,
                )
            } == 0
            {
                return Err(std::io::Error::last_os_error().into());
            }
            Ok(bytes)
        };
        let mut bytes = read(&handle)?;
        let projection = security_projection(&bytes)?;
        if security_projection(&read(&self.file)?)? == projection {
            return Ok(crate::hash::sha256_bytes(&serde_json::to_vec(&projection)?));
        }
        // Metadata rights are requested only for this explicit preservation
        // operation, not for ordinary publication recovery.
        use windows_sys::Win32::Storage::FileSystem::{FILE_SHARE_DELETE, FILE_SHARE_WRITE};
        let metadata_handle = std::fs::OpenOptions::new()
            .access_mode(READ_CONTROL | WRITE_DAC | WRITE_OWNER)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(self.parent.path().join(&self.name))?;
        if identity(&metadata_handle, &self.parent.path().join(&self.name))? != self.identity {
            bail!("publication metadata target changed");
        }

        let mut control = 0;
        let mut revision = 0;
        if unsafe {
            GetSecurityDescriptorControl(bytes.as_mut_ptr().cast(), &mut control, &mut revision)
        } == 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
        let protection = if control & SE_DACL_PROTECTED != 0 {
            PROTECTED_DACL_SECURITY_INFORMATION
        } else {
            UNPROTECTED_DACL_SECURITY_INFORMATION
        };
        if unsafe {
            SetKernelObjectSecurity(
                metadata_handle.as_raw_handle() as _,
                flags | protection,
                bytes.as_mut_ptr().cast(),
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
        if security_projection(&read(&self.file)?)? != security_projection(&bytes)? {
            bail!("publication owner/group/DACL readback differs");
        }
        Ok(crate::hash::sha256_bytes(&serde_json::to_vec(
            &security_projection(&bytes)?,
        )?))
    }

    /// The caller must durably save identity/digest before claiming a Git lock.
    pub(super) fn rename(&mut self, name: &str, replace: bool) -> Result<()> {
        safe_leaf(name)?;
        let text: Vec<u16> = name.encode_utf16().collect();
        let offset = std::mem::offset_of!(FILE_RENAME_INFO, FileName);
        let size = offset + text.len() * 2;
        let mut buffer = vec![0u64; (size + 2).div_ceil(8)];
        let info = buffer.as_mut_ptr().cast::<FILE_RENAME_INFO>();
        unsafe {
            (*info).Anonymous.Flags = if replace { 1 } else { 0 };
            (*info).RootDirectory = self.parent.raw_handle();
            (*info).FileNameLength = (text.len() * 2) as u32;
            std::ptr::copy_nonoverlapping(
                text.as_ptr(),
                std::ptr::addr_of_mut!((*info).FileName).cast(),
                text.len(),
            );
        }
        let mut io = windows_sys::Win32::System::IO::IO_STATUS_BLOCK::default();
        let status = unsafe {
            windows_sys::Wdk::Storage::FileSystem::NtSetInformationFile(
                self.file.as_raw_handle() as _,
                &mut io,
                info.cast(),
                size as u32,
                windows_sys::Wdk::Storage::FileSystem::FileRenameInformationEx,
            )
        };
        if status < 0 {
            return Err(std::io::Error::from_raw_os_error(unsafe {
                windows_sys::Win32::Foundation::RtlNtStatusToDosError(status)
            } as i32)
            .into());
        }
        self.name = name.into();
        if identity(&self.file, &self.parent.path().join(name))? != self.identity {
            bail!("publication slot identity changed");
        }
        self.file.sync_all()?;
        Ok(())
    }

    pub(super) fn resume(
        parent: &Path,
        name: &str,
        expected_identity: &str,
        expected_digest: &str,
    ) -> Result<Self> {
        Self::resume_with_limit(
            parent,
            name,
            expected_identity,
            expected_digest,
            64 * 1024 * 1024,
        )
    }
    pub(super) fn resume_with_limit(
        parent: &Path,
        name: &str,
        expected_identity: &str,
        expected_digest: &str,
        limit: u64,
    ) -> Result<Self> {
        safe_leaf(name)?;
        let parent = super::native::SourceDirectory::open(parent)?;
        let path = parent.path().join(name);
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .access_mode(FILE_GENERIC_READ | FILE_GENERIC_WRITE | DELETE)
            .share_mode(FILE_SHARE_READ)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(&path)?;
        let metadata = file.metadata()?;
        use windows_sys::Win32::Storage::FileSystem::{
            BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
        };
        let mut link_info = BY_HANDLE_FILE_INFORMATION::default();
        if unsafe { GetFileInformationByHandle(file.as_raw_handle() as _, &mut link_info) } == 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        if link_info.nNumberOfLinks != 1 {
            bail!("publication recovery refuses hardlink aliases");
        }

        if !metadata.is_file()
            || crate::file_io::is_link_or_reparse(&metadata)
            || metadata.len() > limit
        {
            bail!("publication recovery entry is unsafe");
        }
        let id = identity(&file, &path)?;
        if id != expected_identity {
            bail!("publication recovery identity mismatch");
        }
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        std::io::copy(&mut file.try_clone()?, &mut hasher)?;
        let digest = format!("{:x}", hasher.finalize());
        if digest != expected_digest {
            bail!("publication recovery bytes changed");
        }
        Ok(Self {
            parent,
            file,
            name: name.into(),
            identity: id,
            sha256: digest,
        })
    }
}

fn security_projection(bytes: &[u8]) -> Result<serde_json::Value> {
    use windows_sys::Win32::Security::{
        ACE_HEADER, GetAce, GetSecurityDescriptorControl, GetSecurityDescriptorDacl,
        GetSecurityDescriptorGroup, GetSecurityDescriptorOwner, SE_DACL_PROTECTED,
    };
    let descriptor = bytes.as_ptr().cast_mut().cast();
    let mut owner = std::ptr::null_mut();
    let mut group = std::ptr::null_mut();
    let mut defaulted = 0;
    let mut present = 0;
    let mut acl = std::ptr::null_mut();
    let mut control = 0;
    let mut revision = 0;
    if unsafe { GetSecurityDescriptorOwner(descriptor, &mut owner, &mut defaulted) } == 0
        || unsafe { GetSecurityDescriptorGroup(descriptor, &mut group, &mut defaulted) } == 0
        || unsafe { GetSecurityDescriptorDacl(descriptor, &mut present, &mut acl, &mut defaulted) }
            == 0
        || unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) } == 0
        || owner.is_null()
        || group.is_null()
    {
        bail!("invalid file security descriptor");
    }
    let mut entries = Vec::new();
    if !acl.is_null() {
        for i in 0..unsafe { (*acl).AceCount } {
            let mut ace = std::ptr::null_mut();
            if unsafe { GetAce(acl, i as u32, &mut ace) } == 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            let header = unsafe { &*(ace as *const ACE_HEADER) };
            entries.push(
                unsafe { std::slice::from_raw_parts(ace as *const u8, header.AceSize as usize) }
                    .to_vec(),
            );
        }
    }
    Ok(
        serde_json::json!({"owner":super::native::sid_string(owner)?,"group":super::native::sid_string(group)?,"dacl_present":present!=0,"dacl_null":acl.is_null(),"dacl_protected":control&SE_DACL_PROTECTED!=0,"aces":entries}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(windows)]
    #[test]
    fn publication_seed_identity_survives_claim_and_recovery() {
        let temp = tempfile::tempdir().unwrap();
        let mut slot = PublicationSlot::create(temp.path(), "seed", b"candidate").unwrap();
        std::fs::write(temp.path().join("metadata-source"), b"old metadata").unwrap();
        slot.preserve_security_from(&temp.path().join("metadata-source"))
            .unwrap();
        let id = slot.identity().to_owned();
        let digest = slot.digest().to_owned();
        std::fs::write(temp.path().join("index.lock"), b"other writer").unwrap();
        assert!(slot.rename("index.lock", false).is_err());
        assert_eq!(
            std::fs::read(temp.path().join("index.lock")).unwrap(),
            b"other writer"
        );
        std::fs::remove_file(temp.path().join("index.lock")).unwrap();
        slot.rename("index.lock", false).unwrap();
        drop(slot);
        assert!(
            PublicationSlot::resume(temp.path(), "index.lock", &"0".repeat(64), &digest).is_err()
        );
        let mut resumed = PublicationSlot::resume(temp.path(), "index.lock", &id, &digest).unwrap();
        std::fs::write(temp.path().join("index"), b"old").unwrap();
        resumed.rename("index", true).unwrap();
        drop(resumed);
        assert_eq!(
            std::fs::read(temp.path().join("index")).unwrap(),
            b"candidate"
        );
        assert!(!temp.path().join("index.lock").exists());
    }
}
