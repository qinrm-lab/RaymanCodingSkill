//! Bounded ordinary-file IO. Privileged callers also retain the protected
//! parent directory lease supplied by the execution core.
#[cfg(windows)]
use anyhow::Context;
use anyhow::{Result, bail};
use std::{
    fs::{self, File},
    io::Write,
    path::Path,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FileIdentity {
    len: u64,
    modified: Option<std::time::SystemTime>,
    #[cfg(windows)]
    pub(crate) volume_serial_number_64: Option<u64>,
    #[cfg(windows)]
    pub(crate) file_id_128: Option<[u8; 16]>,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}

pub(crate) fn is_link_or_reparse(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

pub(crate) fn file_identity_from_handle(
    file: &File,
    metadata: &fs::Metadata,
    _path: &Path,
    label: &str,
) -> Result<FileIdentity> {
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_ID_INFO, FileIdInfo, GetFileInformationByHandleEx,
        };
        let mut info = FILE_ID_INFO::default();
        if unsafe {
            GetFileInformationByHandleEx(
                file.as_raw_handle() as _,
                FileIdInfo,
                (&raw mut info).cast(),
                std::mem::size_of::<FILE_ID_INFO>() as u32,
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error())
                .with_context(|| format!("strong identity unavailable for {label}"));
        }
        Ok(FileIdentity {
            len: metadata.len(),
            modified: metadata.modified().ok(),
            volume_serial_number_64: Some(info.VolumeSerialNumber),
            file_id_128: Some(info.FileId.Identifier),
        })
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let _ = (file, label);
        Ok(FileIdentity {
            len: metadata.len(),
            modified: metadata.modified().ok(),
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }
}

pub(crate) fn read_bytes_from_handle(
    file: &File,
    len: u64,
    path: &Path,
    label: &str,
) -> Result<Vec<u8>> {
    let len = usize::try_from(len)?;
    let mut bytes = vec![0; len];
    let mut offset = 0;
    while offset < len {
        #[cfg(windows)]
        let read = {
            use std::os::windows::fs::FileExt;
            file.seek_read(&mut bytes[offset..], offset as u64)?
        };
        #[cfg(unix)]
        let read = {
            use std::os::unix::fs::FileExt;
            file.read_at(&mut bytes[offset..], offset as u64)?
        };
        if read == 0 {
            bail!("early EOF reading {label}: {}", path.display());
        }
        offset += read;
    }
    Ok(bytes)
}

fn open_read(path: &Path) -> std::io::Result<File> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(1).custom_flags(0x00200000);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    options.open(path)
}

pub(crate) fn read_optional_handle_bound_file_bounded(
    path: &Path,
    label: &str,
    limit: u64,
) -> Result<Option<(Vec<u8>, FileIdentity)>> {
    crate::state_paths::ensure_real_directory(
        path.parent()
            .ok_or_else(|| anyhow::anyhow!("file parent missing"))?,
    )?;
    let file = match open_read(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let metadata = file.metadata()?;
    if !metadata.is_file() || is_link_or_reparse(&metadata) || metadata.len() > limit {
        bail!("{label} is not a bounded ordinary file");
    }
    let before = file_identity_from_handle(&file, &metadata, path, label)?;
    let bytes = read_bytes_from_handle(&file, metadata.len(), path, label)?;
    let after = file_identity_from_handle(&file, &file.metadata()?, path, label)?;
    let named = open_read(path)?;
    let current = file_identity_from_handle(&named, &named.metadata()?, path, label)?;
    if before != after || before != current {
        bail!("{label} changed during read");
    }
    Ok(Some((bytes, before)))
}

pub(crate) fn read_handle_bound_file(path: &Path, label: &str) -> Result<(Vec<u8>, FileIdentity)> {
    read_optional_handle_bound_file_bounded(path, label, 128 * 1024 * 1024)?
        .ok_or_else(|| anyhow::anyhow!("{label} is missing"))
}

pub(crate) fn write_atomic(path: &Path, text: &str) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("file parent missing"))?;
    crate::state_paths::ensure_real_directory(parent)?;
    let mut nonce = [0u8; 16];
    getrandom::fill(&mut nonce).map_err(|e| anyhow::anyhow!("write entropy unavailable: {e}"))?;
    let nonce: String = nonce.iter().map(|b| format!("{b:02x}")).collect();
    let temporary = parent.join(format!(".global-write-{nonce}"));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    file.write_all(text.as_bytes())?;
    file.sync_all()?;
    drop(file);
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::{
            MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
        };
        let from: Vec<u16> = temporary.as_os_str().encode_wide().chain(Some(0)).collect();
        let to: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        if unsafe {
            MoveFileExW(
                from.as_ptr(),
                to.as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
    }
    #[cfg(unix)]
    {
        fs::rename(&temporary, path)?;
        File::open(parent)?.sync_all()?;
    }
    Ok(())
}

pub(crate) fn write_json<T: serde::Serialize>(path: &Path, value: &T) -> Result<()> {
    write_atomic(path, &(serde_json::to_string_pretty(value)? + "\n"))
}
pub(crate) fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    match read_optional_handle_bound_file_bounded(path, "global JSON", 128 * 1024 * 1024)? {
        Some((bytes, _)) => Ok(Some(serde_json::from_slice(&bytes)?)),
        None => Ok(None),
    }
}
