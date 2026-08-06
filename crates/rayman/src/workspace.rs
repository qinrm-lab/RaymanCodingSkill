use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Read, Seek, SeekFrom, Write};
#[cfg(windows)]
use std::path::Prefix;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

use anyhow::{Context, Result, bail};
use serde::Serialize;

use crate::file_io::is_link_or_reparse;
use crate::hash::sha256_bytes;
#[cfg(test)]
use crate::hash::sha256_file;
use crate::pathfmt::display_path;
use crate::state_paths;

const ACTIVATION_RELATIVE: &str = "workspace_skill.yaml";
const SKILL_NAME: &str = "raymancodingskill";
const REBIND_COMMAND: &str = "rayman workspace rebind --yes";
const CANONICAL_SKILL_BYTES: &[u8] = include_bytes!("../assets/canonical-skill.md");
const ACTIVATION_FIELDS: &[&str] = &[
    "skill",
    "enabled",
    "skill_file",
    "skill_sha256",
    "cli_contract",
    "cli_version",
];
const MAX_ACTIVATION_TEMP_ATTEMPTS: usize = 64;
static ACTIVATION_TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);
const ACTIVATION_METADATA_PROBE_BYTES: &[u8] = b"rayman-activation-metadata-probe";
pub const ACTIVATION_METADATA_CAPABILITY_KEY: &str = "activation/metadata-preserving-staging";
pub const ACTIVATION_METADATA_BOUNDARY_CLASS: &str = "filesystem_acl";

#[derive(Debug, Clone, Serialize)]
pub struct WorkspaceActivationReport {
    pub status: String,
    pub active: bool,
    pub state_present: bool,
    pub config_present: bool,
    pub enabled: bool,
    pub skill: Option<String>,
    pub skill_file: Option<String>,
    pub cli_contract: Option<String>,
    pub cli_version: Option<String>,
    pub running_cli_contract: String,
    pub running_cli_version: String,
    pub expected_sha256: Option<String>,
    pub actual_sha256: Option<String>,
    pub issues: Vec<String>,
    pub rebind_eligible: bool,
    pub recovery_command: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub retained_evidence: Vec<RetainedActivationEvidence>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RetainedActivationEvidence {
    pub action: String,
    pub path: String,
    pub sha256: String,
    pub length: u64,
    pub identity: String,
    pub metadata_sha256: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cleanup_error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct WorkspaceRebindReport {
    #[serde(flatten)]
    pub activation: WorkspaceActivationReport,
    pub changed: bool,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ActivationMetadataProbePhase {
    NotApplicable,
    Lock,
    Capture,
    StageCreate,
    StageWrite,
    MetadataApply,
    Verify,
    Cleanup,
    TargetRecheck,
    Complete,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ActivationMetadataFailureClass {
    Contention,
    PermissionDenied,
    OwnerAssignmentDenied,
    PrivilegeNotHeld,
    UnsafeTarget,
    UnsupportedPlatform,
    MetadataMismatch,
    CleanupFailed,
    ConcurrentChange,
    IoError,
}

/// Action-specific, non-destructive proof that the current process can create
/// and clean a staging inode carrying the activation file's authorization
/// metadata. This is deliberately separate from the generic managed-tmp write
/// probe and never authorizes or caches a later activation transaction.
#[derive(Debug, Clone, Serialize)]
pub struct ActivationMetadataCapabilityProbe {
    pub applicable: bool,
    pub probed: bool,
    pub ready: bool,
    pub scope: String,
    pub capability_key: String,
    pub boundary_class: String,
    pub target: Option<String>,
    pub phase: ActivationMetadataProbePhase,
    pub failure_class: Option<ActivationMetadataFailureClass>,
    pub os_error_code: Option<i32>,
    pub activation_unchanged: Option<bool>,
    pub cleanup_complete: Option<bool>,
    pub error: Option<String>,
}

fn scalar(value: &str, line_number: usize) -> Result<String> {
    let value = value.trim();
    if value.is_empty() {
        bail!("workspace_skill.yaml 第 {line_number} 行缺少值");
    }
    let quoted = value.starts_with(['\'', '"']) || value.ends_with(['\'', '"']);
    if quoted {
        let Some(quote) = value.chars().next() else {
            unreachable!("empty activation scalar was rejected")
        };
        if value.len() < 2 || !value.ends_with(quote) {
            bail!("workspace_skill.yaml 第 {line_number} 行包含未闭合或不匹配的引号");
        }
        return Ok(value[1..value.len() - 1].to_string());
    }
    Ok(value.to_string())
}

fn parse_activation(text: &str) -> Result<BTreeMap<String, String>> {
    let mut fields = BTreeMap::new();
    for (index, raw) in text.lines().enumerate() {
        let line_number = index + 1;
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if raw.trim_start() != raw {
            bail!(
                "workspace_skill.yaml 第 {line_number} 行包含不受支持的缩进；激活合同只接受顶层标量"
            );
        }
        let (key, value) = line.split_once(':').ok_or_else(|| {
            anyhow::anyhow!("workspace_skill.yaml 第 {line_number} 行缺少 key/value 分隔符")
        })?;
        let key = key.trim();
        if !ACTIVATION_FIELDS.contains(&key) {
            bail!("workspace_skill.yaml 第 {line_number} 行包含未知字段: {key}");
        }
        if fields.contains_key(key) {
            bail!("workspace_skill.yaml 第 {line_number} 行包含重复字段: {key}");
        }
        fields.insert(key.to_string(), scalar(value, line_number)?);
    }
    Ok(fields)
}
fn resolve_skill_file(root: &Path, value: &str) -> PathBuf {
    let path = Path::new(value);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FileIdentity {
    len: u64,
    modified: Option<SystemTime>,
    created: Option<SystemTime>,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(unix)]
    nlink: u64,
    #[cfg(windows)]
    creation_time: u64,
    #[cfg(windows)]
    last_write_time: u64,
    #[cfg(windows)]
    volume_serial_number: Option<u32>,
    #[cfg(windows)]
    file_index: Option<u64>,
    #[cfg(windows)]
    volume_serial_number_64: Option<u64>,
    #[cfg(windows)]
    file_id_128: Option<[u8; 16]>,
}

fn file_identity(metadata: &fs::Metadata) -> FileIdentity {
    #[cfg(unix)]
    use std::os::unix::fs::MetadataExt as _;
    #[cfg(windows)]
    use std::os::windows::fs::MetadataExt as _;

    FileIdentity {
        len: metadata.len(),
        modified: metadata.modified().ok(),
        created: metadata.created().ok(),
        #[cfg(unix)]
        device: metadata.dev(),
        #[cfg(unix)]
        inode: metadata.ino(),
        #[cfg(unix)]
        nlink: metadata.nlink(),
        #[cfg(windows)]
        creation_time: metadata.creation_time(),
        #[cfg(windows)]
        last_write_time: metadata.last_write_time(),
        #[cfg(windows)]
        volume_serial_number: None,
        #[cfg(windows)]
        file_index: None,
        #[cfg(windows)]
        volume_serial_number_64: None,
        #[cfg(windows)]
        file_id_128: None,
    }
}

#[cfg(windows)]
fn file_identity_from_handle(
    file: &fs::File,
    metadata: &fs::Metadata,
    path: &Path,
) -> Result<FileIdentity> {
    use std::os::windows::io::AsRawHandle as _;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, FILE_ID_INFO, FileIdInfo, GetFileInformationByHandle,
        GetFileInformationByHandleEx,
    };

    let mut information = BY_HANDLE_FILE_INFORMATION::default();
    let read = unsafe { GetFileInformationByHandle(file.as_raw_handle() as _, &mut information) };
    if read == 0 {
        return Err(io::Error::last_os_error()).with_context(|| {
            format!(
                "cannot read strong activation file identity from handle: {}",
                display_path(path)
            )
        });
    }
    let mut identity = file_identity(metadata);
    identity.volume_serial_number = Some(information.dwVolumeSerialNumber);
    identity.file_index =
        Some((u64::from(information.nFileIndexHigh) << 32) | u64::from(information.nFileIndexLow));
    let mut extended = FILE_ID_INFO::default();
    let extended_read = unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle() as _,
            FileIdInfo,
            (&raw mut extended).cast(),
            std::mem::size_of::<FILE_ID_INFO>() as u32,
        )
    };
    if extended_read == 0 {
        return Err(io::Error::last_os_error()).with_context(|| {
            format!(
                "cannot read 128-bit activation file identity from handle: {}",
                display_path(path)
            )
        });
    }
    identity.volume_serial_number_64 = Some(extended.VolumeSerialNumber);
    identity.file_id_128 = Some(extended.FileId.Identifier);
    Ok(identity)
}

#[cfg(not(windows))]
fn file_identity_from_handle(
    _: &fs::File,
    metadata: &fs::Metadata,
    _: &Path,
) -> Result<FileIdentity> {
    Ok(file_identity(metadata))
}
#[derive(Debug, Clone, PartialEq, Eq)]
struct SkillSnapshot {
    path: PathBuf,
    canonical: PathBuf,
    identity: FileIdentity,
    sha256: String,
}

#[derive(Debug, Clone)]
struct PreparedActivation {
    skill: SkillSnapshot,
    recorded_path: String,
    config: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FileSnapshot {
    bytes: Vec<u8>,
    identity: FileIdentity,
    metadata: ActivationMetadata,
}

#[cfg(unix)]
#[derive(Debug, Clone, PartialEq, Eq)]
struct ActivationMetadata {
    mode: u32,
    uid: u32,
    gid: u32,
    #[cfg(target_os = "linux")]
    extended_attributes: Vec<LinuxExtendedAttribute>,
    #[cfg(target_os = "linux")]
    inode_flags: u32,
}

#[cfg(target_os = "linux")]
#[derive(Debug, Clone, PartialEq, Eq)]
struct LinuxExtendedAttribute {
    name: Vec<u8>,
    value: Vec<u8>,
}

#[cfg(windows)]
#[derive(Debug, Clone, PartialEq, Eq)]
enum WindowsDacl {
    Absent,
    Null,
    Acl(Vec<u8>),
}

#[cfg(windows)]
#[derive(Debug, Clone, PartialEq, Eq)]
struct WindowsSecurityMetadata {
    owner: Option<Vec<u8>>,
    group: Option<Vec<u8>>,
    dacl: WindowsDacl,
    control: u16,
}

#[cfg(windows)]
#[derive(Debug, Clone)]
struct ActivationMetadata {
    attributes: u32,
    /// Self-relative OWNER/GROUP/DACL security descriptor returned by
    /// GetKernelObjectSecurity. SACL data is deliberately not claimed: normal
    /// callers do not hold ACCESS_SYSTEM_SECURITY, so attempting to preserve it
    /// would make an unverifiable security promise.
    security_descriptor: Vec<u8>,
    security: WindowsSecurityMetadata,
}

#[cfg(windows)]
impl PartialEq for ActivationMetadata {
    fn eq(&self, other: &Self) -> bool {
        self.attributes == other.attributes && self.security == other.security
    }
}

#[cfg(windows)]
impl Eq for ActivationMetadata {}

#[cfg(not(any(unix, windows)))]
#[derive(Debug, Clone, PartialEq, Eq)]
struct ActivationMetadata;

struct OwnedActivationTemp {
    file: fs::File,
    path: Option<PathBuf>,
    published: bool,
    target: PathBuf,
    snapshot: FileSnapshot,
}

struct ActivationDirectory {
    path: PathBuf,
    file: fs::File,
}

struct ActivationTransaction {
    target: PathBuf,
    target_name: std::ffi::OsString,
    parent: ActivationDirectory,
    retained: ActivationDirectory,
}

struct CapturedActivation {
    file: fs::File,
    path: PathBuf,
    snapshot: FileSnapshot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActivationTransactionPhase {
    RebindAfterOriginalCapture,
    RebindAfterRollbackCapture,
    RebindAfterPublicationMove,
    RebindBeforeSuccessCleanup,
    InstallBeforeCreatePublication,
    InstallAfterRollbackCapture,
    InstallAfterPublicationMove,
}

/// The CAS threat boundary covers non-cooperating writers of the canonical
/// activation target. Private transaction names are capability paths: generate
/// them from OS randomness, never expose them before the cleanup decision, and
/// retain rather than delete whenever their bound identity or bytes differ.
fn activation_temp_path(target: &Path) -> Result<PathBuf> {
    let mut nonce = [0_u8; 16];
    getrandom::fill(&mut nonce).context("cannot obtain randomness for activation transaction")?;
    let random = u128::from_ne_bytes(nonce);
    let counter = ACTIVATION_TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let token = random ^ u128::from(counter);
    let name = target
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .unwrap_or("workspace_skill.yaml");
    Ok(target.with_file_name(format!(".{name}.rayman-{}-{token}.tmp", std::process::id())))
}
fn has_strong_file_identity(identity: &FileIdentity) -> bool {
    #[cfg(windows)]
    {
        identity.volume_serial_number.is_some()
            && identity.file_index.is_some()
            && identity.volume_serial_number_64.is_some()
            && identity.file_id_128.is_some()
    }
    #[cfg(not(windows))]
    {
        let _ = identity;
        true
    }
}

fn snapshots_match(expected: &FileSnapshot, actual: &FileSnapshot) -> bool {
    has_strong_file_identity(&expected.identity)
        && has_strong_file_identity(&actual.identity)
        && expected == actual
}

fn retained_evidence(
    action: impl Into<String>,
    path: &Path,
    snapshot: &FileSnapshot,
    cleanup_error: Option<String>,
) -> RetainedActivationEvidence {
    #[cfg(unix)]
    let identity = format!(
        "dev={};ino={};nlink={}",
        snapshot.identity.device, snapshot.identity.inode, snapshot.identity.nlink
    );
    #[cfg(windows)]
    let identity = format!(
        "volume={:?};file_index={:?};volume64={:?};file_id128={:?}",
        snapshot.identity.volume_serial_number,
        snapshot.identity.file_index,
        snapshot.identity.volume_serial_number_64,
        snapshot.identity.file_id_128
    );
    #[cfg(not(any(unix, windows)))]
    let identity = format!("len={}", snapshot.identity.len);
    RetainedActivationEvidence {
        action: action.into(),
        path: display_path(path),
        sha256: sha256_bytes(&snapshot.bytes),
        length: snapshot.identity.len,
        identity,
        metadata_sha256: sha256_bytes(format!("{:?}", snapshot.metadata).as_bytes()),
        cleanup_error,
    }
}
fn ensure_supported_activation_write_platform() -> Result<()> {
    #[cfg(not(any(windows, target_os = "linux")))]
    bail!("activation writes require the Linux or Windows handle-bound transaction implementation");
    #[cfg(any(windows, target_os = "linux"))]
    Ok(())
}

fn sync_activation_parent(transaction: &ActivationTransaction) -> Result<()> {
    #[cfg(not(target_os = "linux"))]
    let _ = transaction;
    #[cfg(target_os = "linux")]
    {
        transaction.parent.file.sync_all().with_context(|| {
            format!(
                "cannot durably sync activation transaction directory {}",
                display_path(&transaction.parent.path)
            )
        })?;
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn sync_activation_retained(transaction: &ActivationTransaction) -> Result<()> {
    transaction.retained.file.sync_all().with_context(|| {
        format!(
            "cannot durably sync retained activation transaction directory {}",
            display_path(&transaction.retained.path)
        )
    })
}

#[cfg(target_os = "linux")]
const FS_SECRM_FL: u32 = 0x0000_0001;
#[cfg(target_os = "linux")]
const FS_UNRM_FL: u32 = 0x0000_0002;
#[cfg(target_os = "linux")]
const FS_COMPR_FL: u32 = 0x0000_0004;
#[cfg(target_os = "linux")]
const FS_SYNC_FL: u32 = 0x0000_0008;
#[cfg(target_os = "linux")]
const FS_IMMUTABLE_FL: u32 = 0x0000_0010;
#[cfg(target_os = "linux")]
const FS_APPEND_FL: u32 = 0x0000_0020;
#[cfg(target_os = "linux")]
const FS_NODUMP_FL: u32 = 0x0000_0040;
#[cfg(target_os = "linux")]
const FS_NOATIME_FL: u32 = 0x0000_0080;
#[cfg(target_os = "linux")]
const FS_DIRTY_FL: u32 = 0x0000_0100;
#[cfg(target_os = "linux")]
const FS_COMPRBLK_FL: u32 = 0x0000_0200;
#[cfg(target_os = "linux")]
const FS_NOCOMP_FL: u32 = 0x0000_0400;
#[cfg(target_os = "linux")]
const FS_ENCRYPT_FL: u32 = 0x0000_0800;
#[cfg(target_os = "linux")]
const FS_INDEX_FL: u32 = 0x0000_1000;
#[cfg(target_os = "linux")]
const FS_IMAGIC_FL: u32 = 0x0000_2000;
#[cfg(target_os = "linux")]
const FS_JOURNAL_DATA_FL: u32 = 0x0000_4000;
#[cfg(target_os = "linux")]
const FS_NOTAIL_FL: u32 = 0x0000_8000;
#[cfg(target_os = "linux")]
const FS_DIRSYNC_FL: u32 = 0x0001_0000;
#[cfg(target_os = "linux")]
const FS_TOPDIR_FL: u32 = 0x0002_0000;
#[cfg(target_os = "linux")]
const FS_HUGE_FILE_FL: u32 = 0x0004_0000;
#[cfg(target_os = "linux")]
const FS_EXTENT_FL: u32 = 0x0008_0000;
#[cfg(target_os = "linux")]
const FS_VERITY_FL: u32 = 0x0010_0000;
#[cfg(target_os = "linux")]
const FS_EA_INODE_FL: u32 = 0x0020_0000;
#[cfg(target_os = "linux")]
const FS_EOFBLOCKS_FL: u32 = 0x0040_0000;
#[cfg(target_os = "linux")]
const FS_NOCOW_FL: u32 = 0x0080_0000;
#[cfg(target_os = "linux")]
const FS_DAX_FL: u32 = 0x0200_0000;
#[cfg(target_os = "linux")]
const FS_INLINE_DATA_FL: u32 = 0x1000_0000;
#[cfg(target_os = "linux")]
const FS_PROJINHERIT_FL: u32 = 0x2000_0000;
#[cfg(target_os = "linux")]
const FS_CASEFOLD_FL: u32 = 0x4000_0000;
#[cfg(target_os = "linux")]
const FS_RESERVED_FL: u32 = 0x8000_0000;

#[cfg(target_os = "linux")]
// NODUMP is the only user-controlled flag this transaction deliberately
// replays. Other flags are either security-sensitive, capability-dependent,
// directory-only, or filesystem-policy state and therefore fail closed when
// present on the source activation inode.
const LINUX_SAFE_COPY_INODE_FLAGS: u32 = FS_NODUMP_FL;
#[cfg(target_os = "linux")]
const LINUX_UNREPRESENTABLE_INODE_FLAGS: u32 = FS_SECRM_FL
    | FS_UNRM_FL
    | FS_COMPR_FL
    | FS_SYNC_FL
    | FS_IMMUTABLE_FL
    | FS_APPEND_FL
    | FS_NOATIME_FL
    | FS_NOCOMP_FL
    | FS_ENCRYPT_FL
    | FS_JOURNAL_DATA_FL
    | FS_NOTAIL_FL
    | FS_DIRSYNC_FL
    | FS_TOPDIR_FL
    | FS_VERITY_FL
    | FS_NOCOW_FL
    | FS_DAX_FL
    | FS_PROJINHERIT_FL
    | FS_CASEFOLD_FL;
#[cfg(target_os = "linux")]
const LINUX_KERNEL_MANAGED_INODE_FLAGS: u32 = FS_DIRTY_FL
    | FS_COMPRBLK_FL
    | FS_INDEX_FL
    | FS_IMAGIC_FL
    | FS_HUGE_FILE_FL
    | FS_EXTENT_FL
    | FS_EA_INODE_FL
    | FS_EOFBLOCKS_FL
    | FS_INLINE_DATA_FL
    | FS_RESERVED_FL;
#[cfg(target_os = "linux")]
const LINUX_KNOWN_INODE_FLAGS: u32 = LINUX_SAFE_COPY_INODE_FLAGS
    | LINUX_UNREPRESENTABLE_INODE_FLAGS
    | LINUX_KERNEL_MANAGED_INODE_FLAGS;

#[cfg(target_os = "linux")]
fn validate_linux_inode_flags(flags: u32, path: &Path) -> Result<()> {
    let unrepresentable = flags & LINUX_UNREPRESENTABLE_INODE_FLAGS;
    if unrepresentable != 0 {
        bail!(
            "activation inode has flags that cannot be safely reproduced on a new inode (0x{unrepresentable:08x}); refusing replacement: {}",
            display_path(path)
        );
    }
    let unknown = flags & !LINUX_KNOWN_INODE_FLAGS;
    if unknown != 0 {
        bail!(
            "activation inode has unknown flags (0x{unknown:08x}); refusing replacement: {}",
            display_path(path)
        );
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn linux_inode_flags(file: &fs::File, path: &Path) -> Result<u32> {
    use std::os::fd::AsRawFd as _;

    let mut flags: libc::c_int = 0;
    let read = unsafe { libc::ioctl(file.as_raw_fd(), libc::FS_IOC_GETFLAGS, &mut flags) };
    if read != 0 {
        let error = io::Error::last_os_error();
        if error.raw_os_error().is_some_and(|code| {
            code == libc::ENOTTY || code == libc::EOPNOTSUPP || code == libc::ENOSYS
        }) {
            bail!(
                "FS_IOC_GETFLAGS is unavailable; refusing activation replacement: {}: {error}",
                display_path(path)
            );
        }
        return Err(error).with_context(|| {
            format!(
                "cannot capture activation inode flags from its open handle: {}",
                display_path(path)
            )
        });
    }
    let flags = flags as u32;
    validate_linux_inode_flags(flags, path)?;
    Ok(flags)
}

#[cfg(target_os = "linux")]
fn linux_xattr_unsupported(error: &io::Error) -> bool {
    error.raw_os_error().is_some_and(|code| {
        code == libc::ENOTSUP || code == libc::EOPNOTSUPP || code == libc::ENOSYS
    })
}

#[cfg(target_os = "linux")]
fn linux_xattr_name(name: &[u8], path: &Path) -> Result<std::ffi::CString> {
    std::ffi::CString::new(name).with_context(|| {
        format!(
            "activation extended-attribute name contains NUL: {}",
            display_path(path)
        )
    })
}

#[cfg(target_os = "linux")]
fn linux_xattr_value(fd: libc::c_int, name: &[u8], path: &Path) -> Result<Vec<u8>> {
    const MAX_ATTEMPTS: usize = 8;
    let name = linux_xattr_name(name, path)?;
    for _ in 0..MAX_ATTEMPTS {
        let needed = unsafe { libc::fgetxattr(fd, name.as_ptr(), std::ptr::null_mut(), 0) };
        if needed < 0 {
            return Err(io::Error::last_os_error()).with_context(|| {
                format!(
                    "cannot size activation extended attribute {:?}: {}",
                    name,
                    display_path(path)
                )
            });
        }
        let capacity = usize::try_from(needed)
            .context("activation extended-attribute length does not fit in memory")?
            .max(1);
        let mut value = vec![0_u8; capacity];
        let read =
            unsafe { libc::fgetxattr(fd, name.as_ptr(), value.as_mut_ptr().cast(), value.len()) };
        if read < 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ERANGE) {
                continue;
            }
            return Err(error).with_context(|| {
                format!(
                    "cannot capture activation extended attribute {:?}: {}",
                    name,
                    display_path(path)
                )
            });
        }
        let read = usize::try_from(read)
            .context("activation extended-attribute read length is invalid")?;
        if read > value.len() {
            bail!(
                "activation extended-attribute read exceeded its bound: {}",
                display_path(path)
            );
        }
        value.truncate(read);
        return Ok(value);
    }
    bail!(
        "activation extended attribute changed size repeatedly during capture: {}",
        display_path(path)
    )
}

#[cfg(target_os = "linux")]
fn linux_extended_attributes(file: &fs::File, path: &Path) -> Result<Vec<LinuxExtendedAttribute>> {
    use std::os::fd::AsRawFd as _;

    const MAX_ATTEMPTS: usize = 8;
    let fd = file.as_raw_fd();
    for _ in 0..MAX_ATTEMPTS {
        let needed = unsafe { libc::flistxattr(fd, std::ptr::null_mut(), 0) };
        if needed < 0 {
            let error = io::Error::last_os_error();
            if linux_xattr_unsupported(&error) {
                bail!(
                    "handle-bound extended-attribute capture is unavailable on this filesystem; refusing activation replacement: {}: {error}",
                    display_path(path)
                );
            }
            return Err(error).with_context(|| {
                format!(
                    "cannot enumerate activation extended attributes: {}",
                    display_path(path)
                )
            });
        }
        if needed == 0 {
            return Ok(Vec::new());
        }
        let mut names = vec![
            0_u8;
            usize::try_from(needed)
                .context("activation extended-attribute name list is too large")?
        ];
        let read = unsafe { libc::flistxattr(fd, names.as_mut_ptr().cast(), names.len()) };
        if read < 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ERANGE) {
                continue;
            }
            if linux_xattr_unsupported(&error) {
                bail!(
                    "handle-bound extended-attribute capture became unavailable; refusing activation replacement: {}: {error}",
                    display_path(path)
                );
            }
            return Err(error).with_context(|| {
                format!(
                    "cannot capture activation extended-attribute names: {}",
                    display_path(path)
                )
            });
        }
        let read = usize::try_from(read)
            .context("activation extended-attribute name-list length is invalid")?;
        if read > names.len() {
            bail!(
                "activation extended-attribute name list exceeded its bound: {}",
                display_path(path)
            );
        }
        names.truncate(read);
        if names.last() != Some(&0) {
            bail!(
                "activation extended-attribute name list is malformed: {}",
                display_path(path)
            );
        }
        let mut attributes = Vec::new();
        let mut remaining = names.as_slice();
        while !remaining.is_empty() {
            let end = remaining
                .iter()
                .position(|byte| *byte == 0)
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "activation extended-attribute name list is unterminated: {}",
                        display_path(path)
                    )
                })?;
            let name = &remaining[..end];
            if name.is_empty() {
                bail!(
                    "activation extended-attribute name list contains an empty name: {}",
                    display_path(path)
                );
            }
            attributes.push(LinuxExtendedAttribute {
                name: name.to_vec(),
                value: linux_xattr_value(fd, name, path)?,
            });
            remaining = &remaining[end + 1..];
        }
        attributes.sort_by(|left, right| left.name.cmp(&right.name));
        if attributes
            .windows(2)
            .any(|pair| pair[0].name == pair[1].name)
        {
            bail!(
                "activation extended-attribute name list contains duplicates: {}",
                display_path(path)
            );
        }
        return Ok(attributes);
    }
    bail!(
        "activation extended-attribute names changed repeatedly during capture: {}",
        display_path(path)
    )
}

#[cfg(target_os = "linux")]
fn activation_metadata_from_handle(
    file: &fs::File,
    metadata: &fs::Metadata,
    path: &Path,
) -> Result<ActivationMetadata> {
    use std::os::unix::fs::MetadataExt as _;

    Ok(ActivationMetadata {
        mode: metadata.mode() & 0o7777,
        uid: metadata.uid(),
        gid: metadata.gid(),
        // On Linux, POSIX ACLs are represented by system.posix_acl_* xattrs;
        // security.* includes SELinux labels and file capabilities. Capture
        // the complete fd-visible xattr set so an authorization-relevant name
        // cannot be silently dropped by a publication or rollback.
        extended_attributes: linux_extended_attributes(file, path)?,
        inode_flags: linux_inode_flags(file, path)?,
    })
}

#[cfg(all(unix, not(target_os = "linux")))]
fn activation_metadata_from_handle(
    _: &fs::File,
    _: &fs::Metadata,
    path: &Path,
) -> Result<ActivationMetadata> {
    bail!(
        "safe handle-bound preservation of POSIX/NFSv4 ACLs, security extended attributes, and BSD file flags is not implemented for this Unix platform; refusing activation replacement: {}",
        display_path(path)
    )
}

#[cfg(windows)]
fn parse_windows_security_descriptor(bytes: &[u8], path: &Path) -> Result<WindowsSecurityMetadata> {
    const SECURITY_DESCRIPTOR_RELATIVE_HEADER: usize = 20;
    const SE_OWNER_DEFAULTED: u16 = 0x0001;
    const SE_GROUP_DEFAULTED: u16 = 0x0002;
    const SE_DACL_PRESENT: u16 = 0x0004;
    const SE_DACL_DEFAULTED: u16 = 0x0008;
    const SE_DACL_PROTECTED: u16 = 0x1000;
    // AUTO_INHERITED is provenance bookkeeping, not an access-control
    // semantic: Windows clears it when an identical inherited ACL is supplied
    // explicitly to CreateFileW. Preserve and compare the ACE bytes (including
    // every INHERITED_ACE flag) and the PROTECTED boundary instead.
    const RELEVANT_CONTROL: u16 = SE_OWNER_DEFAULTED
        | SE_GROUP_DEFAULTED
        | SE_DACL_PRESENT
        | SE_DACL_DEFAULTED
        | SE_DACL_PROTECTED;

    if bytes.len() < SECURITY_DESCRIPTOR_RELATIVE_HEADER {
        bail!(
            "activation security descriptor is truncated: {}",
            display_path(path)
        );
    }
    let read_u16 = |offset: usize| -> Result<u16> {
        let raw = bytes.get(offset..offset + 2).ok_or_else(|| {
            anyhow::anyhow!(
                "activation security descriptor field is truncated: {}",
                display_path(path)
            )
        })?;
        Ok(u16::from_le_bytes([raw[0], raw[1]]))
    };
    let read_u32 = |offset: usize| -> Result<u32> {
        let raw = bytes.get(offset..offset + 4).ok_or_else(|| {
            anyhow::anyhow!(
                "activation security descriptor field is truncated: {}",
                display_path(path)
            )
        })?;
        Ok(u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]))
    };
    let control = read_u16(2)?;
    let owner_offset = read_u32(4)? as usize;
    let group_offset = read_u32(8)? as usize;
    let dacl_offset = read_u32(16)? as usize;
    let read_sid = |offset: usize| -> Result<Option<Vec<u8>>> {
        if offset == 0 {
            return Ok(None);
        }
        let sub_authorities = *bytes.get(offset + 1).ok_or_else(|| {
            anyhow::anyhow!(
                "activation security descriptor SID is truncated: {}",
                display_path(path)
            )
        })? as usize;
        let length = 8_usize
            .checked_add(sub_authorities.checked_mul(4).ok_or_else(|| {
                anyhow::anyhow!("activation security descriptor SID length overflow")
            })?)
            .ok_or_else(|| anyhow::anyhow!("activation security descriptor SID length overflow"))?;
        let sid = bytes.get(offset..offset + length).ok_or_else(|| {
            anyhow::anyhow!(
                "activation security descriptor SID bytes are truncated: {}",
                display_path(path)
            )
        })?;
        Ok(Some(sid.to_vec()))
    };
    let dacl = if control & SE_DACL_PRESENT == 0 {
        WindowsDacl::Absent
    } else if dacl_offset == 0 {
        WindowsDacl::Null
    } else {
        let acl_size = read_u16(dacl_offset + 2)? as usize;
        if acl_size < 8 {
            bail!(
                "activation DACL has an invalid length: {}",
                display_path(path)
            );
        }
        let acl = bytes
            .get(dacl_offset..dacl_offset + acl_size)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "activation DACL bytes are truncated: {}",
                    display_path(path)
                )
            })?;
        WindowsDacl::Acl(acl.to_vec())
    };
    Ok(WindowsSecurityMetadata {
        owner: read_sid(owner_offset)?,
        group: read_sid(group_offset)?,
        dacl,
        control: control & RELEVANT_CONTROL,
    })
}

#[cfg(windows)]
fn activation_metadata_from_handle(
    file: &fs::File,
    metadata: &fs::Metadata,
    path: &Path,
) -> Result<ActivationMetadata> {
    use std::ffi::c_void;
    use std::os::windows::fs::MetadataExt as _;
    use std::os::windows::io::AsRawHandle as _;

    const OWNER_SECURITY_INFORMATION: u32 = 0x0000_0001;
    const GROUP_SECURITY_INFORMATION: u32 = 0x0000_0002;
    const DACL_SECURITY_INFORMATION: u32 = 0x0000_0004;
    const REQUESTED_SECURITY_INFORMATION: u32 =
        OWNER_SECURITY_INFORMATION | GROUP_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION;

    #[link(name = "advapi32")]
    unsafe extern "system" {
        fn GetKernelObjectSecurity(
            handle: *mut c_void,
            requested_information: u32,
            security_descriptor: *mut c_void,
            length: u32,
            length_needed: *mut u32,
        ) -> i32;
    }

    let handle = file.as_raw_handle();
    let mut needed = 0_u32;
    let first = unsafe {
        GetKernelObjectSecurity(
            handle,
            REQUESTED_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            0,
            &mut needed,
        )
    };
    if first == 0 && needed == 0 {
        return Err(io::Error::last_os_error()).with_context(|| {
            format!(
                "cannot size activation owner/group/DACL metadata: {}",
                display_path(path)
            )
        });
    }
    if needed == 0 {
        bail!(
            "activation owner/group/DACL query returned an empty descriptor: {}",
            display_path(path)
        );
    }
    let mut security_descriptor = vec![0_u8; needed as usize];
    let read = unsafe {
        GetKernelObjectSecurity(
            handle,
            REQUESTED_SECURITY_INFORMATION,
            security_descriptor.as_mut_ptr().cast(),
            needed,
            &mut needed,
        )
    };
    if read == 0 {
        return Err(io::Error::last_os_error()).with_context(|| {
            format!(
                "cannot capture activation owner/group/DACL metadata: {}",
                display_path(path)
            )
        });
    }
    security_descriptor.truncate(needed as usize);
    let security = parse_windows_security_descriptor(&security_descriptor, path)?;
    Ok(ActivationMetadata {
        attributes: metadata.file_attributes(),
        security_descriptor,
        security,
    })
}

#[cfg(not(any(unix, windows)))]
fn activation_metadata_from_handle(
    _: &fs::File,
    _: &fs::Metadata,
    path: &Path,
) -> Result<ActivationMetadata> {
    bail!(
        "activation security metadata capture is unsupported on this platform: {}",
        display_path(path)
    )
}

#[cfg(test)]
fn read_bound_activation_metadata(path: &Path) -> Result<(ActivationMetadata, FileIdentity)> {
    let file = open_file_no_follow(path).with_context(|| {
        format!(
            "cannot open activation metadata without following links: {}",
            display_path(path)
        )
    })?;
    let before = file.metadata().with_context(|| {
        format!(
            "cannot read activation metadata handle: {}",
            display_path(path)
        )
    })?;
    if is_link_or_reparse(&before) || !before.file_type().is_file() {
        bail!(
            "activation metadata target must be an ordinary non-link file: {}",
            display_path(path)
        );
    }
    let before_identity = file_identity_from_handle(&file, &before, path)?;
    let before_security = activation_metadata_from_handle(&file, &before, path)?;
    let after = file.metadata().with_context(|| {
        format!(
            "cannot recheck activation metadata handle: {}",
            display_path(path)
        )
    })?;
    let after_identity = file_identity_from_handle(&file, &after, path)?;
    let after_security = activation_metadata_from_handle(&file, &after, path)?;
    if before_identity != after_identity || before_security != after_security {
        bail!(
            "activation security metadata changed during capture: {}",
            display_path(path)
        );
    }

    let named = open_file_no_follow(path).with_context(|| {
        format!(
            "activation path changed after metadata capture: {}",
            display_path(path)
        )
    })?;
    let named_metadata = named.metadata().with_context(|| {
        format!(
            "cannot recheck named activation metadata: {}",
            display_path(path)
        )
    })?;
    if is_link_or_reparse(&named_metadata) || !named_metadata.file_type().is_file() {
        bail!(
            "activation metadata path became a link or non-file: {}",
            display_path(path)
        );
    }
    let named_identity = file_identity_from_handle(&named, &named_metadata, path)?;
    let named_security = activation_metadata_from_handle(&named, &named_metadata, path)?;
    if named_identity != after_identity || named_security != after_security {
        bail!(
            "activation path or security metadata changed after capture: {}",
            display_path(path)
        );
    }
    Ok((after_security, after_identity))
}

#[cfg(windows)]
fn create_activation_temp_file(
    path: &Path,
    metadata: Option<&ActivationMetadata>,
    delete_on_close: bool,
) -> io::Result<fs::File> {
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt as _;
    use std::os::windows::fs::OpenOptionsExt as _;
    use std::os::windows::io::FromRawHandle as _;

    let Some(metadata) = metadata else {
        return fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .access_mode(0xc001_0000)
            .share_mode(1)
            .custom_flags(if delete_on_close { 0x0400_0000 } else { 0 })
            .open(path);
    };

    #[repr(C)]
    struct SecurityAttributes {
        length: u32,
        security_descriptor: *mut c_void,
        inherit_handle: i32,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateFileW(
            file_name: *const u16,
            desired_access: u32,
            share_mode: u32,
            security_attributes: *const SecurityAttributes,
            creation_disposition: u32,
            flags_and_attributes: u32,
            template_file: *mut c_void,
        ) -> *mut c_void;
    }

    const GENERIC_READ: u32 = 0x8000_0000;
    const GENERIC_WRITE: u32 = 0x4000_0000;
    const DELETE_ACCESS: u32 = 0x0001_0000;
    const CREATE_NEW: u32 = 1;
    const FILE_ATTRIBUTE_NORMAL: u32 = 0x0000_0080;
    const FILE_FLAG_DELETE_ON_CLOSE: u32 = 0x0400_0000;
    const INVALID_HANDLE_VALUE: *mut c_void = -1_isize as *mut c_void;

    let path = path
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let attributes = SecurityAttributes {
        length: std::mem::size_of::<SecurityAttributes>() as u32,
        security_descriptor: metadata.security_descriptor.as_ptr().cast_mut().cast(),
        inherit_handle: 0,
    };
    let handle = unsafe {
        CreateFileW(
            path.as_ptr(),
            GENERIC_READ | GENERIC_WRITE | DELETE_ACCESS,
            1,
            &attributes,
            CREATE_NEW,
            FILE_ATTRIBUTE_NORMAL
                | if delete_on_close {
                    FILE_FLAG_DELETE_ON_CLOSE
                } else {
                    0
                },
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        Err(io::Error::last_os_error())
    } else {
        Ok(unsafe { fs::File::from_raw_handle(handle) })
    }
}

#[cfg(target_os = "linux")]
fn create_activation_temp_file(
    transaction: &ActivationTransaction,
    _: Option<&ActivationMetadata>,
    _: bool,
) -> io::Result<fs::File> {
    use std::os::fd::{AsRawFd as _, FromRawFd as _};

    let dot = c".";
    let fd = unsafe {
        libc::openat(
            transaction.parent.file.as_raw_fd(),
            dot.as_ptr(),
            libc::O_RDWR | libc::O_TMPFILE | libc::O_CLOEXEC,
            0o600,
        )
    };
    if fd < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(unsafe { fs::File::from_raw_fd(fd) })
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
fn create_activation_temp_file(
    _: &Path,
    _: Option<&ActivationMetadata>,
    _: bool,
) -> io::Result<fs::File> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "anonymous activation staging is unsupported on this platform",
    ))
}

#[cfg(target_os = "linux")]
fn apply_linux_extended_attributes(
    file: &fs::File,
    path: &Path,
    expected: &[LinuxExtendedAttribute],
) -> Result<()> {
    use std::os::fd::AsRawFd as _;

    let actual = linux_extended_attributes(file, path)?;
    let expected = expected
        .iter()
        .map(|attribute| (attribute.name.clone(), attribute.value.clone()))
        .collect::<BTreeMap<_, _>>();
    let actual = actual
        .into_iter()
        .map(|attribute| (attribute.name, attribute.value))
        .collect::<BTreeMap<_, _>>();
    let fd = file.as_raw_fd();

    for name in actual.keys().filter(|name| !expected.contains_key(*name)) {
        let name = linux_xattr_name(name, path)?;
        let removed = unsafe { libc::fremovexattr(fd, name.as_ptr()) };
        if removed != 0 {
            return Err(io::Error::last_os_error()).with_context(|| {
                format!(
                    "cannot remove an extra activation extended attribute {:?} from staging file: {}",
                    name,
                    display_path(path)
                )
            });
        }
    }
    for (name, value) in &expected {
        if actual.get(name).is_some_and(|actual| actual == value) {
            continue;
        }
        let name = linux_xattr_name(name, path)?;
        let value_pointer = if value.is_empty() {
            std::ptr::null()
        } else {
            value.as_ptr().cast()
        };
        let updated = unsafe { libc::fsetxattr(fd, name.as_ptr(), value_pointer, value.len(), 0) };
        if updated != 0 {
            return Err(io::Error::last_os_error()).with_context(|| {
                format!(
                    "cannot preserve activation extended attribute {:?} on staging file: {}",
                    name,
                    display_path(path)
                )
            });
        }
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn apply_activation_metadata(
    file: &fs::File,
    path: &Path,
    expected: &ActivationMetadata,
) -> Result<()> {
    use std::os::fd::AsRawFd as _;
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};

    let current = file.metadata().with_context(|| {
        format!(
            "cannot inspect activation staging ownership: {}",
            display_path(path)
        )
    })?;
    if current.uid() != expected.uid || current.gid() != expected.gid {
        let changed = unsafe {
            libc::fchown(
                file.as_raw_fd(),
                expected.uid as libc::uid_t,
                expected.gid as libc::gid_t,
            )
        };
        if changed != 0 {
            return Err(io::Error::last_os_error()).with_context(|| {
                format!(
                    "cannot preserve activation owner/group on staging file: {}",
                    display_path(path)
                )
            });
        }
    }
    file.set_permissions(fs::Permissions::from_mode(expected.mode))
        .with_context(|| {
            format!(
                "cannot preserve activation Unix mode on staging file: {}",
                display_path(path)
            )
        })?;
    // Apply ownership and mode first: fchown may clear capabilities and chmod
    // may rewrite the POSIX ACL mask. Restoring the exact fd-bound xattr bytes
    // last preserves both ACL semantics and security labels.
    apply_linux_extended_attributes(file, path, &expected.extended_attributes)?;
    let current_flags = linux_inode_flags(file, path)?;
    let requested_flags = (current_flags & !LINUX_SAFE_COPY_INODE_FLAGS)
        | (expected.inode_flags & LINUX_SAFE_COPY_INODE_FLAGS);
    if requested_flags != current_flags {
        let mut requested = requested_flags as libc::c_int;
        let updated =
            unsafe { libc::ioctl(file.as_raw_fd(), libc::FS_IOC_SETFLAGS, &mut requested) };
        if updated != 0 {
            return Err(io::Error::last_os_error()).with_context(|| {
                format!(
                    "cannot preserve activation inode flags on staging file: {}",
                    display_path(path)
                )
            });
        }
    }
    let actual_metadata = file.metadata().with_context(|| {
        format!(
            "cannot verify activation Unix metadata on staging file: {}",
            display_path(path)
        )
    })?;
    let actual = activation_metadata_from_handle(file, &actual_metadata, path)?;
    if &actual != expected {
        bail!(
            "activation Unix owner/group/mode, POSIX ACL, security extended attributes, or inode flags drifted while staging: {}",
            display_path(path)
        );
    }
    Ok(())
}

#[cfg(all(unix, not(target_os = "linux")))]
fn apply_activation_metadata(_: &fs::File, path: &Path, _: &ActivationMetadata) -> Result<()> {
    bail!(
        "safe handle-bound preservation of POSIX/NFSv4 ACLs, security extended attributes, and BSD file flags is not implemented for this Unix platform; refusing activation replacement: {}",
        display_path(path)
    )
}

#[cfg(windows)]
fn apply_activation_metadata(
    file: &fs::File,
    path: &Path,
    expected: &ActivationMetadata,
) -> Result<()> {
    use std::os::windows::io::AsRawHandle as _;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_BASIC_INFO, FileBasicInfo, SetFileInformationByHandle,
    };

    let basic = FILE_BASIC_INFO {
        FileAttributes: expected.attributes,
        ..Default::default()
    };
    let updated = unsafe {
        SetFileInformationByHandle(
            file.as_raw_handle() as _,
            FileBasicInfo,
            (&raw const basic).cast(),
            std::mem::size_of::<FILE_BASIC_INFO>() as u32,
        )
    };
    if updated == 0 {
        return Err(io::Error::last_os_error()).with_context(|| {
            format!(
                "cannot preserve activation Windows attributes on staging file: {}",
                display_path(path)
            )
        });
    }
    let actual_metadata = file.metadata().with_context(|| {
        format!(
            "cannot verify activation Windows metadata on staging file: {}",
            display_path(path)
        )
    })?;
    let actual = activation_metadata_from_handle(file, &actual_metadata, path)?;
    if &actual != expected {
        bail!(
            "activation Windows owner/group/DACL or attributes drifted while staging: {}; expected attributes=0x{:08x}, actual=0x{:08x}, expected descriptor={} ({} bytes), actual={} ({} bytes), expected security={:?}, actual security={:?}",
            display_path(path),
            expected.attributes,
            actual.attributes,
            sha256_bytes(&expected.security_descriptor),
            expected.security_descriptor.len(),
            sha256_bytes(&actual.security_descriptor),
            actual.security_descriptor.len(),
            expected.security,
            actual.security,
        );
    }
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn apply_activation_metadata(_: &fs::File, path: &Path, _: &ActivationMetadata) -> Result<()> {
    bail!(
        "activation security metadata application is unsupported on this platform: {}",
        display_path(path)
    )
}

#[cfg(unix)]
fn read_bytes_from_handle(file: &fs::File, len: u64, path: &Path) -> Result<Vec<u8>> {
    use std::os::unix::fs::FileExt as _;

    let len = usize::try_from(len).context("activation file is too large to read")?;
    let mut bytes = vec![0_u8; len];
    let mut offset = 0;
    while offset < len {
        let read = file
            .read_at(&mut bytes[offset..], offset as u64)
            .with_context(|| format!("cannot read activation handle: {}", display_path(path)))?;
        if read == 0 {
            bail!(
                "activation handle reached an early EOF: {}",
                display_path(path)
            );
        }
        offset += read;
    }
    Ok(bytes)
}

#[cfg(windows)]
fn read_bytes_from_handle(file: &fs::File, len: u64, path: &Path) -> Result<Vec<u8>> {
    use std::os::windows::fs::FileExt as _;

    let len = usize::try_from(len).context("activation file is too large to read")?;
    let mut bytes = vec![0_u8; len];
    let mut offset = 0;
    while offset < len {
        let read = file
            .seek_read(&mut bytes[offset..], offset as u64)
            .with_context(|| format!("cannot read activation handle: {}", display_path(path)))?;
        if read == 0 {
            bail!(
                "activation handle reached an early EOF: {}",
                display_path(path)
            );
        }
        offset += read;
    }
    Ok(bytes)
}

fn read_file_snapshot_from_handle(file: &fs::File, path: &Path) -> Result<FileSnapshot> {
    let before = file
        .metadata()
        .with_context(|| format!("cannot inspect activation handle: {}", display_path(path)))?;
    if is_link_or_reparse(&before) || !before.file_type().is_file() {
        bail!(
            "activation handle is not an ordinary file: {}",
            display_path(path)
        );
    }
    let before_identity = file_identity_from_handle(file, &before, path)?;
    if !has_strong_file_identity(&before_identity) {
        bail!(
            "activation handle lacks a strong identity: {}",
            display_path(path)
        );
    }
    let before_security = activation_metadata_from_handle(file, &before, path)?;
    let first = read_bytes_from_handle(file, before_identity.len, path)?;
    let middle = file.metadata()?;
    let middle_identity = file_identity_from_handle(file, &middle, path)?;
    let middle_security = activation_metadata_from_handle(file, &middle, path)?;
    let second = read_bytes_from_handle(file, middle_identity.len, path)?;
    let after = file.metadata()?;
    let after_identity = file_identity_from_handle(file, &after, path)?;
    let after_security = activation_metadata_from_handle(file, &after, path)?;
    if first != second
        || before_identity != middle_identity
        || middle_identity != after_identity
        || before_security != middle_security
        || middle_security != after_security
    {
        bail!(
            "activation bytes, identity, or authorization metadata changed during handle-bound capture: {}",
            display_path(path)
        );
    }
    Ok(FileSnapshot {
        bytes: second,
        identity: after_identity,
        metadata: after_security,
    })
}

fn require_single_named_link(snapshot: &FileSnapshot, path: &Path) -> Result<()> {
    #[cfg(unix)]
    if snapshot.identity.nlink != 1 {
        bail!(
            "activation target must have exactly one hard link before replacement (found {}): {}",
            snapshot.identity.nlink,
            display_path(path)
        );
    }
    #[cfg(not(unix))]
    let _ = (snapshot, path);
    Ok(())
}

fn read_file_snapshot(path: &Path) -> Result<FileSnapshot> {
    let file = open_file_no_follow(path).with_context(|| {
        format!(
            "cannot open activation path without following its final component: {}",
            display_path(path)
        )
    })?;
    let first = read_file_snapshot_from_handle(&file, path)?;
    require_single_named_link(&first, path)?;
    let named = open_file_no_follow(path).with_context(|| {
        format!(
            "activation final component changed during handle-bound read: {}",
            display_path(path)
        )
    })?;
    let second = read_file_snapshot_from_handle(&named, path)?;
    require_single_named_link(&second, path)?;
    if !snapshots_match(&first, &second) {
        bail!(
            "activation final component changed identity during read: {}",
            display_path(path)
        );
    }
    Ok(second)
}

fn read_optional_activation_snapshot(path: &Path) -> Result<Option<FileSnapshot>> {
    match read_file_snapshot(path) {
        Ok(snapshot) => Ok(Some(snapshot)),
        Err(error)
            if error
                .downcast_ref::<io::Error>()
                .is_some_and(|source| source.kind() == io::ErrorKind::NotFound) =>
        {
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

fn activation_metadata_scope() -> &'static str {
    #[cfg(windows)]
    return "owner_group_dacl_file_attributes";
    #[cfg(target_os = "linux")]
    return "uid_gid_mode_xattrs_inode_flags";
    #[cfg(not(any(windows, target_os = "linux")))]
    "unsupported_platform"
}

fn activation_metadata_platform_supported() -> bool {
    cfg!(any(windows, target_os = "linux"))
}

fn empty_activation_metadata_probe() -> ActivationMetadataCapabilityProbe {
    ActivationMetadataCapabilityProbe {
        applicable: false,
        probed: false,
        ready: false,
        scope: activation_metadata_scope().to_string(),
        capability_key: ACTIVATION_METADATA_CAPABILITY_KEY.to_string(),
        boundary_class: ACTIVATION_METADATA_BOUNDARY_CLASS.to_string(),
        target: None,
        phase: ActivationMetadataProbePhase::NotApplicable,
        failure_class: None,
        os_error_code: None,
        activation_unchanged: None,
        cleanup_complete: None,
        error: None,
    }
}

fn activation_probe_os_error(error: &anyhow::Error) -> Option<i32> {
    error.chain().find_map(|cause| {
        cause
            .downcast_ref::<io::Error>()
            .and_then(io::Error::raw_os_error)
    })
}

fn activation_probe_io_kind(error: &anyhow::Error, kind: io::ErrorKind) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<io::Error>()
            .is_some_and(|source| source.kind() == kind)
    })
}

fn classify_activation_probe_failure(
    phase: ActivationMetadataProbePhase,
    error: &anyhow::Error,
) -> ActivationMetadataFailureClass {
    use ActivationMetadataFailureClass as Class;
    use ActivationMetadataProbePhase as Phase;

    if phase == Phase::NotApplicable {
        return Class::UnsupportedPlatform;
    }
    if phase == Phase::Cleanup {
        return Class::CleanupFailed;
    }
    if phase == Phase::Verify {
        return Class::MetadataMismatch;
    }
    if phase == Phase::TargetRecheck {
        return Class::ConcurrentChange;
    }
    if activation_probe_io_kind(error, io::ErrorKind::Unsupported) {
        return Class::UnsupportedPlatform;
    }
    if activation_probe_io_kind(error, io::ErrorKind::WouldBlock) {
        return Class::Contention;
    }
    match activation_probe_os_error(error) {
        Some(32 | 33) => return Class::Contention,
        Some(1307 | 1308) => return Class::OwnerAssignmentDenied,
        Some(1314) => return Class::PrivilegeNotHeld,
        _ => {}
    }
    if activation_probe_io_kind(error, io::ErrorKind::PermissionDenied) {
        return Class::PermissionDenied;
    }
    let message = format!("{error:#}");
    if phase == Phase::Capture
        && (activation_probe_io_kind(error, io::ErrorKind::NotFound)
            || message.contains("changed during")
            || message.contains("changed identity during"))
    {
        return Class::ConcurrentChange;
    }
    if phase == Phase::Capture
        && [
            "link",
            "reparse",
            "not an ordinary file",
            "not a real directory",
        ]
        .iter()
        .any(|needle| message.contains(needle))
    {
        return Class::UnsafeTarget;
    }
    Class::IoError
}

fn failed_activation_metadata_probe(
    mut probe: ActivationMetadataCapabilityProbe,
    phase: ActivationMetadataProbePhase,
    error: anyhow::Error,
) -> ActivationMetadataCapabilityProbe {
    probe.phase = phase;
    probe.os_error_code = activation_probe_os_error(&error);
    probe.failure_class = Some(classify_activation_probe_failure(phase, &error));
    probe.error = Some(format!("{error:#}"));
    probe.ready = false;
    probe
}

#[cfg(windows)]
fn probe_activation_metadata_stage(
    target: &Path,
    metadata: &ActivationMetadata,
    phase: &mut ActivationMetadataProbePhase,
    cleanup_complete: &mut Option<bool>,
) -> Result<()> {
    let (mut file, path) = loop {
        *phase = ActivationMetadataProbePhase::StageCreate;
        let path = activation_temp_path(target)?;
        match create_activation_temp_file(&path, Some(metadata), true) {
            Ok(file) => break (file, path),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                *cleanup_complete = Some(true);
                return Err(error).with_context(|| {
                    format!(
                        "cannot create a delete-on-close activation metadata probe beside {}",
                        display_path(target)
                    )
                });
            }
        }
    };

    let operation = (|| -> Result<()> {
        *phase = ActivationMetadataProbePhase::StageWrite;
        file.write_all(ACTIVATION_METADATA_PROBE_BYTES)
            .and_then(|_| file.sync_all())
            .with_context(|| {
                format!(
                    "cannot write the activation metadata probe beside {}",
                    display_path(target)
                )
            })?;
        *phase = ActivationMetadataProbePhase::MetadataApply;
        apply_activation_metadata(&file, target, metadata)?;
        file.sync_all()?;
        *phase = ActivationMetadataProbePhase::Verify;
        let snapshot = read_file_snapshot_from_handle(&file, target)?;
        if snapshot.bytes != ACTIVATION_METADATA_PROBE_BYTES || &snapshot.metadata != metadata {
            bail!("activation metadata probe stage did not preserve bytes and metadata");
        }
        Ok(())
    })();
    let operation_phase = *phase;

    drop(file);
    *phase = ActivationMetadataProbePhase::Cleanup;
    let cleanup = match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).context("cannot verify delete-on-close probe cleanup"),
        Ok(_) => bail!(
            "delete-on-close activation metadata probe still exists: {}",
            display_path(&path)
        ),
    };
    *cleanup_complete = Some(cleanup.is_ok());
    match (operation, cleanup) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) => {
            *phase = operation_phase;
            Err(error)
        }
        (Ok(()), Err(cleanup)) => Err(cleanup),
        (Err(operation), Err(cleanup)) => Err(anyhow::anyhow!(
            "activation metadata probe failed and cleanup also failed: operation={operation:#}; cleanup={cleanup:#}"
        )),
    }
}

#[cfg(target_os = "linux")]
fn probe_activation_metadata_stage(
    target: &Path,
    metadata: &ActivationMetadata,
    phase: &mut ActivationMetadataProbePhase,
    cleanup_complete: &mut Option<bool>,
) -> Result<()> {
    *phase = ActivationMetadataProbePhase::StageCreate;
    let transaction = ActivationTransaction::open(target)?;
    let mut file =
        create_activation_temp_file(&transaction, Some(metadata), true).with_context(|| {
            format!(
                "cannot create an anonymous activation metadata probe beside {}",
                display_path(target)
            )
        })?;
    let operation = (|| -> Result<()> {
        *phase = ActivationMetadataProbePhase::StageWrite;
        file.write_all(ACTIVATION_METADATA_PROBE_BYTES)
            .and_then(|_| file.sync_all())?;
        *phase = ActivationMetadataProbePhase::MetadataApply;
        apply_activation_metadata(&file, target, metadata)?;
        file.sync_all()?;
        *phase = ActivationMetadataProbePhase::Verify;
        let snapshot = read_file_snapshot_from_handle(&file, target)?;
        if snapshot.bytes != ACTIVATION_METADATA_PROBE_BYTES || &snapshot.metadata != metadata {
            bail!("activation metadata probe stage did not preserve bytes and metadata");
        }
        Ok(())
    })();
    let operation_phase = *phase;
    drop(file);
    *cleanup_complete = Some(true);
    *phase = operation_phase;
    operation
}

#[cfg(not(any(windows, target_os = "linux")))]
fn probe_activation_metadata_stage(
    _: &Path,
    _: &ActivationMetadata,
    phase: &mut ActivationMetadataProbePhase,
    _: &mut Option<bool>,
) -> Result<()> {
    *phase = ActivationMetadataProbePhase::StageCreate;
    bail!("activation metadata probes require Linux or Windows handle-bound staging")
}

fn activation_metadata_capability_probe_with<S>(
    root: &Path,
    mut stage_probe: S,
) -> ActivationMetadataCapabilityProbe
where
    S: FnMut(
        &Path,
        &ActivationMetadata,
        &mut ActivationMetadataProbePhase,
        &mut Option<bool>,
    ) -> Result<()>,
{
    let mut probe = empty_activation_metadata_probe();
    let target = match state_paths::managed_state_file(root, Path::new(ACTIVATION_RELATIVE), false)
    {
        Ok(path) => path,
        Err(error) => {
            probe.probed = true;
            return failed_activation_metadata_probe(
                probe,
                ActivationMetadataProbePhase::Capture,
                error,
            );
        }
    };
    match fs::symlink_metadata(&target) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return probe,
        Err(error) => {
            probe.applicable = true;
            probe.probed = true;
            probe.target = Some(display_path(&target));
            return failed_activation_metadata_probe(
                probe,
                ActivationMetadataProbePhase::Capture,
                error.into(),
            );
        }
        Ok(metadata) if is_link_or_reparse(&metadata) || !metadata.file_type().is_file() => {
            probe.applicable = true;
            probe.probed = true;
            probe.target = Some(display_path(&target));
            return failed_activation_metadata_probe(
                probe,
                ActivationMetadataProbePhase::Capture,
                anyhow::anyhow!(
                    "activation metadata probe target is a link, reparse point, or non-file: {}",
                    display_path(&target)
                ),
            );
        }
        Ok(_) => {}
    }

    probe.target = Some(display_path(&target));
    if !activation_metadata_platform_supported() {
        return failed_activation_metadata_probe(
            probe,
            ActivationMetadataProbePhase::NotApplicable,
            anyhow::anyhow!(
                "activation metadata probes are unsupported on this platform: {}",
                display_path(&target)
            ),
        );
    }
    probe.applicable = true;
    probe.probed = true;
    probe.phase = ActivationMetadataProbePhase::Lock;
    let _lock = match crate::state_lock::acquire_state_lock(&target) {
        Ok(lock) => lock,
        Err(error) => {
            return failed_activation_metadata_probe(
                probe,
                ActivationMetadataProbePhase::Lock,
                error,
            );
        }
    };
    probe.phase = ActivationMetadataProbePhase::Capture;
    let before = match read_file_snapshot(&target) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            return failed_activation_metadata_probe(
                probe,
                ActivationMetadataProbePhase::Capture,
                error,
            );
        }
    };

    let mut phase = ActivationMetadataProbePhase::StageCreate;
    let mut cleanup_complete = None;
    let stage_error = stage_probe(&target, &before.metadata, &mut phase, &mut cleanup_complete)
        .err()
        .map(|error| (phase, error));
    probe.cleanup_complete = cleanup_complete;

    probe.phase = ActivationMetadataProbePhase::TargetRecheck;
    let after = match read_file_snapshot(&target) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            return failed_activation_metadata_probe(
                probe,
                ActivationMetadataProbePhase::TargetRecheck,
                error,
            );
        }
    };
    if !snapshots_match(&before, &after) {
        probe.activation_unchanged = Some(false);
        return failed_activation_metadata_probe(
            probe,
            ActivationMetadataProbePhase::TargetRecheck,
            anyhow::anyhow!("activation changed during the metadata capability probe"),
        );
    }
    probe.activation_unchanged = Some(true);
    if let Some((failure_phase, error)) = stage_error {
        return failed_activation_metadata_probe(probe, failure_phase, error);
    }

    probe.phase = ActivationMetadataProbePhase::Complete;
    probe.ready = true;
    probe
}

pub fn activation_metadata_capability_probe(root: &Path) -> ActivationMetadataCapabilityProbe {
    activation_metadata_capability_probe_with(root, probe_activation_metadata_stage)
}

fn create_owned_activation_temp(
    transaction: &ActivationTransaction,
    bytes: &[u8],
    metadata: Option<&ActivationMetadata>,
) -> Result<OwnedActivationTemp> {
    let target = &transaction.target;
    for _ in 0..MAX_ACTIVATION_TEMP_ATTEMPTS {
        #[cfg(windows)]
        let named_path = activation_temp_path(target)?;
        #[cfg(windows)]
        let created = create_activation_temp_file(named_path.as_path(), metadata, false);
        #[cfg(target_os = "linux")]
        let created = create_activation_temp_file(transaction, metadata, false);
        #[cfg(not(any(windows, target_os = "linux")))]
        let created = create_activation_temp_file(target, metadata, false);
        let mut file = match created {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "cannot create an exclusive handle-bound activation staging inode beside {}",
                        display_path(target)
                    )
                });
            }
        };
        file.write_all(bytes)
            .and_then(|_| file.sync_all())
            .with_context(|| {
                format!(
                    "cannot sync activation staging inode beside {}",
                    display_path(target)
                )
            })?;
        if let Some(metadata) = metadata {
            apply_activation_metadata(&file, target, metadata)?;
            file.sync_all()?;
        }
        let snapshot = read_file_snapshot_from_handle(&file, target)?;
        if snapshot.bytes != bytes {
            bail!("activation staging bytes changed through their bound handle");
        }
        #[cfg(target_os = "linux")]
        {
            if snapshot.identity.nlink != 0 {
                bail!(
                    "anonymous activation staging inode unexpectedly has links before publication"
                );
            }
            let retained_path = transaction.retained_path()?;
            let retained_name = retained_path.file_name().ok_or_else(|| {
                anyhow::anyhow!("activation retained path has no final component")
            })?;
            link_linux_activation_temp_to_retained(&file, transaction, retained_name)
                .with_context(|| {
                    format!(
                        "cannot bind the activation publication preflight stage in {}",
                        display_path(&transaction.retained.path)
                    )
                })?;

            let rebound = (|| -> Result<FileSnapshot> {
                sync_activation_retained(transaction)?;
                let held = read_file_snapshot_from_handle(&file, &retained_path)?;
                if held.identity.device != snapshot.identity.device
                    || held.identity.inode != snapshot.identity.inode
                    || held.identity.nlink != 1
                    || held.bytes != snapshot.bytes
                    || held.metadata != snapshot.metadata
                {
                    bail!(
                        "named activation publication preflight stage changed inode, bytes, or authorization metadata"
                    );
                }
                let rebound = read_transaction_retained_snapshot(transaction, &retained_path)?;
                if rebound != held {
                    bail!(
                        "named activation publication preflight stage did not rebind to its held inode"
                    );
                }
                Ok(rebound)
            })()
            .with_context(|| {
                format!(
                    "activation publication preflight stage was named but could not be verified; retained at {}",
                    display_path(&retained_path)
                )
            })?;
            return Ok(OwnedActivationTemp {
                file,
                path: Some(retained_path),
                published: false,
                target: target.to_path_buf(),
                snapshot: rebound,
            });
        }
        #[cfg(windows)]
        return Ok(OwnedActivationTemp {
            file,
            path: Some(named_path),
            published: false,
            target: target.to_path_buf(),
            snapshot,
        });
    }
    bail!(
        "cannot allocate an exclusive activation staging inode after {MAX_ACTIVATION_TEMP_ATTEMPTS} attempts beside {}",
        display_path(target)
    )
}

#[cfg(target_os = "linux")]
fn link_linux_activation_temp_to_retained(
    file: &fs::File,
    transaction: &ActivationTransaction,
    retained_name: &std::ffi::OsStr,
) -> io::Result<()> {
    use std::os::fd::AsRawFd as _;
    use std::os::unix::ffi::OsStrExt as _;

    let retained_name = std::ffi::CString::new(retained_name.as_bytes()).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "activation retained name contains NUL",
        )
    })?;
    let source = std::ffi::CString::new(format!("/proc/self/fd/{}", file.as_raw_fd()))
        .expect("decimal file descriptor path contains no NUL");
    let linked = unsafe {
        libc::linkat(
            libc::AT_FDCWD,
            source.as_ptr(),
            transaction.retained.file.as_raw_fd(),
            retained_name.as_ptr(),
            libc::AT_SYMLINK_FOLLOW,
        )
    };
    if linked == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(target_os = "linux")]
fn rename_linux_activation_no_replace(
    source_directory: &ActivationDirectory,
    source_name: &std::ffi::OsStr,
    target_directory: &ActivationDirectory,
    target_name: &std::ffi::OsStr,
) -> io::Result<()> {
    use std::os::fd::AsRawFd as _;
    use std::os::unix::ffi::OsStrExt as _;

    let source = std::ffi::CString::new(source_name.as_bytes()).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "activation source name contains NUL",
        )
    })?;
    let target = std::ffi::CString::new(target_name.as_bytes()).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "activation target name contains NUL",
        )
    })?;
    let moved = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            source_directory.file.as_raw_fd(),
            source.as_ptr(),
            target_directory.file.as_raw_fd(),
            target.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if moved == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(target_os = "linux")]
fn publish_linux_activation_temp(
    temp: &OwnedActivationTemp,
    transaction: &ActivationTransaction,
) -> io::Result<()> {
    let path = temp.path.as_deref().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "Linux activation publication stage has no retained path",
        )
    })?;
    if path.parent() != Some(transaction.retained.path.as_path()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Linux activation publication stage is outside the retained directory",
        ));
    }
    let name = path.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "Linux activation publication stage has no final component",
        )
    })?;
    rename_linux_activation_no_replace(
        &transaction.retained,
        name,
        &transaction.parent,
        &transaction.target_name,
    )
}

#[cfg(windows)]
fn rename_windows_file_handle_no_replace(
    file: &fs::File,
    directory: &ActivationDirectory,
    name: &std::ffi::OsStr,
) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt as _;
    use std::os::windows::io::AsRawHandle as _;
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::Storage::FileSystem::FILE_RENAME_INFO;

    #[repr(C)]
    struct IoStatusBlock {
        status_or_pointer: *mut std::ffi::c_void,
        information: usize,
    }

    #[link(name = "ntdll")]
    unsafe extern "system" {
        fn NtSetInformationFile(
            file_handle: HANDLE,
            io_status_block: *mut IoStatusBlock,
            file_information: *mut std::ffi::c_void,
            length: u32,
            file_information_class: u32,
        ) -> i32;
        fn RtlNtStatusToDosError(status: i32) -> u32;
    }
    const FILE_RENAME_INFORMATION_CLASS: u32 = 10;

    let directory_metadata = directory.file.metadata()?;
    if !directory_metadata.is_dir() || is_link_or_reparse(&directory_metadata) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "activation parent is not a real directory",
        ));
    }
    let name = name.encode_wide().collect::<Vec<_>>();
    if name.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "activation target name contains NUL",
        ));
    }
    let offset = std::mem::offset_of!(FILE_RENAME_INFO, FileName);
    let bytes = offset + name.len() * std::mem::size_of::<u16>();
    // Some supported Windows builds inspect a trailing WCHAR even though
    // FileNameLength excludes it. Allocate a zero terminator outside the
    // counted buffer, matching the native installer transaction.
    let allocation_bytes = bytes + std::mem::size_of::<u16>();
    let words = allocation_bytes.div_ceil(std::mem::size_of::<usize>());
    let mut storage = vec![0_usize; words];
    let information = storage.as_mut_ptr().cast::<FILE_RENAME_INFO>();
    unsafe {
        (*information).Anonymous.ReplaceIfExists = false;
        (*information).RootDirectory = directory.file.as_raw_handle() as _;
        (*information).FileNameLength = u32::try_from(name.len() * 2).unwrap();
        std::ptr::copy_nonoverlapping(
            name.as_ptr(),
            (*information).FileName.as_mut_ptr(),
            name.len(),
        );
    }
    let mut io_status = IoStatusBlock {
        status_or_pointer: std::ptr::null_mut(),
        information: 0,
    };
    let status = unsafe {
        NtSetInformationFile(
            file.as_raw_handle() as _,
            &mut io_status,
            storage.as_mut_ptr().cast(),
            u32::try_from(bytes).unwrap(),
            FILE_RENAME_INFORMATION_CLASS,
        )
    };
    if status < 0 {
        let win32 = unsafe { RtlNtStatusToDosError(status) };
        Err(io::Error::from_raw_os_error(win32 as i32))
    } else {
        Ok(())
    }
}

fn publish_owned_activation_temp(
    temp: &mut OwnedActivationTemp,
    transaction: &ActivationTransaction,
) -> Result<()> {
    let target = &transaction.target;
    let before = read_file_snapshot_from_handle(&temp.file, target)?;
    if before != temp.snapshot {
        bail!("activation staging inode changed before handle-bound publication");
    }
    #[cfg(target_os = "linux")]
    publish_linux_activation_temp(temp, transaction)?;
    #[cfg(windows)]
    rename_windows_file_handle_no_replace(
        &temp.file,
        &transaction.parent,
        &transaction.target_name,
    )?;
    #[cfg(not(any(windows, target_os = "linux")))]
    bail!("handle-bound activation publication is unsupported on this platform");
    // From this point the stage is at the canonical name. Record that fact
    // before any fallible durability or verification step so callers never
    // mistake a partially completed publication for a private retained stage.
    temp.path = Some(target.to_path_buf());
    temp.published = true;
    let after = read_file_snapshot_from_handle(&temp.file, target)
        .context("cannot snapshot named activation publication handle")?;
    #[cfg(unix)]
    if before.identity.device != after.identity.device
        || before.identity.inode != after.identity.inode
    {
        bail!("published activation handle changed inode identity");
    }
    #[cfg(windows)]
    if before.identity.volume_serial_number != after.identity.volume_serial_number
        || before.identity.file_index != after.identity.file_index
        || before.identity.volume_serial_number_64 != after.identity.volume_serial_number_64
        || before.identity.file_id_128 != after.identity.file_id_128
    {
        bail!("published activation handle changed file identity");
    }
    if before.bytes != after.bytes || before.metadata != after.metadata {
        bail!("published activation handle changed bytes or authorization metadata");
    }
    require_single_named_link(&after, target)?;
    temp.snapshot = after;
    #[cfg(target_os = "linux")]
    sync_activation_retained(transaction)
        .context("cannot sync the source directory of named activation publication")?;
    sync_activation_parent(transaction)
        .context("cannot sync the destination directory of named activation publication")?;
    verify_transaction_target_snapshot(
        transaction,
        &temp.snapshot,
        "handle-bound activation publication",
    )
    .context("cannot bind named activation publication to its held parent directory")
}

fn publish_owned_activation_temp_to_retained(
    temp: &mut OwnedActivationTemp,
    transaction: &ActivationTransaction,
    action: &str,
) -> Result<RetainedActivationEvidence> {
    #[cfg(target_os = "linux")]
    let path = temp.path.clone().ok_or_else(|| {
        anyhow::anyhow!("Linux activation recovery stage has no durable retained path")
    })?;
    #[cfg(windows)]
    let path = transaction.retained_path()?;
    let name = path
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("activation retained path has no final component"))?;
    let before = read_file_snapshot_from_handle(&temp.file, &path)?;
    if before != temp.snapshot {
        bail!("activation recovery staging inode changed before retained publication");
    }
    #[cfg(target_os = "linux")]
    {
        if path.parent() != Some(transaction.retained.path.as_path()) || temp.published {
            bail!("Linux activation recovery stage is not a private retained publication");
        }
        let _ = name;
    }
    #[cfg(windows)]
    {
        rename_windows_file_handle_no_replace(&temp.file, &transaction.retained, name)
            .context("cannot publish verified activation recovery handle to managed temp")?;
        temp.path = Some(path.clone());
        temp.published = true;
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    bail!("retained activation publication is unsupported on this platform");

    let after = read_file_snapshot_from_handle(&temp.file, &path)?;
    #[cfg(unix)]
    if before.identity.device != after.identity.device
        || before.identity.inode != after.identity.inode
    {
        bail!("retained activation publication changed inode identity");
    }
    #[cfg(windows)]
    if before.identity.volume_serial_number != after.identity.volume_serial_number
        || before.identity.file_index != after.identity.file_index
        || before.identity.volume_serial_number_64 != after.identity.volume_serial_number_64
        || before.identity.file_id_128 != after.identity.file_id_128
    {
        bail!("retained activation publication changed file identity");
    }
    if before.bytes != after.bytes || before.metadata != after.metadata {
        bail!("retained activation publication changed bytes or authorization metadata");
    }
    require_single_named_link(&after, &path)?;
    temp.snapshot = after;
    #[cfg(target_os = "linux")]
    sync_activation_retained(transaction)?;
    verify_transaction_retained_snapshot(transaction, &path, &temp.snapshot, action)?;
    Ok(retained_evidence(action, &path, &temp.snapshot, None))
}

fn persist_snapshot_to_retained(
    transaction: &ActivationTransaction,
    snapshot: &FileSnapshot,
    action: &str,
) -> Result<RetainedActivationEvidence> {
    let mut stage =
        create_owned_activation_temp(transaction, &snapshot.bytes, Some(&snapshot.metadata))?;
    match publish_owned_activation_temp_to_retained(&mut stage, transaction, action) {
        Ok(evidence) => Ok(evidence),
        Err(primary)
            if stage
                .path
                .as_deref()
                .is_some_and(|path| path.parent() == Some(transaction.retained.path.as_path())) =>
        {
            bail!(
                "verified activation recovery was named but post-publication verification failed: {primary:#}; retained at {}",
                display_path(stage.path.as_deref().expect("named recovery path"))
            )
        }
        Err(primary) if stage.published => bail!(
            "verified activation recovery was named but post-publication verification failed: {primary:#}; retained at {}",
            display_path(stage.path.as_deref().expect("named recovery path"))
        ),
        Err(primary) => {
            let cleanup =
                cleanup_owned_activation_temp(stage, "failed retained recovery staging cleanup");
            bail!(
                "cannot persist verified activation recovery: {primary:#}; staging cleanup={cleanup:?}"
            )
        }
    }
}

fn restore_or_persist_snapshot(
    transaction: &ActivationTransaction,
    snapshot: &FileSnapshot,
    action: &str,
) -> String {
    match restore_snapshot_bytes_no_replace(transaction, snapshot, action) {
        Ok(()) => "restored canonical activation without overwrite".to_string(),
        Err(restore) => match persist_snapshot_to_retained(
            transaction,
            snapshot,
            &format!("{action} retained recovery"),
        ) {
            Ok(evidence) => format!(
                "canonical restoration failed ({restore:#}); persisted recovery evidence: {}",
                serde_json::to_string(&evidence).unwrap_or_else(|_| format!("{:?}", evidence))
            ),
            Err(retain) => format!(
                "canonical restoration failed ({restore:#}); retained recovery publication also failed ({retain:#})"
            ),
        },
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

fn cleanup_owned_activation_temp(temp: OwnedActivationTemp, action: &str) -> Result<()> {
    let actual = read_file_snapshot_from_handle(&temp.file, &temp.target)?;
    if actual != temp.snapshot {
        bail!("refusing to clean a changed activation staging handle during {action}");
    }
    #[cfg(target_os = "linux")]
    {
        if temp.published {
            bail!(
                "refusing to unlink the canonical Linux activation inode during {action}: {}",
                display_path(temp.path.as_deref().unwrap_or(&temp.target))
            );
        }
        if let Some(path) = temp.path.as_deref() {
            bail!(
                "refusing to unlink a durable private Linux activation stage during {action}; retained at {}",
                display_path(path)
            );
        }
        drop(temp);
        return Ok(());
    }
    #[cfg(windows)]
    {
        let path = temp
            .path
            .clone()
            .ok_or_else(|| anyhow::anyhow!("Windows activation staging path is missing"))?;
        delete_windows_file_by_handle(&temp.file).with_context(|| {
            format!(
                "cannot mark the exact activation staging handle for deletion: {}",
                display_path(&path)
            )
        })?;
        drop(temp);
        match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error).context("cannot verify handle-bound staging cleanup"),
            Ok(_) => bail!(
                "activation staging path still exists after handle-bound cleanup: {}",
                display_path(&path)
            ),
        }
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    bail!("handle-bound activation cleanup is unsupported on this platform")
}

#[cfg(target_os = "linux")]
fn open_activation_directory(path: &Path) -> Result<ActivationDirectory> {
    use std::os::unix::fs::OpenOptionsExt as _;

    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
        .with_context(|| {
            format!(
                "cannot open activation transaction directory without following links: {}",
                display_path(path)
            )
        })?;
    let metadata = file.metadata()?;
    if !metadata.is_dir() || is_link_or_reparse(&metadata) {
        bail!(
            "activation transaction directory is not a real directory: {}",
            display_path(path)
        );
    }
    Ok(ActivationDirectory {
        path: path.to_path_buf(),
        file,
    })
}

#[cfg(windows)]
fn open_activation_directory(path: &Path) -> Result<ActivationDirectory> {
    use std::os::windows::fs::OpenOptionsExt as _;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };

    let file = fs::OpenOptions::new()
        .read(true)
        // Deliberately omit FILE_SHARE_DELETE: the verified directory name
        // cannot be renamed or removed while the activation transaction holds
        // this capability.
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .with_context(|| {
            format!(
                "cannot open activation transaction directory without following reparse points: {}",
                display_path(path)
            )
        })?;
    let metadata = file.metadata()?;
    if !metadata.is_dir() || is_link_or_reparse(&metadata) {
        bail!(
            "activation transaction directory is not a real directory: {}",
            display_path(path)
        );
    }
    Ok(ActivationDirectory {
        path: path.to_path_buf(),
        file,
    })
}

#[cfg(not(any(windows, target_os = "linux")))]
fn open_activation_directory(path: &Path) -> Result<ActivationDirectory> {
    bail!(
        "activation transaction directories are unsupported on this platform: {}",
        display_path(path)
    )
}

#[cfg(target_os = "linux")]
fn open_or_create_activation_child(
    parent: &ActivationDirectory,
    name: &std::ffi::CStr,
) -> Result<ActivationDirectory> {
    use std::os::fd::{AsRawFd as _, FromRawFd as _};
    use std::os::unix::ffi::OsStrExt as _;
    use std::os::unix::fs::MetadataExt as _;

    let created = unsafe { libc::mkdirat(parent.file.as_raw_fd(), name.as_ptr(), 0o700) };
    if created != 0 {
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::AlreadyExists {
            return Err(error).with_context(|| {
                format!(
                    "cannot create managed activation transaction directory below {}",
                    display_path(&parent.path)
                )
            });
        }
    } else {
        parent.file.sync_all().with_context(|| {
            format!(
                "cannot durably sync newly created activation transaction directory below {}",
                display_path(&parent.path)
            )
        })?;
    }
    let fd = unsafe {
        libc::openat(
            parent.file.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error()).with_context(|| {
            format!(
                "cannot bind managed activation transaction directory below {}",
                display_path(&parent.path)
            )
        });
    }
    let file = unsafe { fs::File::from_raw_fd(fd) };
    let metadata = file.metadata()?;
    let parent_metadata = parent.file.metadata()?;
    if !metadata.is_dir()
        || is_link_or_reparse(&metadata)
        || metadata.dev() != parent_metadata.dev()
    {
        bail!(
            "managed activation transaction directory is not a same-filesystem real directory below {}",
            display_path(&parent.path)
        );
    }
    if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o022 != 0 {
        bail!(
            "managed activation transaction directory is not owned by the effective user with group/other writes disabled below {}",
            display_path(&parent.path)
        );
    }
    let child_name = std::ffi::OsStr::from_bytes(name.to_bytes());
    Ok(ActivationDirectory {
        path: parent.path.join(child_name),
        file,
    })
}

#[cfg(windows)]
fn open_or_create_activation_child(
    parent: &ActivationDirectory,
    name: &std::ffi::CStr,
) -> Result<ActivationDirectory> {
    let name = std::str::from_utf8(name.to_bytes())
        .context("managed activation transaction directory name is not UTF-8")?;
    let path = parent.path.join(name);
    match fs::create_dir(&path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => {
            return Err(error).with_context(|| {
                format!(
                    "cannot create managed activation transaction directory {}",
                    display_path(&path)
                )
            });
        }
    }
    open_activation_directory(&path)
}

#[cfg(not(any(windows, target_os = "linux")))]
fn open_or_create_activation_child(
    parent: &ActivationDirectory,
    _: &std::ffi::CStr,
) -> Result<ActivationDirectory> {
    bail!(
        "managed activation transaction directories are unsupported below {}",
        display_path(&parent.path)
    )
}

fn activation_retained_directory(parent: &ActivationDirectory) -> Result<ActivationDirectory> {
    let temp = open_or_create_activation_child(parent, c"tmp")?;
    open_or_create_activation_child(&temp, c"activation-retained")
}

impl ActivationTransaction {
    fn open(target: &Path) -> Result<Self> {
        let parent_path = target
            .parent()
            .ok_or_else(|| anyhow::anyhow!("activation target has no state directory"))?;
        let target_name = target
            .file_name()
            .ok_or_else(|| anyhow::anyhow!("activation target has no final component"))?
            .to_os_string();
        let parent = open_activation_directory(parent_path)?;
        let retained = activation_retained_directory(&parent)?;
        Ok(Self {
            target: target.to_path_buf(),
            target_name,
            parent,
            retained,
        })
    }

    fn retained_path(&self) -> Result<PathBuf> {
        let template = self.retained.path.join(&self.target_name);
        activation_temp_path(&template)
    }
}

#[cfg(windows)]
fn open_activation_capture_handle(transaction: &ActivationTransaction) -> io::Result<fs::File> {
    use std::os::windows::fs::OpenOptionsExt as _;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };
    fs::OpenOptions::new()
        .access_mode(0x8003_0000)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(&transaction.target)
}

#[cfg(target_os = "linux")]
fn open_activation_capture_handle(transaction: &ActivationTransaction) -> io::Result<fs::File> {
    use std::os::fd::{AsRawFd as _, FromRawFd as _};
    use std::os::unix::ffi::OsStrExt as _;

    let name = std::ffi::CString::new(transaction.target_name.as_bytes()).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "activation target name contains NUL",
        )
    })?;
    let fd = unsafe {
        libc::openat(
            transaction.parent.file.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(unsafe { fs::File::from_raw_fd(fd) })
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
fn open_activation_capture_handle(_: &ActivationTransaction) -> io::Result<fs::File> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "handle-bound activation capture is unsupported on this platform",
    ))
}

#[cfg(target_os = "linux")]
fn rename_linux_activation_to_retained(
    transaction: &ActivationTransaction,
    retained_name: &std::ffi::OsStr,
) -> io::Result<()> {
    rename_linux_activation_no_replace(
        &transaction.parent,
        &transaction.target_name,
        &transaction.retained,
        retained_name,
    )
}

#[cfg(target_os = "linux")]
fn open_activation_directory_file(
    directory: &ActivationDirectory,
    name: &std::ffi::OsStr,
) -> io::Result<fs::File> {
    use std::os::fd::{AsRawFd as _, FromRawFd as _};
    use std::os::unix::ffi::OsStrExt as _;

    let name = std::ffi::CString::new(name.as_bytes()).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "activation file name contains NUL",
        )
    })?;
    let fd = unsafe {
        libc::openat(
            directory.file.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(unsafe { fs::File::from_raw_fd(fd) })
    }
}

#[cfg(windows)]
fn open_activation_directory_file(
    directory: &ActivationDirectory,
    name: &std::ffi::OsStr,
) -> io::Result<fs::File> {
    use std::os::windows::fs::OpenOptionsExt as _;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };

    fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(directory.path.join(name))
}

#[cfg(not(any(windows, target_os = "linux")))]
fn open_activation_directory_file(
    _: &ActivationDirectory,
    _: &std::ffi::OsStr,
) -> io::Result<fs::File> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "handle-bound activation directory reads are unsupported on this platform",
    ))
}

fn read_activation_directory_snapshot(
    directory: &ActivationDirectory,
    name: &std::ffi::OsStr,
    display: &Path,
) -> Result<FileSnapshot> {
    let first_file = open_activation_directory_file(directory, name).with_context(|| {
        format!(
            "cannot open activation transaction leaf through its directory handle: {}",
            display_path(display)
        )
    })?;
    let first = read_file_snapshot_from_handle(&first_file, display)?;
    require_single_named_link(&first, display)?;
    let second_file = open_activation_directory_file(directory, name).with_context(|| {
        format!(
            "activation transaction leaf changed after its first handle-bound read: {}",
            display_path(display)
        )
    })?;
    let second = read_file_snapshot_from_handle(&second_file, display)?;
    require_single_named_link(&second, display)?;
    if !snapshots_match(&first, &second) {
        bail!(
            "activation transaction leaf changed identity during handle-bound read: {}",
            display_path(display)
        );
    }
    Ok(second)
}

fn read_transaction_target_snapshot(transaction: &ActivationTransaction) -> Result<FileSnapshot> {
    read_activation_directory_snapshot(
        &transaction.parent,
        &transaction.target_name,
        &transaction.target,
    )
}

fn read_transaction_retained_snapshot(
    transaction: &ActivationTransaction,
    retained_path: &Path,
) -> Result<FileSnapshot> {
    let name = retained_path
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("activation retained path has no final component"))?;
    read_activation_directory_snapshot(&transaction.retained, name, retained_path)
}

fn verify_transaction_target_snapshot(
    transaction: &ActivationTransaction,
    expected: &FileSnapshot,
    action: &str,
) -> Result<()> {
    let actual = read_transaction_target_snapshot(transaction)?;
    if !snapshots_match(expected, &actual) {
        bail!(
            "activation file identity, bytes, or authorization metadata changed during {action}: {}",
            display_path(&transaction.target)
        );
    }
    Ok(())
}

fn verify_transaction_retained_snapshot(
    transaction: &ActivationTransaction,
    retained_path: &Path,
    expected: &FileSnapshot,
    action: &str,
) -> Result<()> {
    let actual = read_transaction_retained_snapshot(transaction, retained_path)?;
    if !snapshots_match(expected, &actual) {
        bail!(
            "retained activation identity, bytes, or authorization metadata changed during {action}: {}",
            display_path(retained_path)
        );
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ActivationCapturePhase {
    BeforeMove,
    AfterMoveNotFound,
}

fn capture_activation_target_with<H>(
    transaction: &ActivationTransaction,
    action: &str,
    capture_hook: &mut H,
) -> Result<Option<CapturedActivation>>
where
    H: FnMut(ActivationCapturePhase, &Path) -> Result<()>,
{
    let target = &transaction.target;
    let file = match open_activation_capture_handle(transaction) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| {
                format!(
                    "cannot open activation target for {action}: {}",
                    display_path(target)
                )
            });
        }
    };
    let expected = read_file_snapshot_from_handle(&file, target)?;
    require_single_named_link(&expected, target)?;
    capture_hook(ActivationCapturePhase::BeforeMove, target)?;
    for _ in 0..MAX_ACTIVATION_TEMP_ATTEMPTS {
        let path = transaction.retained_path()?;
        let retained_name = path
            .file_name()
            .ok_or_else(|| anyhow::anyhow!("activation retained path has no final component"))?;
        #[cfg(windows)]
        let moved =
            rename_windows_file_handle_no_replace(&file, &transaction.retained, retained_name);
        #[cfg(target_os = "linux")]
        let moved = rename_linux_activation_to_retained(transaction, retained_name);
        #[cfg(not(any(windows, target_os = "linux")))]
        let moved = Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "activation capture unsupported",
        ));
        match moved {
            Ok(()) => {
                let post_move = (|| -> Result<()> {
                    sync_activation_parent(transaction)?;
                    #[cfg(target_os = "linux")]
                    transaction.retained.file.sync_all()?;
                    let rebound = read_file_snapshot_from_handle(&file, &path)?;
                    if rebound != expected {
                        bail!("captured activation handle changed after rename");
                    }
                    let named = read_transaction_retained_snapshot(transaction, &path)
                        .with_context(|| {
                            retained_snapshot_message(
                                "captured activation path could not be rebound to its open handle",
                                &path,
                            )
                        })?;
                    if named != expected {
                        bail!("captured activation path is not the verified source handle");
                    }
                    Ok(())
                })();
                match post_move {
                    Ok(()) => {
                        return Ok(Some(CapturedActivation {
                            file,
                            path,
                            snapshot: expected,
                        }));
                    }
                    Err(error) => {
                        let recovery = restore_or_persist_snapshot(
                            transaction,
                            &expected,
                            "capture post-move recovery",
                        );
                        bail!(
                            "captured activation move completed but post-move verification failed: {error:#}; recovery={recovery}; retained evidence: {}",
                            display_path(&path)
                        );
                    }
                }
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                capture_hook(ActivationCapturePhase::AfterMoveNotFound, target)?;
                let recovery =
                    restore_or_persist_snapshot(transaction, &expected, "capture-race restoration");
                bail!(
                    "activation disappeared during handle-bound capture; recovery={recovery}: {}",
                    display_path(target)
                );
            }
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "cannot capture activation target for {action} without overwrite: {}",
                        display_path(target)
                    )
                });
            }
        }
    }
    bail!(
        "cannot allocate retained activation evidence after {MAX_ACTIVATION_TEMP_ATTEMPTS} attempts"
    )
}

fn capture_activation_target(
    transaction: &ActivationTransaction,
    action: &str,
) -> Result<Option<CapturedActivation>> {
    let mut capture_hook = |_: ActivationCapturePhase, _: &Path| Ok(());
    capture_activation_target_with(transaction, action, &mut capture_hook)
}

fn restore_snapshot_bytes_no_replace(
    transaction: &ActivationTransaction,
    captured: &FileSnapshot,
    action: &str,
) -> Result<()> {
    let mut stage =
        create_owned_activation_temp(transaction, &captured.bytes, Some(&captured.metadata))?;
    if let Err(primary) = publish_owned_activation_temp(&mut stage, transaction) {
        if stage.published {
            bail!(
                "activation restore became named but durability/verification failed for {action}: {primary:#}; canonical recovery remains at {}",
                display_path(&transaction.target)
            );
        }
        let cleanup =
            cleanup_owned_activation_temp(stage, "failed activation restore staging cleanup");
        bail!(
            "cannot atomically restore activation bytes for {action}; primary: {primary:#}; cleanup={cleanup:?}"
        );
    }
    verify_transaction_target_snapshot(transaction, &stage.snapshot, action)
}

fn finalize_captured_activation(
    transaction: &ActivationTransaction,
    captured: CapturedActivation,
    action: &str,
) -> Result<Option<RetainedActivationEvidence>> {
    let actual = read_file_snapshot_from_handle(&captured.file, &captured.path)?;
    if actual != captured.snapshot {
        bail!(
            "captured activation handle changed during {action}; retained evidence: {}",
            display_path(&captured.path)
        );
    }
    let rebound = read_transaction_retained_snapshot(transaction, &captured.path)?;
    if !snapshots_match(&captured.snapshot, &rebound) {
        bail!(
            "captured activation path changed during {action}; retained evidence: {}",
            display_path(&captured.path)
        );
    }
    #[cfg(target_os = "linux")]
    {
        let evidence = retained_evidence(action, &captured.path, &captured.snapshot, None);
        drop(captured.file);
        return Ok(Some(evidence));
    }
    #[cfg(windows)]
    {
        if let Err(error) = delete_windows_file_by_handle(&captured.file) {
            let evidence = retained_evidence(
                action,
                &captured.path,
                &captured.snapshot,
                Some(error.to_string()),
            );
            drop(captured.file);
            return Ok(Some(evidence));
        }
        drop(captured.file);
        match fs::symlink_metadata(&captured.path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error).context("cannot verify captured activation handle cleanup"),
            Ok(_) => bail!(
                "captured activation path still exists after exact-handle cleanup: {}",
                display_path(&captured.path)
            ),
        }
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    bail!("captured activation finalization is unsupported on this platform")
}

fn retained_snapshot_message(reason: &str, retained: &Path) -> String {
    format!(
        "{reason}; preserved an independent recovery snapshot at {}",
        display_path(retained)
    )
}

fn retained_snapshots_message(reason: &str, first: &Path, second: &Path) -> String {
    format!(
        "{reason}; preserved independent recovery snapshots at {} and {}",
        display_path(first),
        display_path(second)
    )
}
#[cfg(windows)]
fn open_file_no_follow(path: &Path) -> io::Result<fs::File> {
    use std::os::windows::fs::OpenOptionsExt as _;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };

    fs::OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .open(path)
}

#[cfg(unix)]
fn open_file_no_follow(path: &Path) -> io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt as _;

    fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
}

#[cfg(not(any(windows, unix)))]
fn open_file_no_follow(_: &Path) -> io::Result<fs::File> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "handle-bound no-follow activation reads are unsupported on this platform",
    ))
}
fn ensure_ordinary_skill_path(path: &Path) -> Result<fs::Metadata> {
    let mut ancestors = path
        .ancestors()
        .filter(|candidate| !candidate.as_os_str().is_empty())
        .collect::<Vec<_>>();
    ancestors.reverse();
    let mut final_metadata = None;
    for candidate in ancestors {
        let metadata = fs::symlink_metadata(candidate).with_context(|| {
            format!("无法读取 skill_file 路径组件: {}", display_path(candidate))
        })?;
        if is_link_or_reparse(&metadata) {
            bail!(
                "skill_file 路径不得经过链接/reparse: {}",
                display_path(candidate)
            );
        }
        if candidate == path {
            if !metadata.file_type().is_file() {
                bail!("skill_file 不是普通文件: {}", display_path(candidate));
            }
            final_metadata = Some(metadata);
        } else if !metadata.file_type().is_dir() {
            bail!("skill_file 路径组件不是目录: {}", display_path(candidate));
        }
    }
    final_metadata.ok_or_else(|| anyhow::anyhow!("skill_file 路径为空"))
}

fn inspect_skill_path(path: &Path) -> Result<SkillSnapshot> {
    ensure_ordinary_skill_path(path)?;
    let before_canonical = path
        .canonicalize()
        .with_context(|| format!("无法规范化 skill_file: {}", display_path(path)))?;
    ensure_ordinary_skill_path(&before_canonical)?;
    let (bytes, identity) = read_handle_bound_file(path, "skill_file")?;
    let after_canonical = path
        .canonicalize()
        .with_context(|| format!("无法复查 skill_file: {}", display_path(path)))?;
    if before_canonical != after_canonical {
        bail!("skill_file 在校验期间发生变化: {}", display_path(path));
    }
    Ok(SkillSnapshot {
        path: path.to_path_buf(),
        canonical: before_canonical,
        identity,
        sha256: sha256_bytes(&bytes),
    })
}
fn inspect_skill_file(root: &Path, recorded: &str) -> Result<SkillSnapshot> {
    inspect_skill_path(&resolve_skill_file(root, recorded))
}
fn complete_activation(fields: &BTreeMap<String, String>) -> bool {
    fields.len() == ACTIVATION_FIELDS.len()
        && ACTIVATION_FIELDS
            .iter()
            .all(|field| fields.contains_key(*field))
}

fn running_canonical_skill_sha256() -> String {
    sha256_bytes(CANONICAL_SKILL_BYTES)
}

#[cfg(windows)]
fn safe_windows_normal_component(component: &std::ffi::OsStr) -> bool {
    let Some(component) = component.to_str() else {
        return false;
    };
    if component.ends_with(['.', ' ']) || component.contains(':') {
        return false;
    }
    let stem = component
        .split('.')
        .next()
        .unwrap_or(component)
        .trim_end_matches(' ')
        .to_ascii_uppercase();
    !matches!(
        stem.as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
            | "CONIN$"
            | "CONOUT$"
    )
}

#[cfg(windows)]
fn safe_windows_absolute_text(value: &str) -> bool {
    if value.contains('/') {
        return false;
    }
    let remainder = value
        .strip_prefix("\\\\?\\UNC\\")
        .or_else(|| value.strip_prefix("\\\\?\\"))
        .or_else(|| value.strip_prefix("\\\\"))
        .unwrap_or(value);
    !remainder.starts_with('\\') && !remainder.contains("\\\\")
}

fn safe_recorded_skill_file(value: &str) -> bool {
    if value.is_empty()
        || value.contains(['\r', '\n'])
        || value.starts_with(['\'', '"'])
        || value.ends_with(['\'', '"'])
        || value
            .split(['/', '\\'])
            .any(|segment| matches!(segment, "." | ".."))
    {
        return false;
    }

    let path = Path::new(value);
    #[cfg(windows)]
    if path.components().any(|component| {
        matches!(component, Component::Normal(segment) if !safe_windows_normal_component(segment))
    }) {
        return false;
    }
    if path.is_absolute() {
        #[cfg(windows)]
        {
            if !safe_windows_absolute_text(value) {
                return false;
            }
            return path.components().all(|component| match component {
                Component::Prefix(prefix) => matches!(
                    prefix.kind(),
                    Prefix::Disk(_)
                        | Prefix::VerbatimDisk(_)
                        | Prefix::UNC(_, _)
                        | Prefix::VerbatimUNC(_, _)
                ),
                Component::RootDir | Component::Normal(_) => true,
                _ => false,
            });
        }
        #[cfg(not(windows))]
        {
            return path
                .components()
                .all(|component| matches!(component, Component::RootDir | Component::Normal(_)));
        }
    }
    if value.contains('\\') {
        return false;
    }
    let components = path.components().collect::<Vec<_>>();
    if components.is_empty()
        || components
            .iter()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return false;
    }
    components
        .iter()
        .map(|component| match component {
            Component::Normal(segment) => segment.to_str(),
            _ => None,
        })
        .collect::<Option<Vec<_>>>()
        .is_some_and(|segments| segments.join("/") == value)
}

fn is_valid_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn is_canonical_positive_integer(value: &str) -> bool {
    !value.is_empty()
        && (value == "0" || !value.starts_with('0'))
        && value
            .parse::<u64>()
            .is_ok_and(|parsed| parsed > 0 && parsed.to_string() == value)
}

fn is_valid_cli_contract(value: &str) -> bool {
    value
        .strip_prefix("rayman-cli-contract-v")
        .is_some_and(is_canonical_positive_integer)
}

fn is_canonical_version_component(value: &str) -> bool {
    !value.is_empty()
        && (value == "0" || !value.starts_with('0'))
        && value
            .parse::<u64>()
            .is_ok_and(|parsed| parsed.to_string() == value)
}

fn is_valid_cli_version(value: &str) -> bool {
    let components = value.split('.').collect::<Vec<_>>();
    components.len() == 3
        && components
            .iter()
            .all(|component| is_canonical_version_component(component))
}

fn has_valid_recorded_identity(fields: &BTreeMap<String, String>) -> bool {
    fields
        .get("skill_sha256")
        .is_some_and(|value| is_valid_sha256(value))
        && fields
            .get("cli_contract")
            .is_some_and(|value| is_valid_cli_contract(value))
        && fields
            .get("cli_version")
            .is_some_and(|value| is_valid_cli_version(value))
}

fn identity_drifted(fields: &BTreeMap<String, String>, actual_sha256: &str) -> bool {
    fields
        .get("skill_sha256")
        .is_none_or(|expected| !expected.eq_ignore_ascii_case(actual_sha256))
        || fields
            .get("cli_contract")
            .is_none_or(|value| value != crate::CLI_CONTRACT)
        || fields
            .get("cli_version")
            .is_none_or(|value| value != crate::CLI_VERSION)
}
pub fn activation_status(root: &Path) -> Result<WorkspaceActivationReport> {
    let root = root.canonicalize().context("无法规范化工作区根")?;
    let state = state_paths::managed_state_root(&root, false)?;
    let state_present = state.is_some();
    let config_path =
        state_paths::managed_state_file(&root, Path::new(ACTIVATION_RELATIVE), false)?;
    let config = read_optional_handle_bound_file(&config_path, "workspace_skill.yaml")
        .context("无法从同一 no-follow 文件句柄读取 workspace_skill.yaml")?;
    if config.is_none() {
        return Ok(WorkspaceActivationReport {
            status: if state_present {
                "orphan_state".into()
            } else {
                "inactive".into()
            },
            active: false,
            state_present,
            config_present: false,
            enabled: false,
            skill: None,
            skill_file: None,
            expected_sha256: None,
            cli_contract: None,
            cli_version: None,
            running_cli_contract: crate::CLI_CONTRACT.into(),
            running_cli_version: crate::CLI_VERSION.into(),
            actual_sha256: None,
            issues: if state_present {
                vec!["受管状态存在，但缺少显式 workspace_skill.yaml 激活合同".into()]
            } else {
                Vec::new()
            },
            rebind_eligible: false,
            recovery_command: None,
            retained_evidence: Vec::new(),
        });
    }

    let (config_bytes, _) = config.expect("activation handle presence was checked");
    let text =
        std::str::from_utf8(&config_bytes).context("workspace_skill.yaml 必须是有效 UTF-8")?;
    let fields = parse_activation(text)?;
    let skill = fields.get("skill").cloned();
    let enabled_value = fields.get("enabled").map(String::as_str);
    let enabled = enabled_value == Some("true");
    let skill_file = fields.get("skill_file").cloned();
    let expected_sha256 = fields.get("skill_sha256").cloned();
    let mut issues = Vec::new();
    let cli_contract = fields.get("cli_contract").cloned();
    let cli_version = fields.get("cli_version").cloned();
    if cli_contract.as_deref() != Some(crate::CLI_CONTRACT) {
        issues.push(format!("cli_contract 必须精确为 {}", crate::CLI_CONTRACT));
    }
    if cli_version.as_deref() != Some(crate::CLI_VERSION) {
        issues.push(format!("cli_version 必须精确为 {}", crate::CLI_VERSION));
    }
    if skill.as_deref() != Some(SKILL_NAME) {
        issues.push(format!("skill 必须精确为 {SKILL_NAME}"));
    }
    match enabled_value {
        Some("true") | Some("false") => {}
        Some(_) | None => issues.push("enabled 不是 true".into()),
    }
    let skill_snapshot = if let Some(value) = skill_file.as_deref() {
        if !safe_recorded_skill_file(value) {
            issues.push("skill_file 路径不是 activate 可生成的安全规范形式".into());
            None
        } else {
            match inspect_skill_file(&root, value) {
                Ok(snapshot) => Some(snapshot),
                Err(error) => {
                    issues.push(format!(
                        "skill_file 不可安全读取 {}: {error}",
                        display_path(resolve_skill_file(&root, value).as_ref())
                    ));
                    None
                }
            }
        }
    } else {
        issues.push("缺少 skill_file".into());
        None
    };
    let actual_sha256 = skill_snapshot
        .as_ref()
        .map(|snapshot| snapshot.sha256.clone());
    match (expected_sha256.as_deref(), actual_sha256.as_deref()) {
        (Some(expected), Some(actual))
            if expected.len() == 64
                && expected
                    .chars()
                    .all(|character| character.is_ascii_hexdigit())
                && expected.eq_ignore_ascii_case(actual) => {}
        (Some(_), Some(_)) => issues.push("skill_sha256 与 skill_file 当前内容不一致".into()),
        (None, _) => issues.push("缺少 skill_sha256".into()),
        _ => {}
    }
    let structurally_rebindable = complete_activation(&fields)
        && fields.get("skill").is_some_and(|value| value == SKILL_NAME)
        && fields.get("enabled").is_some_and(|value| value == "true")
        && skill_file.as_deref().is_some_and(safe_recorded_skill_file)
        && has_valid_recorded_identity(&fields)
        && skill_snapshot.is_some();
    let identity_drift = skill_snapshot
        .as_ref()
        .is_some_and(|snapshot| identity_drifted(&fields, &snapshot.sha256));
    let matches_running_canonical = skill_snapshot
        .as_ref()
        .is_some_and(|snapshot| snapshot.sha256 == running_canonical_skill_sha256());
    if structurally_rebindable && identity_drift && !matches_running_canonical {
        issues.push("skill_file 当前内容与此 CLI 内嵌的 canonical SKILL.md 不一致".into());
    }
    let active = enabled && issues.is_empty();
    let status = if active {
        "active".to_string()
    } else if enabled_value == Some("false") && skill.as_deref() == Some(SKILL_NAME) {
        "inactive".to_string()
    } else {
        "invalid".to_string()
    };
    let rebind_eligible = status == "invalid"
        && structurally_rebindable
        && identity_drift
        && matches_running_canonical;
    let recovery_command = rebind_eligible.then(|| REBIND_COMMAND.to_string());
    Ok(WorkspaceActivationReport {
        status,
        active,
        state_present,
        config_present: true,
        enabled,
        skill,
        skill_file,
        expected_sha256,
        actual_sha256,
        cli_contract,
        cli_version,
        running_cli_contract: crate::CLI_CONTRACT.into(),
        running_cli_version: crate::CLI_VERSION.into(),
        issues,
        rebind_eligible,
        recovery_command,
        retained_evidence: Vec::new(),
    })
}

pub fn require_active(root: &Path) -> Result<WorkspaceActivationReport> {
    let report = activation_status(root)?;
    if !report.active {
        if let Some(command) = report.recovery_command.as_deref() {
            bail!(
                "RaymanCodingSkill 工作区激活身份已漂移（status={}）：运行 `{command}`",
                report.status
            );
        }
        bail!(
            "RaymanCodingSkill 工作区未显式激活（status={}）：运行 `rayman workspace activate --skill-file <canonical-SKILL.md> --yes`；历史 .RaymanCodingSkill 状态不会自动激活 skill",
            report.status
        );
    }
    Ok(report)
}

fn prepare_activation(root: &Path, skill_file: &Path) -> Result<PreparedActivation> {
    let lexical = if skill_file.is_absolute() {
        skill_file.to_path_buf()
    } else {
        root.join(skill_file)
    };
    let skill = inspect_skill_path(&lexical)
        .with_context(|| format!("无法读取 canonical skill: {}", display_path(&lexical)))?;
    let recorded_path = skill
        .canonical
        .strip_prefix(root)
        .map(|path| path.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| display_path(&skill.canonical));
    if recorded_path.contains(['\r', '\n']) {
        bail!("skill_file 路径不能包含换行");
    }
    if recorded_path.starts_with(['\'', '"']) || recorded_path.ends_with(['\'', '"']) {
        bail!(
            "skill_file 路径不能以引号开头或结尾（激活合同按未加引号的标量写入）: {recorded_path}"
        );
    }
    if !safe_recorded_skill_file(&recorded_path) {
        bail!("skill_file 路径不是 activate 可生成的安全规范形式");
    }
    let config = format!(
        "skill: {SKILL_NAME}\nenabled: true\nskill_file: {recorded_path}\nskill_sha256: {}\ncli_contract: {}\ncli_version: {}\n",
        skill.sha256,
        crate::CLI_CONTRACT,
        crate::CLI_VERSION
    );
    Ok(PreparedActivation {
        skill,
        recorded_path,
        config,
    })
}

fn verify_prepared_activation(
    root: &Path,
    config_path: &Path,
    prepared: &PreparedActivation,
    report: &WorkspaceActivationReport,
) -> Result<()> {
    if !report.active
        || report.skill.as_deref() != Some(SKILL_NAME)
        || report.skill_file.as_deref() != Some(prepared.recorded_path.as_str())
        || report.expected_sha256.as_deref() != Some(prepared.skill.sha256.as_str())
        || report.cli_contract.as_deref() != Some(crate::CLI_CONTRACT)
        || report.cli_version.as_deref() != Some(crate::CLI_VERSION)
    {
        bail!("写入后的工作区激活合同仍无效: {}", report.issues.join("; "));
    }
    let (bytes, _) = read_stable_activation(config_path)?;
    if bytes != prepared.config.as_bytes() {
        bail!("workspace_skill.yaml changed during activation verification");
    }
    if inspect_skill_path(&prepared.skill.path)? != prepared.skill {
        bail!("skill_file changed during activation verification");
    }
    let resolved_recorded = inspect_skill_file(root, &prepared.recorded_path)?;
    if resolved_recorded.canonical != prepared.skill.canonical
        || resolved_recorded.sha256 != prepared.skill.sha256
    {
        bail!("recorded skill_file changed target during activation verification");
    }
    Ok(())
}

fn rollback_activation_contract(
    transaction: &ActivationTransaction,
    original: Option<&CapturedActivation>,
    publication: &FileSnapshot,
    action: &str,
) -> Result<Option<RetainedActivationEvidence>> {
    let Some(current) = capture_activation_target(transaction, action)? else {
        if let Some(original) = original {
            restore_snapshot_bytes_no_replace(
                transaction,
                &original.snapshot,
                "activation rollback restore after missing publication",
            )?;
        }
        return Ok(None);
    };
    let current_path = current.path.clone();
    if !snapshots_match(publication, &current.snapshot) {
        restore_snapshot_bytes_no_replace(
            transaction,
            &current.snapshot,
            "concurrent activation restoration",
        )?;
        bail!(
            "activation rollback captured a concurrent target; it was restored without overwrite and retained at {}",
            display_path(&current_path)
        );
    }
    if let Some(original) = original {
        restore_snapshot_bytes_no_replace(
            transaction,
            &original.snapshot,
            "activation rollback original restoration",
        )?;
    }
    finalize_captured_activation(
        transaction,
        current,
        "activation rollback publication retention/cleanup",
    )
}

fn replace_activation_contract<V>(
    root: &Path,
    config_path: &Path,
    bytes: &[u8],
    mut verifier: V,
) -> Result<WorkspaceActivationReport>
where
    V: FnMut(&Path, &FileSnapshot) -> Result<WorkspaceActivationReport>,
{
    let baseline = read_optional_activation_snapshot(config_path)?;
    let transaction = ActivationTransaction::open(config_path)?;
    let mut stage = create_owned_activation_temp(
        &transaction,
        bytes,
        baseline.as_ref().map(|snapshot| &snapshot.metadata),
    )?;
    let original = match baseline.as_ref() {
        Some(expected) => {
            match capture_activation_target(&transaction, "activation contract original capture")? {
                Some(original) if snapshots_match(expected, &original.snapshot) => Some(original),
                Some(original) => {
                    let restore = restore_snapshot_bytes_no_replace(
                        &transaction,
                        &original.snapshot,
                        "changed activation restoration",
                    );
                    let cleanup =
                        cleanup_owned_activation_temp(stage, "unused activation staging cleanup");
                    bail!(
                        "activation target changed before capture; current bytes restoration={restore:?}; stage cleanup={cleanup:?}; retained evidence: {}",
                        display_path(&original.path)
                    );
                }
                None => {
                    let restore = restore_snapshot_bytes_no_replace(
                        &transaction,
                        expected,
                        "disappeared activation restoration",
                    );
                    let cleanup =
                        cleanup_owned_activation_temp(stage, "unused activation staging cleanup");
                    bail!(
                        "activation target disappeared before capture; restoration={restore:?}; stage cleanup={cleanup:?}"
                    );
                }
            }
        }
        None => None,
    };

    if let Err(primary) = publish_owned_activation_temp(&mut stage, &transaction) {
        if stage.published {
            let publication = stage.snapshot.clone();
            drop(stage);
            let rollback = rollback_activation_contract(
                &transaction,
                original.as_ref(),
                &publication,
                "partial activation publication rollback capture",
            );
            bail!(
                "activation publication became named but durability/verification failed: {primary:#}; rollback={rollback:?}"
            );
        }
        let restore = original.as_ref().map(|original| {
            restore_snapshot_bytes_no_replace(
                &transaction,
                &original.snapshot,
                "failed activation publication restoration",
            )
        });
        let cleanup = cleanup_owned_activation_temp(stage, "failed activation staging cleanup");
        bail!(
            "activation publication failed without overwrite: {primary:#}; restore={restore:?}; stage cleanup={cleanup:?}"
        );
    }
    let publication = stage.snapshot.clone();
    let verified = verifier(root, &publication).and_then(|report| {
        verify_transaction_target_snapshot(
            &transaction,
            &publication,
            "activation contract post-publication verification",
        )?;
        Ok(report)
    });
    match verified {
        Ok(mut report) => {
            if let Some(original) = original
                && let Some(evidence) = finalize_captured_activation(
                    &transaction,
                    original,
                    "activation contract preserved prior activation",
                )?
            {
                report.retained_evidence.push(evidence);
            }
            Ok(report)
        }
        Err(primary) => {
            drop(stage);
            let rollback = rollback_activation_contract(
                &transaction,
                original.as_ref(),
                &publication,
                "activation contract rollback capture",
            );
            bail!("activation contract verification failed: {primary:#}; rollback={rollback:?}")
        }
    }
}

pub fn activate(root: &Path, skill_file: &Path) -> Result<WorkspaceActivationReport> {
    ensure_supported_activation_write_platform()?;
    let root = root.canonicalize().context("无法规范化工作区根")?;
    let path =
        state_paths::managed_state_file(root.as_path(), Path::new(ACTIVATION_RELATIVE), true)?;
    let _lock = crate::state_lock::acquire_state_lock(&path)?;
    let prepared = prepare_activation(&root, skill_file)?;
    replace_activation_contract(&root, &path, prepared.config.as_bytes(), |root, _| {
        let report = activation_status(root)?;
        verify_prepared_activation(root, &path, &prepared, &report)?;
        Ok(report)
    })
}
struct LoadedRebind {
    config_path: PathBuf,
    original_bytes: Vec<u8>,
    config_identity: FileIdentity,
    config_metadata: ActivationMetadata,
    skill_file: String,
    skill: SkillSnapshot,
    changed: bool,
}

fn read_handle_bound_file(path: &Path, label: &str) -> Result<(Vec<u8>, FileIdentity)> {
    let mut file = open_file_no_follow(path).with_context(|| {
        format!(
            "cannot open {label} without following links: {}",
            display_path(path)
        )
    })?;
    let before = file.metadata().with_context(|| {
        format!(
            "cannot read {label} handle metadata: {}",
            display_path(path)
        )
    })?;
    if is_link_or_reparse(&before) || !before.file_type().is_file() {
        bail!(
            "{label} must be an ordinary non-link file: {}",
            display_path(path)
        );
    }
    let before_identity = file_identity_from_handle(&file, &before, path)?;
    if !has_strong_file_identity(&before_identity) {
        bail!(
            "{label} lacks the strong file identity required for transactions: {}",
            display_path(path)
        );
    }

    let mut first = Vec::new();
    file.read_to_end(&mut first).with_context(|| {
        format!(
            "cannot read {label} from its bound handle: {}",
            display_path(path)
        )
    })?;
    let middle = file.metadata().with_context(|| {
        format!(
            "cannot recheck {label} handle metadata: {}",
            display_path(path)
        )
    })?;
    let middle_identity = file_identity_from_handle(&file, &middle, path)?;
    file.seek(SeekFrom::Start(0))
        .with_context(|| format!("cannot rewind {label} handle: {}", display_path(path)))?;
    let mut second = Vec::new();
    file.read_to_end(&mut second).with_context(|| {
        format!(
            "cannot reread {label} from its bound handle: {}",
            display_path(path)
        )
    })?;
    let after = file.metadata().with_context(|| {
        format!(
            "cannot finalize {label} handle metadata: {}",
            display_path(path)
        )
    })?;
    let after_identity = file_identity_from_handle(&file, &after, path)?;
    if first != second
        || before_identity != middle_identity
        || middle_identity != after_identity
        || after_identity.len != second.len() as u64
    {
        bail!(
            "{label} changed during handle-bound read: {}",
            display_path(path)
        );
    }

    let named_file = open_file_no_follow(path).with_context(|| {
        format!(
            "{label} path changed after handle-bound read: {}",
            display_path(path)
        )
    })?;
    let named_metadata = named_file.metadata().with_context(|| {
        format!(
            "cannot recheck named {label} handle: {}",
            display_path(path)
        )
    })?;
    if is_link_or_reparse(&named_metadata) || !named_metadata.file_type().is_file() {
        bail!(
            "{label} path became a link or non-file: {}",
            display_path(path)
        );
    }
    let named_identity = file_identity_from_handle(&named_file, &named_metadata, path)?;
    if named_identity != after_identity {
        bail!(
            "{label} path changed identity during handle-bound read: {}",
            display_path(path)
        );
    }
    Ok((second, after_identity))
}

fn read_optional_handle_bound_file(
    path: &Path,
    label: &str,
) -> Result<Option<(Vec<u8>, FileIdentity)>> {
    match read_handle_bound_file(path, label) {
        Ok(value) => Ok(Some(value)),
        Err(error)
            if error.chain().any(|source| {
                source
                    .downcast_ref::<io::Error>()
                    .is_some_and(|io_error| io_error.kind() == io::ErrorKind::NotFound)
            }) =>
        {
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

fn read_stable_activation(path: &Path) -> Result<(Vec<u8>, FileIdentity)> {
    read_handle_bound_file(path, "workspace_skill.yaml")
}
fn load_rebind(root: &Path) -> Result<LoadedRebind> {
    let config_path = state_paths::managed_state_file(root, Path::new(ACTIVATION_RELATIVE), false)?;
    let original = read_file_snapshot(&config_path)?;
    let original_bytes = original.bytes;
    let config_identity = original.identity;
    let config_metadata = original.metadata;
    let text =
        std::str::from_utf8(&original_bytes).context("workspace_skill.yaml 必须是有效 UTF-8")?;
    let fields = parse_activation(text)?;
    if !complete_activation(&fields) {
        bail!("workspace rebind 只接受完整六字段激活合同");
    }
    if fields.get("skill").is_none_or(|value| value != SKILL_NAME) {
        bail!("workspace rebind 只接受 skill: {SKILL_NAME}");
    }
    if fields.get("enabled").is_none_or(|value| value != "true") {
        bail!("workspace rebind 只接受 enabled: true");
    }
    if !has_valid_recorded_identity(&fields) {
        bail!("workspace rebind 拒绝格式无效的旧身份字段");
    }
    let skill_file = fields
        .get("skill_file")
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("workspace rebind 缺少 skill_file"))?;
    if !safe_recorded_skill_file(&skill_file) {
        bail!("workspace rebind 拒绝无法原样安全写回的 skill_file");
    }
    let skill = inspect_skill_file(root, &skill_file)?;
    if skill.sha256 != running_canonical_skill_sha256() {
        bail!("skill_file 当前内容与此 CLI 内嵌的 canonical SKILL.md 不一致");
    }
    let changed = identity_drifted(&fields, &skill.sha256);
    Ok(LoadedRebind {
        config_path,
        original_bytes,
        config_identity,
        config_metadata,
        skill_file,
        skill,
        changed,
    })
}

fn ensure_rebind_inputs_unchanged(root: &Path, loaded: &LoadedRebind) -> Result<()> {
    let current = read_file_snapshot(&loaded.config_path)?;
    if current.bytes != loaded.original_bytes
        || current.identity != loaded.config_identity
        || current.metadata != loaded.config_metadata
    {
        bail!("workspace_skill.yaml 在 rebind 发布前发生并发变化");
    }
    let skill = inspect_skill_file(root, &loaded.skill_file)?;
    if skill != loaded.skill {
        bail!("skill_file 在 rebind 发布前发生并发变化");
    }
    Ok(())
}

fn replace_identity_scalar(line: &str, skill_sha256: &str) -> String {
    let Some((key, raw_value)) = line.split_once(':') else {
        return line.to_string();
    };
    let replacement = match key.trim() {
        "skill_sha256" => skill_sha256,
        "cli_contract" => crate::CLI_CONTRACT,
        "cli_version" => crate::CLI_VERSION,
        _ => return line.to_string(),
    };
    let leading_len = raw_value.len() - raw_value.trim_start().len();
    let trailing_start = raw_value.trim_end().len();
    let leading = &raw_value[..leading_len];
    let trailing = &raw_value[trailing_start..];
    let scalar = raw_value.trim();
    let quote = scalar.chars().next().filter(|quote| {
        matches!(quote, '\'' | '"') && scalar.len() >= 2 && scalar.ends_with(*quote)
    });
    let replacement = match quote {
        Some(quote) => format!("{quote}{replacement}{quote}"),
        None => replacement.to_string(),
    };
    format!("{key}:{leading}{replacement}{trailing}")
}

fn rewrite_rebind_contract(original: &str, skill_sha256: &str) -> String {
    let mut rewritten = String::with_capacity(original.len());
    for segment in original.split_inclusive('\n') {
        let (line, ending) = if let Some(line) = segment.strip_suffix("\r\n") {
            (line, "\r\n")
        } else if let Some(line) = segment.strip_suffix('\n') {
            (line, "\n")
        } else {
            (segment, "")
        };
        rewritten.push_str(&replace_identity_scalar(line, skill_sha256));
        rewritten.push_str(ending);
    }
    rewritten
}

fn verify_rebind_publication(
    transaction: &ActivationTransaction,
    root: &Path,
    loaded: &LoadedRebind,
    publication: &FileSnapshot,
    report: &WorkspaceActivationReport,
) -> Result<()> {
    if !report.active
        || report.skill.as_deref() != Some(SKILL_NAME)
        || report.skill_file.as_deref() != Some(loaded.skill_file.as_str())
        || report.expected_sha256.as_deref() != Some(loaded.skill.sha256.as_str())
        || report.cli_contract.as_deref() != Some(crate::CLI_CONTRACT)
        || report.cli_version.as_deref() != Some(crate::CLI_VERSION)
    {
        bail!(
            "写入后的 workspace rebind 合同仍无效: {}",
            report.issues.join("; ")
        );
    }
    verify_transaction_target_snapshot(
        transaction,
        publication,
        "workspace rebind post-publication verification",
    )?;
    if inspect_skill_file(root, &loaded.skill_file)? != loaded.skill {
        bail!("skill_file 在 rebind 写后验证期间发生并发变化");
    }
    Ok(())
}

fn rollback_rebind_publication<H>(
    transaction: &ActivationTransaction,
    loaded: &LoadedRebind,
    original: &CapturedActivation,
    publication: &FileSnapshot,
    transaction_hook: &mut H,
) -> Result<()>
where
    H: FnMut(ActivationTransactionPhase, &Path, &Path) -> Result<()>,
{
    let Some(current) =
        capture_activation_target(transaction, "workspace rebind rollback capture")?
    else {
        return restore_snapshot_bytes_no_replace(
            transaction,
            &original.snapshot,
            "workspace rebind rollback restore",
        );
    };

    let hook_error = transaction_hook(
        ActivationTransactionPhase::RebindAfterRollbackCapture,
        &loaded.config_path,
        &current.path,
    )
    .err();
    let rebound =
        read_transaction_retained_snapshot(transaction, &current.path).with_context(|| {
            retained_snapshots_message(
                "workspace rebind rollback capture could not be reverified",
                &original.path,
                &current.path,
            )
        })?;
    if !snapshots_match(&current.snapshot, &rebound) {
        bail!(
            "{}; hook_error={hook_error:?}",
            retained_snapshots_message(
                "workspace rebind rollback capture changed in the private transaction namespace",
                &original.path,
                &current.path,
            )
        );
    }
    if !snapshots_match(publication, &current.snapshot) {
        let restore = restore_snapshot_bytes_no_replace(
            transaction,
            &current.snapshot,
            "concurrent activation restoration",
        );
        match restore {
            Ok(()) => {
                bail!(
                    "{}",
                    retained_snapshots_message(
                        "workspace rebind rollback captured a different activation and restored its bytes without overwriting",
                        &original.path,
                        &current.path,
                    )
                )
            }
            Err(error) => bail!(
                "{}: {error:#}",
                retained_snapshots_message(
                    "workspace rebind rollback captured a different activation and could not restore it",
                    &original.path,
                    &current.path,
                )
            ),
        }
    }

    let restore = restore_snapshot_bytes_no_replace(
        transaction,
        &original.snapshot,
        "workspace rebind rollback restore",
    );
    let cleanup = finalize_captured_activation(
        transaction,
        current,
        "workspace rebind rollback publication retention/cleanup",
    );
    let outcome = match (restore, cleanup) {
        (Ok(()), Ok(_)) => Ok(()),
        (Err(restore), Ok(_)) => Err(restore),
        (Ok(()), Err(cleanup)) => Err(cleanup),
        (Err(restore), Err(cleanup)) => Err(anyhow::anyhow!(
            "workspace rebind rollback restore and publication cleanup both failed: restore: {restore:#}; cleanup: {cleanup:#}"
        )),
    };
    match (hook_error, outcome) {
        (None, outcome) => outcome,
        (Some(hook), Ok(())) => Err(hook).context(
            "workspace rebind rollback hook failed after the captured activation was recovered",
        ),
        (Some(hook), Err(outcome)) => bail!(
            "workspace rebind rollback hook and recovery both failed: hook: {hook:#}; recovery: {outcome:#}"
        ),
    }
}
fn rebind_with<B, H, V>(
    root: &Path,
    before_publish: &mut B,
    transaction_hook: &mut H,
    verifier: &mut V,
) -> Result<WorkspaceRebindReport>
where
    B: FnMut(&Path, &Path) -> Result<()>,
    H: FnMut(ActivationTransactionPhase, &Path, &Path) -> Result<()>,
    V: FnMut(&Path) -> Result<WorkspaceActivationReport>,
{
    let root = root.canonicalize().context("无法规范化工作区根")?;
    let preflight = activation_status(&root)?;
    if preflight.active {
        return Ok(WorkspaceRebindReport {
            activation: preflight,
            changed: false,
        });
    }
    if !preflight.rebind_eligible {
        bail!(
            "当前 workspace 激活合同不可 rebind: {}",
            preflight.issues.join("; ")
        );
    }

    let loaded = load_rebind(&root)?;
    if !loaded.changed {
        let report = verifier(&root)?;
        if !report.active {
            bail!("workspace rebind 已无需更新，但激活状态仍无效");
        }
        return Ok(WorkspaceRebindReport {
            activation: report,
            changed: false,
        });
    }

    before_publish(&loaded.config_path, &loaded.skill.path)?;
    ensure_rebind_inputs_unchanged(&root, &loaded)?;
    let original_text =
        std::str::from_utf8(&loaded.original_bytes).context("原激活合同不是有效 UTF-8")?;
    let published = rewrite_rebind_contract(original_text, &loaded.skill.sha256);
    let transaction = ActivationTransaction::open(&loaded.config_path)?;
    let mut stage = create_owned_activation_temp(
        &transaction,
        published.as_bytes(),
        Some(&loaded.config_metadata),
    )?;
    let publication_snapshot;

    let original = match capture_activation_target(
        &transaction,
        "workspace rebind original capture",
    ) {
        Ok(Some(original)) => original,
        Ok(None) => {
            let cleanup =
                cleanup_owned_activation_temp(stage, "workspace rebind unused stage cleanup");
            match cleanup {
                Ok(()) => bail!("workspace_skill.yaml disappeared before atomic rebind capture"),
                Err(error) => bail!(
                    "workspace_skill.yaml disappeared before atomic rebind capture; staging cleanup failed: {error:#}"
                ),
            }
        }
        Err(primary) => {
            let cleanup = cleanup_owned_activation_temp(
                stage,
                "workspace rebind capture-error stage cleanup",
            );
            return match cleanup {
                Ok(()) => Err(primary),
                Err(cleanup) => bail!(
                    "workspace rebind capture failed and stage cleanup also failed: capture: {primary:#}; cleanup: {cleanup:#}"
                ),
            };
        }
    };
    let expected_original = FileSnapshot {
        bytes: loaded.original_bytes.clone(),
        identity: loaded.config_identity.clone(),
        metadata: loaded.config_metadata.clone(),
    };
    if !snapshots_match(&expected_original, &original.snapshot) {
        let restore = restore_snapshot_bytes_no_replace(
            &transaction,
            &original.snapshot,
            "pre-publication concurrent activation restoration",
        );
        let cleanup = cleanup_owned_activation_temp(stage, "unused rebind stage cleanup");
        let reason = retained_snapshot_message(
            "workspace_skill.yaml changed before atomic rebind capture",
            &original.path,
        );
        match (restore, cleanup) {
            (Ok(()), Ok(())) => bail!("{reason}"),
            (restore, cleanup) => bail!("{reason}; restore={restore:?}; stage_cleanup={cleanup:?}"),
        }
    }

    let prepublication = (|| -> Result<()> {
        transaction_hook(
            ActivationTransactionPhase::RebindAfterOriginalCapture,
            &loaded.config_path,
            &original.path,
        )?;
        verify_transaction_retained_snapshot(
            &transaction,
            &original.path,
            &original.snapshot,
            "workspace rebind private original capture verification",
        )
        .with_context(|| {
            retained_snapshot_message(
                "workspace rebind original capture changed before publication",
                &original.path,
            )
        })?;
        if inspect_skill_file(&root, &loaded.skill_file)? != loaded.skill {
            bail!("skill_file changed after the activation was captured");
        }
        Ok(())
    })();
    if let Err(primary) = prepublication {
        let restore = restore_snapshot_bytes_no_replace(
            &transaction,
            &original.snapshot,
            "workspace rebind pre-publication restoration",
        );
        let cleanup = cleanup_owned_activation_temp(stage, "unused rebind stage cleanup");
        bail!(
            "{}: primary: {primary:#}; restore={restore:?}; stage_cleanup={cleanup:?}",
            retained_snapshot_message(
                "workspace rebind stopped after capturing the original activation",
                &original.path,
            )
        );
    }
    let publication = match publish_owned_activation_temp(&mut stage, &transaction) {
        Ok(()) => {
            publication_snapshot = stage.snapshot.clone();
            let result = (|| -> Result<WorkspaceActivationReport> {
                transaction_hook(
                    ActivationTransactionPhase::RebindAfterPublicationMove,
                    &loaded.config_path,
                    &original.path,
                )?;
                sync_activation_parent(&transaction)?;
                let report = verifier(&root)?;
                verify_rebind_publication(
                    &transaction,
                    &root,
                    &loaded,
                    &publication_snapshot,
                    &report,
                )?;
                Ok(report)
            })();
            Some(result)
        }
        Err(primary) => {
            if stage.published {
                publication_snapshot = stage.snapshot.clone();
                drop(stage);
                let rollback = rollback_rebind_publication(
                    &transaction,
                    &loaded,
                    &original,
                    &publication_snapshot,
                    transaction_hook,
                );
                bail!(
                    "{}; named publication post-check failed: {primary:#}; rollback={rollback:?}",
                    retained_snapshot_message(
                        "workspace rebind publication became named before failing",
                        &original.path,
                    )
                );
            }
            let cleanup = cleanup_owned_activation_temp(stage, "failed rebind stage cleanup");
            let reason = retained_snapshot_message(
                "workspace rebind publication did not replace the concurrent target",
                &original.path,
            );
            return match cleanup {
                Ok(()) => Err(primary).context(reason),
                Err(cleanup) => {
                    bail!("{reason}; publication: {primary}; stage cleanup: {cleanup:#}")
                }
            };
        }
    };

    match publication.expect("publication move records an outcome") {
        Ok(mut activation) => {
            if let Err(error) = transaction_hook(
                ActivationTransactionPhase::RebindBeforeSuccessCleanup,
                &loaded.config_path,
                &original.path,
            ) {
                return Err(error).context(retained_snapshot_message(
                    "workspace rebind publication succeeded but cleanup hook failed",
                    &original.path,
                ));
            }
            if let Some(evidence) = finalize_captured_activation(
                &transaction,
                original,
                "workspace rebind preserved prior activation",
            )? {
                activation.retained_evidence.push(evidence);
            }
            Ok(WorkspaceRebindReport {
                activation,
                changed: true,
            })
        }
        Err(primary) => {
            drop(stage);
            let rollback = rollback_rebind_publication(
                &transaction,
                &loaded,
                &original,
                &publication_snapshot,
                transaction_hook,
            );
            match rollback {
                Ok(()) => bail!(
                    "{}: {primary:#}",
                    retained_snapshot_message(
                        "workspace rebind failed; original bytes were restored without overwrite",
                        &original.path,
                    )
                ),
                Err(rollback) => bail!(
                    "{}: primary: {primary:#}; rollback: {rollback:#}",
                    retained_snapshot_message(
                        "workspace rebind failed and rollback could not restore the original",
                        &original.path,
                    )
                ),
            }
        }
    }
}
pub fn rebind(root: &Path) -> Result<WorkspaceRebindReport> {
    ensure_supported_activation_write_platform()?;
    let root = root.canonicalize().context("无法规范化工作区根")?;
    let initial = activation_status(&root)?;
    if !initial.config_present {
        bail!(
            "当前 workspace 激活合同不可 rebind: {}",
            initial.issues.join("; ")
        );
    }
    let config_path =
        state_paths::managed_state_file(&root, Path::new(ACTIVATION_RELATIVE), false)?;
    let _lock = crate::state_lock::acquire_state_lock(&config_path)?;
    let mut before_publish = |_: &Path, _: &Path| Ok(());
    let mut transaction_hook = |_: ActivationTransactionPhase, _: &Path, _: &Path| Ok(());
    let mut verifier = |workspace: &Path| activation_status(workspace);
    rebind_with(
        &root,
        &mut before_publish,
        &mut transaction_hook,
        &mut verifier,
    )
}

fn ensure_install_bind_target(
    root: &Path,
    report: &WorkspaceActivationReport,
    prepared: &PreparedActivation,
) -> Result<()> {
    let recorded = report
        .skill_file
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("workspace install-bind requires an existing skill_file"))?;
    if !safe_recorded_skill_file(recorded) {
        bail!("workspace install-bind refuses an unsafe recorded skill_file");
    }
    let bound = inspect_skill_file(root, recorded)?;
    if bound.canonical != prepared.skill.canonical {
        bail!("workspace install-bind refuses a canonical skill_file path change");
    }
    if bound.sha256 != prepared.skill.sha256 || bound.identity != prepared.skill.identity {
        bail!("workspace install-bind detected concurrent skill_file changes");
    }
    Ok(())
}

fn rollback_created_install_binding<H>(
    transaction: &ActivationTransaction,
    config_path: &Path,
    publication: &FileSnapshot,
    transaction_hook: &mut H,
) -> Result<()>
where
    H: FnMut(ActivationTransactionPhase, &Path, &Path) -> Result<()>,
{
    let Some(current) = capture_activation_target(transaction, "install-bind rollback capture")?
    else {
        return Ok(());
    };
    let hook_error = transaction_hook(
        ActivationTransactionPhase::InstallAfterRollbackCapture,
        config_path,
        &current.path,
    )
    .err();
    let rebound =
        read_transaction_retained_snapshot(transaction, &current.path).with_context(|| {
            retained_snapshot_message(
                "install-bind rollback capture could not be reverified",
                &current.path,
            )
        })?;
    if !snapshots_match(&current.snapshot, &rebound) {
        bail!(
            "{}",
            retained_snapshot_message(
                "install-bind rollback capture changed in the private transaction namespace",
                &current.path,
            )
        );
    }
    if !snapshots_match(publication, &current.snapshot) {
        let restore = restore_snapshot_bytes_no_replace(
            transaction,
            &current.snapshot,
            "concurrent install-bind activation restoration",
        );
        match restore {
            Ok(()) => {
                bail!(
                    "{}",
                    retained_snapshot_message(
                        "install-bind rollback captured another activation and restored its bytes without overwrite",
                        &current.path,
                    )
                )
            }
            Err(error) => bail!(
                "{}: {error:#}",
                retained_snapshot_message(
                    "install-bind rollback captured another activation and could not restore it",
                    &current.path,
                )
            ),
        }
    }
    let cleanup = finalize_captured_activation(
        transaction,
        current,
        "failed install-bind publication cleanup",
    )
    .map(|_| ());
    match (hook_error, cleanup) {
        (None, cleanup) => cleanup,
        (Some(hook), Ok(())) => Err(hook)
            .context("install-bind rollback hook failed after its publication was captured safely"),
        (Some(hook), Err(cleanup)) => bail!(
            "install-bind rollback hook and cleanup both failed: hook: {hook:#}; cleanup: {cleanup:#}"
        ),
    }
}
fn publish_missing_install_binding<H, V>(
    root: &Path,
    config_path: &Path,
    prepared: &PreparedActivation,
    transaction_hook: &mut H,
    verifier: &mut V,
) -> Result<WorkspaceActivationReport>
where
    H: FnMut(ActivationTransactionPhase, &Path, &Path) -> Result<()>,
    V: FnMut(&Path) -> Result<WorkspaceActivationReport>,
{
    let transaction = ActivationTransaction::open(config_path)?;
    let mut stage = create_owned_activation_temp(&transaction, prepared.config.as_bytes(), None)?;
    let publication_snapshot;
    let prepublication = (|| -> Result<()> {
        let stage_path = stage.path.as_deref().unwrap_or(config_path);
        transaction_hook(
            ActivationTransactionPhase::InstallBeforeCreatePublication,
            config_path,
            stage_path,
        )?;
        if inspect_skill_path(&prepared.skill.path)? != prepared.skill {
            bail!("skill_file changed before install-bind create publication");
        }
        Ok(())
    })();
    if let Err(primary) = prepublication {
        let cleanup = cleanup_owned_activation_temp(stage, "unused install-bind stage cleanup");
        return match cleanup {
            Ok(()) => Err(primary),
            Err(cleanup) => bail!(
                "install-bind pre-publication check and stage cleanup both failed: primary: {primary:#}; cleanup: {cleanup:#}"
            ),
        };
    }

    if let Err(primary) = publish_owned_activation_temp(&mut stage, &transaction) {
        if stage.published {
            publication_snapshot = stage.snapshot.clone();
            drop(stage);
            let rollback = rollback_created_install_binding(
                &transaction,
                config_path,
                &publication_snapshot,
                transaction_hook,
            );
            bail!(
                "install-bind publication became named but durability/verification failed: {primary:#}; rollback={rollback:?}"
            );
        }
        let cleanup = cleanup_owned_activation_temp(stage, "failed install-bind stage cleanup");
        return match cleanup {
            Ok(()) => Err(primary).with_context(|| {
                format!(
                    "install-bind activation target appeared or no-replace rename is unavailable; refusing overwrite at {}",
                    display_path(config_path)
                )
            }),
            Err(cleanup) => bail!(
                "install-bind publication did not move and stage cleanup failed: publication: {primary}; cleanup: {cleanup:#}"
            ),
        };
    }
    publication_snapshot = stage.snapshot.clone();

    let publication = (|| -> Result<WorkspaceActivationReport> {
        transaction_hook(
            ActivationTransactionPhase::InstallAfterPublicationMove,
            config_path,
            config_path,
        )?;
        sync_activation_parent(&transaction)?;
        let report = verifier(root)?;
        verify_prepared_activation(root, config_path, prepared, &report)?;
        verify_transaction_target_snapshot(
            &transaction,
            &publication_snapshot,
            "install-bind post-publication verification",
        )?;
        Ok(report)
    })();
    match publication {
        Ok(report) => Ok(report),
        Err(primary) => {
            drop(stage);
            let rollback = rollback_created_install_binding(
                &transaction,
                config_path,
                &publication_snapshot,
                transaction_hook,
            );
            match rollback {
                Ok(()) => bail!(
                    "workspace install-bind failed; its own publication was captured and removed without deleting a concurrent target: {primary:#}"
                ),
                Err(rollback) => bail!(
                    "workspace install-bind failed and rollback retained recovery evidence: primary: {primary:#}; rollback: {rollback:#}"
                ),
            }
        }
    }
}
pub fn install_bind(root: &Path, skill_file: &Path) -> Result<WorkspaceRebindReport> {
    ensure_supported_activation_write_platform()?;
    let root = root.canonicalize().context("无法规范化工作区根")?;
    let config_path = state_paths::managed_state_file(&root, Path::new(ACTIVATION_RELATIVE), true)?;
    let _lock = crate::state_lock::acquire_state_lock(&config_path)?;
    let prepared = prepare_activation(&root, skill_file)?;
    if prepared.skill.sha256 != running_canonical_skill_sha256() {
        bail!("workspace install-bind requires the canonical SKILL.md embedded in this CLI");
    }

    let current = activation_status(&root)?;
    if !current.config_present {
        let mut transaction_hook = |_: ActivationTransactionPhase, _: &Path, _: &Path| Ok(());
        let mut verifier = |workspace: &Path| activation_status(workspace);
        let activation = publish_missing_install_binding(
            &root,
            &config_path,
            &prepared,
            &mut transaction_hook,
            &mut verifier,
        )?;
        return Ok(WorkspaceRebindReport {
            activation,
            changed: true,
        });
    }

    ensure_install_bind_target(&root, &current, &prepared)?;
    if current.active {
        let activation = activation_status(&root)?;
        if !activation.active {
            bail!("workspace install-bind activation changed during the locked no-op check");
        }
        ensure_install_bind_target(&root, &activation, &prepared)?;
        return Ok(WorkspaceRebindReport {
            activation,
            changed: false,
        });
    }
    if !current.rebind_eligible {
        bail!(
            "workspace install-bind refuses the existing activation state: {}",
            current.issues.join("; ")
        );
    }

    let requested = prepared.clone();
    let mut before_publish = move |_: &Path, bound_skill: &Path| {
        let current_skill = inspect_skill_path(bound_skill)?;
        if current_skill.canonical != requested.skill.canonical {
            bail!("workspace install-bind refuses a canonical skill_file path change");
        }
        if current_skill.sha256 != requested.skill.sha256
            || current_skill.identity != requested.skill.identity
        {
            bail!("workspace install-bind detected concurrent skill_file changes");
        }
        Ok(())
    };
    let mut transaction_hook = |_: ActivationTransactionPhase, _: &Path, _: &Path| Ok(());
    let mut verifier = |workspace: &Path| activation_status(workspace);
    let report = rebind_with(
        &root,
        &mut before_publish,
        &mut transaction_hook,
        &mut verifier,
    )?;
    ensure_install_bind_target(&root, &report.activation, &prepared)?;
    Ok(report)
}
pub fn deactivate(root: &Path) -> Result<WorkspaceActivationReport> {
    ensure_supported_activation_write_platform()?;
    let path = state_paths::managed_state_file(root, Path::new(ACTIVATION_RELATIVE), true)?;
    let config = format!("skill: {SKILL_NAME}\nenabled: false\n");
    let _lock = crate::state_lock::acquire_state_lock(&path)?;
    replace_activation_contract(root, &path, config.as_bytes(), |root, publication| {
        let report = activation_status(root)?;
        if report.enabled || report.active || !report.config_present {
            bail!("workspace deactivation verification did not observe enabled: false");
        }
        if publication.bytes != config.as_bytes() {
            bail!("workspace deactivation publication bytes changed");
        }
        Ok(report)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_running_canonical_skill(path: &Path) {
        fs::write(path, CANONICAL_SKILL_BYTES).unwrap();
    }

    fn no_transaction_hook(_: ActivationTransactionPhase, _: &Path, _: &Path) -> Result<()> {
        Ok(())
    }

    #[cfg(unix)]
    fn set_distinct_activation_metadata(path: &Path) -> ActivationMetadata {
        use std::os::unix::fs::PermissionsExt as _;

        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
        let (metadata, _) = read_bound_activation_metadata(path).unwrap();
        assert_eq!(metadata.mode, 0o600);
        metadata
    }

    #[cfg(windows)]
    fn append_access_allowed_ace(acl: &mut Vec<u8>, mask: u32, sid: &[u8]) {
        const ACCESS_ALLOWED_ACE_TYPE: u8 = 0;
        let size = u16::try_from(8 + sid.len()).unwrap();
        acl.push(ACCESS_ALLOWED_ACE_TYPE);
        // Deliberately non-inherited: the regression must distinguish an
        // explicit authorization grant from ordinary directory inheritance.
        acl.push(0);
        acl.extend_from_slice(&size.to_le_bytes());
        acl.extend_from_slice(&mask.to_le_bytes());
        acl.extend_from_slice(sid);
    }

    #[cfg(windows)]
    fn protected_test_dacl(owner: &[u8]) -> Vec<u8> {
        const ACL_REVISION: u8 = 2;
        const FILE_ALL_ACCESS: u32 = 0x001f_01ff;
        const FILE_GENERIC_READ: u32 = 0x0012_0089;
        // S-1-1-0 (Everyone), encoded as a self-contained SID.
        const WORLD_SID: [u8; 12] = [1, 1, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0];

        let mut acl = vec![ACL_REVISION, 0, 0, 0, 2, 0, 0, 0];
        append_access_allowed_ace(&mut acl, FILE_ALL_ACCESS, owner);
        append_access_allowed_ace(&mut acl, FILE_GENERIC_READ, &WORLD_SID);
        let size = u16::try_from(acl.len()).unwrap();
        acl[2..4].copy_from_slice(&size.to_le_bytes());
        acl
    }

    #[cfg(windows)]
    fn assert_protected_distinct_dacl(metadata: &ActivationMetadata) {
        const SE_DACL_PROTECTED: u16 = 0x1000;
        const INHERITED_ACE: u8 = 0x10;
        const ACCESS_ALLOWED_ACE_TYPE: u8 = 0;
        const FILE_GENERIC_READ: u32 = 0x0012_0089;
        const WORLD_SID: [u8; 12] = [1, 1, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0];

        assert_ne!(metadata.security.control & SE_DACL_PROTECTED, 0);
        let WindowsDacl::Acl(acl) = &metadata.security.dacl else {
            panic!("test DACL was not captured as an ACL")
        };
        assert!(acl.len() >= 8, "truncated ACL: {acl:?}");
        let ace_count = u16::from_le_bytes([acl[4], acl[5]]) as usize;
        let mut offset = 8;
        let mut found = false;
        for _ in 0..ace_count {
            assert!(offset + 8 <= acl.len(), "truncated ACE: {acl:?}");
            let ace_type = acl[offset];
            let ace_flags = acl[offset + 1];
            let ace_size = u16::from_le_bytes([acl[offset + 2], acl[offset + 3]]) as usize;
            assert!(ace_size >= 8 && offset + ace_size <= acl.len());
            let mask = u32::from_le_bytes([
                acl[offset + 4],
                acl[offset + 5],
                acl[offset + 6],
                acl[offset + 7],
            ]);
            let sid = &acl[offset + 8..offset + ace_size];
            found |= ace_type == ACCESS_ALLOWED_ACE_TYPE
                && ace_flags & INHERITED_ACE == 0
                && mask == FILE_GENERIC_READ
                && sid == WORLD_SID;
            offset += ace_size;
        }
        assert!(
            found,
            "distinct explicit Everyone-read ACE is missing: {acl:?}"
        );
    }

    #[cfg(windows)]
    fn set_distinct_activation_metadata(path: &Path) -> ActivationMetadata {
        use std::ffi::c_void;
        use std::os::windows::fs::OpenOptionsExt as _;
        use std::os::windows::io::AsRawHandle as _;
        use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_HIDDEN;

        #[link(name = "advapi32")]
        unsafe extern "system" {
            fn SetSecurityInfo(
                handle: *mut c_void,
                object_type: u32,
                security_information: u32,
                owner: *mut c_void,
                group: *mut c_void,
                dacl: *mut c_void,
                sacl: *mut c_void,
            ) -> u32;
        }

        const SE_FILE_OBJECT: u32 = 1;
        const DACL_SECURITY_INFORMATION: u32 = 0x0000_0004;
        const PROTECTED_DACL_SECURITY_INFORMATION: u32 = 0x8000_0000;
        const READ_CONTROL: u32 = 0x0002_0000;
        const WRITE_DAC: u32 = 0x0004_0000;

        let (baseline, _) = read_bound_activation_metadata(path).unwrap();
        let owner = baseline.security.owner.as_deref().unwrap();
        let mut dacl = protected_test_dacl(owner);
        let mut options = fs::OpenOptions::new();
        options.access_mode(READ_CONTROL | WRITE_DAC);
        let security_file = options.open(path).unwrap();
        let status = unsafe {
            SetSecurityInfo(
                security_file.as_raw_handle().cast(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                dacl.as_mut_ptr().cast(),
                std::ptr::null_mut(),
            )
        };
        assert_eq!(status, 0, "{}", io::Error::from_raw_os_error(status as i32));
        drop(security_file);

        let (mut expected, _) = read_bound_activation_metadata(path).unwrap();
        assert_protected_distinct_dacl(&expected);
        expected.attributes |= FILE_ATTRIBUTE_HIDDEN;
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .unwrap();
        apply_activation_metadata(&file, path, &expected).unwrap();
        drop(file);
        let (actual, _) = read_bound_activation_metadata(path).unwrap();
        assert_eq!(actual, expected);
        assert_protected_distinct_dacl(&actual);
        actual
    }

    #[cfg(target_os = "linux")]
    fn read_named_acl(path: &Path) -> Vec<u8> {
        let output = std::process::Command::new("getfacl")
            .arg("-cpn")
            .arg(path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "getfacl failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        output.stdout
    }

    #[cfg(target_os = "linux")]
    fn install_named_acl_if_supported(path: &Path) -> Option<Vec<u8>> {
        for program in ["setfacl", "getfacl"] {
            match std::process::Command::new(program)
                .arg("--version")
                .output()
            {
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    eprintln!("skipping named POSIX ACL regression: {program} is not installed");
                    return None;
                }
                Err(error) => panic!("cannot probe {program}: {error}"),
                Ok(output) => assert!(
                    output.status.success(),
                    "{program} capability probe failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                ),
            }
        }

        let effective_uid = unsafe { libc::geteuid() };
        let named_uid = if effective_uid == 65_534 {
            65_533
        } else {
            65_534
        };
        let output = std::process::Command::new("setfacl")
            .arg("-m")
            .arg(format!("u:{named_uid}:r--"))
            .arg(path)
            .output()
            .unwrap();
        if !output.status.success() {
            let error = String::from_utf8_lossy(&output.stderr);
            let normalized = error.to_ascii_lowercase();
            if normalized.contains("operation not supported")
                || normalized.contains("not supported")
                || normalized.contains("function not implemented")
            {
                eprintln!(
                    "skipping named POSIX ACL regression: fixture filesystem has no ACL capability: {error}"
                );
                return None;
            }
            panic!("setfacl failed on an owned fixture: {error}");
        }
        let acl = read_named_acl(path);
        assert!(
            String::from_utf8_lossy(&acl).contains(&format!("user:{named_uid}:r--")),
            "named ACL entry was not installed: {}",
            String::from_utf8_lossy(&acl)
        );
        Some(acl)
    }

    #[test]
    fn orphan_state_is_not_activation() {
        let root = tempfile::tempdir().unwrap();
        state_paths::managed_state_root(root.path(), true).unwrap();
        let report = activation_status(root.path()).unwrap();
        assert_eq!(report.status, "orphan_state");
        assert!(!report.active);
        assert!(require_active(root.path()).is_err());
    }

    #[test]
    fn activation_is_hash_bound_and_deactivation_is_explicit() {
        let root = tempfile::tempdir().unwrap();
        let skill = root.path().join("SKILL.md");
        fs::write(&skill, "canonical skill\n").unwrap();
        assert!(activate(root.path(), &skill).unwrap().active);

        fs::write(&skill, "changed skill\n").unwrap();
        let drifted = activation_status(root.path()).unwrap();
        assert_eq!(drifted.status, "invalid");
        assert!(!drifted.active);

        let inactive = deactivate(root.path()).unwrap();
        assert_eq!(inactive.status, "inactive");
        assert!(!inactive.active);
    }

    #[test]
    fn activation_enabled_value_is_exact_and_false_is_inactive() {
        let root = tempfile::tempdir().unwrap();
        let skill = root.path().join("SKILL.md");
        fs::write(&skill, "canonical skill\n").unwrap();
        assert!(activate(root.path(), &skill).unwrap().active);

        let activation = root.path().join(".RaymanCodingSkill/workspace_skill.yaml");
        let canonical = fs::read_to_string(&activation).unwrap();
        for malformed in ["maybe", "True", "FALSE"] {
            fs::write(
                &activation,
                canonical.replace("enabled: true", &format!("enabled: {malformed}")),
            )
            .unwrap();
            let report = activation_status(root.path()).unwrap();
            assert_eq!(report.status, "invalid", "accepted enabled: {malformed}");
            assert!(!report.active);
            assert!(!report.enabled);
            assert!(
                report.issues.iter().any(|issue| issue.contains("enabled")),
                "{:?}",
                report.issues
            );
        }

        fs::write(&activation, canonical.replace("enabled: true\n", "")).unwrap();
        let missing = activation_status(root.path()).unwrap();
        assert_eq!(missing.status, "invalid");
        assert!(missing.issues.iter().any(|issue| issue.contains("enabled")));

        let inactive_text = canonical.replace("enabled: true", "enabled: false");
        fs::write(&activation, &inactive_text).unwrap();
        let inactive = activation_status(root.path()).unwrap();
        assert_eq!(inactive.status, "inactive");
        assert!(!inactive.active);
        assert!(!inactive.enabled);

        for malformed in [
            inactive_text.replace("skill: raymancodingskill", "skill: another-skill"),
            inactive_text.replace("skill: raymancodingskill\n", ""),
        ] {
            fs::write(&activation, malformed).unwrap();
            let report = activation_status(root.path()).unwrap();
            assert_eq!(
                report.status, "invalid",
                "enabled: false without the managed skill must fail closed"
            );
            assert!(!report.active);
        }
    }

    #[test]
    fn activation_contract_rejects_duplicate_unknown_and_malformed_fields() {
        for invalid in [
            "skill: raymancodingskill\nskill: raymancodingskill\nenabled: true\n",
            "skill: raymancodingskill\nenabled: true\nauto_use: true\n",
            "skill: raymancodingskill\n  enabled: true\n",
            "skill: \"raymancodingskill\nenabled: true\n",
            "skill raymancodingskill\nenabled: true\n",
        ] {
            assert!(parse_activation(invalid).is_err(), "accepted: {invalid:?}");
        }
    }

    #[test]
    fn activation_is_bound_to_the_running_cli_contract_and_version() {
        let root = tempfile::tempdir().unwrap();
        let skill = root.path().join("SKILL.md");
        fs::write(&skill, "canonical skill\n").unwrap();
        let hash = sha256_file(&skill).unwrap();
        let state = state_paths::managed_state_root(root.path(), true)
            .unwrap()
            .unwrap();
        fs::write(
            state.join(ACTIVATION_RELATIVE),
            format!(
                "skill: {SKILL_NAME}\nenabled: true\nskill_file: SKILL.md\nskill_sha256: {hash}\ncli_contract: rayman-cli-contract-v5\ncli_version: 2.1.0\n"
            ),
        )
        .unwrap();

        let report = activation_status(root.path()).unwrap();
        assert!(!report.active);
        assert_eq!(
            report.cli_contract.as_deref(),
            Some("rayman-cli-contract-v5")
        );
        assert_eq!(report.running_cli_contract, crate::CLI_CONTRACT);
        assert!(
            report
                .issues
                .iter()
                .any(|issue| issue.contains("cli_contract"))
        );
        assert!(
            report
                .issues
                .iter()
                .any(|issue| issue.contains("cli_version"))
        );
    }
    fn write_test_binding(
        root: &Path,
        recorded_skill_file: &str,
        skill_sha256: &str,
        cli_contract: &str,
        cli_version: &str,
    ) -> PathBuf {
        let state = state_paths::managed_state_root(root, true)
            .unwrap()
            .unwrap();
        let path = state.join(ACTIVATION_RELATIVE);
        fs::write(
            &path,
            format!(
                "skill: {SKILL_NAME}\nenabled: true\nskill_file: {recorded_skill_file}\nskill_sha256: {skill_sha256}\ncli_contract: {cli_contract}\ncli_version: {cli_version}\n"
            ),
        )
        .unwrap();
        path
    }

    #[test]
    fn rebind_repairs_only_identity_drift_and_is_idempotent() {
        for drift in ["all", "hash", "contract", "version"] {
            let root = tempfile::tempdir().unwrap();
            let skill = root.path().join("SKILL.md");
            write_running_canonical_skill(&skill);
            let current_hash = sha256_file(&skill).unwrap();
            let old_hash = "0".repeat(64);
            let binding = write_test_binding(
                root.path(),
                "SKILL.md",
                if matches!(drift, "all" | "hash") {
                    &old_hash
                } else {
                    &current_hash
                },
                if matches!(drift, "all" | "contract") {
                    "rayman-cli-contract-v6"
                } else {
                    crate::CLI_CONTRACT
                },
                if matches!(drift, "all" | "version") {
                    "2.2.0"
                } else {
                    crate::CLI_VERSION
                },
            );
            let sentinel = root.path().join(".RaymanCodingSkill").join("sentinel.txt");
            fs::write(&sentinel, "preserve").unwrap();

            let before = activation_status(root.path()).unwrap();
            assert!(before.rebind_eligible, "{drift}: {:?}", before.issues);
            assert_eq!(before.recovery_command.as_deref(), Some(REBIND_COMMAND));
            assert!(
                require_active(root.path())
                    .unwrap_err()
                    .to_string()
                    .contains(REBIND_COMMAND)
            );

            let rebound = rebind(root.path()).unwrap();
            assert!(rebound.changed);
            assert!(rebound.activation.active);
            assert_eq!(fs::read_to_string(&sentinel).unwrap(), "preserve");
            let rebound_text = fs::read_to_string(&binding).unwrap();
            assert!(rebound_text.contains("skill_file: SKILL.md\n"));
            assert!(rebound_text.contains(&format!("skill_sha256: {current_hash}\n")));

            let rebound_bytes = fs::read(&binding).unwrap();
            let idempotent = rebind(root.path()).unwrap();
            assert!(!idempotent.changed);
            assert!(idempotent.activation.active);
            assert_eq!(fs::read(&binding).unwrap(), rebound_bytes);
            let transaction_files = fs::read_dir(root.path().join(".RaymanCodingSkill"))
                .unwrap()
                .filter_map(|entry| entry.ok())
                .filter(|entry| {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    name.starts_with(".workspace_skill.yaml.rayman-") && name.ends_with(".tmp")
                })
                .collect::<Vec<_>>();
            assert!(transaction_files.is_empty(), "{transaction_files:?}");
        }
    }

    #[test]
    fn rebind_preserves_an_external_recorded_skill_path_scalar() {
        let root = tempfile::tempdir().unwrap();
        let external = tempfile::tempdir().unwrap();
        let skill = external.path().join("SKILL.md");
        write_running_canonical_skill(&skill);
        let recorded = skill.canonicalize().unwrap().to_string_lossy().into_owned();
        #[cfg(windows)]
        assert!(recorded.starts_with(r"\\?\"));
        let binding = write_test_binding(
            root.path(),
            &recorded,
            &"0".repeat(64),
            "rayman-cli-contract-v6",
            "2.2.0",
        );

        assert!(rebind(root.path()).unwrap().changed);
        assert!(
            fs::read_to_string(binding)
                .unwrap()
                .contains(&format!("skill_file: {recorded}\n"))
        );
    }

    #[test]
    fn rebind_changes_only_identity_scalars_and_preserves_contract_formatting() {
        let root = tempfile::tempdir().unwrap();
        let skill = root.path().join("SKILL.md");
        write_running_canonical_skill(&skill);
        let current_hash = sha256_file(&skill).unwrap();
        let old_hash = "0".repeat(64);
        let binding =
            state_paths::managed_state_file(root.path(), Path::new(ACTIVATION_RELATIVE), true)
                .unwrap();
        let original = format!(
            "# preserve this comment\r\ncli_version : '2.2.0'\r\nskill_file: \"SKILL.md\"\r\nskill: raymancodingskill\r\nenabled: true\r\nskill_sha256 :  {old_hash}  \r\ncli_contract: \"rayman-cli-contract-v6\"\r\n# preserve final comment"
        );
        fs::write(&binding, &original).unwrap();

        let rebound = rebind(root.path()).unwrap();
        assert!(rebound.changed);
        assert!(rebound.activation.active);
        let expected = format!(
            "# preserve this comment\r\ncli_version : '{}'\r\nskill_file: \"SKILL.md\"\r\nskill: raymancodingskill\r\nenabled: true\r\nskill_sha256 :  {current_hash}  \r\ncli_contract: \"{}\"\r\n# preserve final comment",
            crate::CLI_VERSION,
            crate::CLI_CONTRACT,
        );
        assert_eq!(fs::read_to_string(&binding).unwrap(), expected);
    }

    #[test]
    fn rebind_refuses_untrusted_same_path_skill_content() {
        let root = tempfile::tempdir().unwrap();
        let skill = root.path().join("SKILL.md");
        fs::write(&skill, "not the skill embedded in this CLI\n").unwrap();
        let binding = write_test_binding(
            root.path(),
            "SKILL.md",
            &"0".repeat(64),
            "rayman-cli-contract-v6",
            "2.2.0",
        );
        let original = fs::read(&binding).unwrap();

        let status = activation_status(root.path()).unwrap();
        assert!(!status.rebind_eligible);
        assert!(status.recovery_command.is_none());
        assert!(status.issues.iter().any(|issue| issue.contains("内嵌")));
        assert!(rebind(root.path()).is_err());
        assert_eq!(fs::read(&binding).unwrap(), original);
    }

    #[test]
    fn rebind_rejects_malformed_recorded_identity_without_writing() {
        let cases = [
            ("not-a-sha256", "rayman-cli-contract-v6", "2.2.0"),
            (
                "0000000000000000000000000000000000000000000000000000000000000000",
                "rayman-cli-contract-v0",
                "2.2.0",
            ),
            (
                "0000000000000000000000000000000000000000000000000000000000000000",
                "rayman-cli-contract-v06",
                "2.2.0",
            ),
            (
                "0000000000000000000000000000000000000000000000000000000000000000",
                "garbage",
                "2.2.0",
            ),
            (
                "0000000000000000000000000000000000000000000000000000000000000000",
                "rayman-cli-contract-v6",
                "2.2",
            ),
            (
                "0000000000000000000000000000000000000000000000000000000000000000",
                "rayman-cli-contract-v6",
                "02.2.0",
            ),
        ];
        for (hash, contract, version) in cases {
            let root = tempfile::tempdir().unwrap();
            fs::write(root.path().join("SKILL.md"), "canonical skill\n").unwrap();
            let binding = write_test_binding(root.path(), "SKILL.md", hash, contract, version);
            let original = fs::read(&binding).unwrap();

            let status = activation_status(root.path()).unwrap();
            assert!(!status.rebind_eligible, "{contract} {version}");
            assert!(status.recovery_command.is_none());
            assert!(rebind(root.path()).is_err());
            assert_eq!(fs::read(&binding).unwrap(), original);
        }
    }

    #[test]
    fn rebind_rejects_structural_contract_drift_without_writing() {
        let root = tempfile::tempdir().unwrap();
        let skill = root.path().join("SKILL.md");
        fs::write(&skill, "canonical skill\n").unwrap();
        let hash = sha256_file(&skill).unwrap();
        let state = state_paths::managed_state_root(root.path(), true)
            .unwrap()
            .unwrap();
        let binding = state.join(ACTIVATION_RELATIVE);
        for invalid in [
            format!(
                "skill: other\nenabled: true\nskill_file: SKILL.md\nskill_sha256: {hash}\ncli_contract: rayman-cli-contract-v6\ncli_version: 2.2.0\n"
            ),
            format!(
                "skill: {SKILL_NAME}\nenabled: false\nskill_file: SKILL.md\nskill_sha256: {hash}\ncli_contract: rayman-cli-contract-v6\ncli_version: 2.2.0\n"
            ),
            format!(
                "skill: {SKILL_NAME}\nenabled: true\nskill_file: SKILL.md\nskill_sha256: {hash}\ncli_contract: rayman-cli-contract-v6\n"
            ),
            format!(
                "skill: {SKILL_NAME}\nenabled: true\nskill_file: missing.md\nskill_sha256: {hash}\ncli_contract: rayman-cli-contract-v6\ncli_version: 2.2.0\n"
            ),
        ] {
            fs::write(&binding, &invalid).unwrap();
            assert!(rebind(root.path()).is_err());
            assert_eq!(fs::read_to_string(&binding).unwrap(), invalid);
        }
    }

    #[test]
    fn rebind_and_deactivate_serialize_on_the_same_activation_lock() {
        let root = tempfile::tempdir().unwrap();
        let skill = root.path().join("SKILL.md");
        write_running_canonical_skill(&skill);
        let binding = write_test_binding(
            root.path(),
            "SKILL.md",
            &"0".repeat(64),
            "rayman-cli-contract-v6",
            "2.2.0",
        );
        let held = crate::state_lock::acquire_state_lock(&binding).unwrap();
        let (rebind_tx, rebind_rx) = std::sync::mpsc::channel();
        let rebind_root = root.path().to_path_buf();
        let rebind_worker = std::thread::spawn(move || {
            let _ = rebind_tx.send(rebind(&rebind_root));
        });
        let (deactivate_tx, deactivate_rx) = std::sync::mpsc::channel();
        let deactivate_root = root.path().to_path_buf();
        let deactivate_worker = std::thread::spawn(move || {
            let _ = deactivate_tx.send(deactivate(&deactivate_root));
        });

        let blocked_for = std::time::Duration::from_millis(150);
        assert!(matches!(
            rebind_rx.recv_timeout(blocked_for),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout)
        ));
        assert!(matches!(
            deactivate_rx.recv_timeout(blocked_for),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout)
        ));
        drop(held);

        let completed_within = std::time::Duration::from_secs(5);
        let rebind_result = rebind_rx.recv_timeout(completed_within).unwrap();
        let deactivate_report = deactivate_rx
            .recv_timeout(completed_within)
            .unwrap()
            .unwrap();
        rebind_worker.join().unwrap();
        deactivate_worker.join().unwrap();
        assert!(!deactivate_report.active);
        if let Err(error) = rebind_result {
            assert!(error.to_string().contains("不可 rebind"));
        }
        let final_status = activation_status(root.path()).unwrap();
        assert_eq!(final_status.status, "inactive");
        assert!(!final_status.active);
    }

    fn stale_rebind_fixture() -> (tempfile::TempDir, PathBuf, Vec<u8>) {
        let root = tempfile::tempdir().unwrap();
        write_running_canonical_skill(&root.path().join("SKILL.md"));
        let binding = write_test_binding(
            root.path(),
            "SKILL.md",
            &"0".repeat(64),
            "rayman-cli-contract-v6",
            "2.2.0",
        );
        let original = fs::read(&binding).unwrap();
        (root, binding, original)
    }

    #[test]
    fn rebind_preserves_platform_security_metadata_on_publication() {
        let (root, binding, _) = stale_rebind_fixture();
        let expected = set_distinct_activation_metadata(&binding);

        let report = rebind(root.path()).unwrap();

        assert!(report.changed);
        assert!(report.activation.active);
        let (actual, _) = read_bound_activation_metadata(&binding).unwrap();
        assert_eq!(actual, expected);
        #[cfg(windows)]
        assert_protected_distinct_dacl(&actual);
    }

    #[test]
    fn activation_metadata_probe_preserves_platform_metadata_and_target() {
        let (root, binding, original) = stale_rebind_fixture();
        let expected = set_distinct_activation_metadata(&binding);
        let before = read_file_snapshot(&binding).unwrap();

        let probe = activation_metadata_capability_probe(root.path());

        assert!(probe.applicable, "{probe:?}");
        assert!(probe.probed, "{probe:?}");
        assert!(probe.ready, "{probe:?}");
        assert_eq!(probe.phase, ActivationMetadataProbePhase::Complete);
        assert_eq!(probe.activation_unchanged, Some(true));
        assert_eq!(probe.cleanup_complete, Some(true));
        assert_eq!(fs::read(&binding).unwrap(), original);
        let after = read_file_snapshot(&binding).unwrap();
        assert!(snapshots_match(&before, &after));
        assert_eq!(after.metadata, expected);
        #[cfg(windows)]
        assert_protected_distinct_dacl(&after.metadata);
        assert!(
            fs::read_dir(binding.parent().unwrap())
                .unwrap()
                .filter_map(std::result::Result::ok)
                .all(|entry| {
                    !entry
                        .file_name()
                        .to_string_lossy()
                        .starts_with(".workspace_skill.yaml.rayman-")
                }),
            "activation metadata probe left a named sidecar"
        );
    }

    #[cfg(windows)]
    #[test]
    fn activation_metadata_probe_classifies_invalid_owner_without_touching_target() {
        let (root, binding, original) = stale_rebind_fixture();
        let probe = activation_metadata_capability_probe_with(
            root.path(),
            |_, _, phase, cleanup_complete| {
                *phase = ActivationMetadataProbePhase::StageCreate;
                *cleanup_complete = Some(true);
                Err(io::Error::from_raw_os_error(1307).into())
            },
        );

        assert!(!probe.ready, "{probe:?}");
        assert_eq!(
            probe.failure_class,
            Some(ActivationMetadataFailureClass::OwnerAssignmentDenied)
        );
        assert_eq!(probe.os_error_code, Some(1307));
        assert_eq!(probe.activation_unchanged, Some(true));
        assert_eq!(probe.cleanup_complete, Some(true));
        assert_eq!(fs::read(binding).unwrap(), original);
    }

    #[test]
    fn activation_metadata_probe_treats_cleanup_failure_as_not_ready() {
        let (root, binding, original) = stale_rebind_fixture();
        let probe = activation_metadata_capability_probe_with(
            root.path(),
            |_, _, phase, cleanup_complete| {
                *phase = ActivationMetadataProbePhase::Cleanup;
                *cleanup_complete = Some(false);
                bail!("injected activation metadata probe cleanup failure")
            },
        );

        assert!(!probe.ready, "{probe:?}");
        assert_eq!(
            probe.failure_class,
            Some(ActivationMetadataFailureClass::CleanupFailed)
        );
        assert_eq!(probe.activation_unchanged, Some(true));
        assert_eq!(probe.cleanup_complete, Some(false));
        assert_eq!(fs::read(binding).unwrap(), original);
    }

    #[test]
    fn activation_metadata_probe_classifies_capture_races_and_lock_timeouts() {
        let capture_race = anyhow::anyhow!(
            "activation bytes, identity, or authorization metadata changed during handle-bound capture"
        );
        assert_eq!(
            classify_activation_probe_failure(ActivationMetadataProbePhase::Capture, &capture_race),
            ActivationMetadataFailureClass::ConcurrentChange
        );

        let lock_timeout = anyhow::Error::new(io::Error::new(
            io::ErrorKind::WouldBlock,
            "injected state-lock contention",
        ))
        .context("state-lock timeout");
        assert_eq!(
            classify_activation_probe_failure(ActivationMetadataProbePhase::Lock, &lock_timeout),
            ActivationMetadataFailureClass::Contention
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn rebind_preserves_a_real_named_posix_acl_on_publication_and_rollback() {
        let (publication_root, publication_binding, _) = stale_rebind_fixture();
        let Some(publication_acl) = install_named_acl_if_supported(&publication_binding) else {
            return;
        };
        let publication_metadata = read_bound_activation_metadata(&publication_binding)
            .unwrap()
            .0;
        assert!(
            publication_metadata
                .extended_attributes
                .iter()
                .any(|attribute| { attribute.name.as_slice() == b"system.posix_acl_access" })
        );

        let report = rebind(publication_root.path()).unwrap();
        assert!(report.changed);
        assert_eq!(read_named_acl(&publication_binding), publication_acl);
        assert_eq!(
            read_bound_activation_metadata(&publication_binding)
                .unwrap()
                .0,
            publication_metadata
        );

        let (rollback_root, rollback_binding, rollback_bytes) = stale_rebind_fixture();
        let rollback_acl = install_named_acl_if_supported(&rollback_binding)
            .expect("ACL capability disappeared between publication and rollback fixtures");
        let rollback_metadata = read_bound_activation_metadata(&rollback_binding).unwrap().0;
        let mut original_capture = None;
        let mut before_publish = |_: &Path, _: &Path| Ok(());
        let mut hook = |phase: ActivationTransactionPhase, _: &Path, saved: &Path| {
            if phase == ActivationTransactionPhase::RebindAfterOriginalCapture {
                original_capture = Some(saved.to_path_buf());
            }
            Ok(())
        };
        let mut verifier = |_: &Path| -> Result<WorkspaceActivationReport> {
            bail!("injected named-ACL verification failure")
        };
        let error = rebind_with(
            rollback_root.path(),
            &mut before_publish,
            &mut hook,
            &mut verifier,
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("injected named-ACL verification failure"));
        let original_capture = original_capture.unwrap();
        assert_eq!(fs::read(&rollback_binding).unwrap(), rollback_bytes);
        assert_eq!(read_named_acl(&rollback_binding), rollback_acl);
        assert_eq!(read_named_acl(&original_capture), rollback_acl);
        assert_eq!(
            read_bound_activation_metadata(&rollback_binding).unwrap().0,
            rollback_metadata
        );
        assert_eq!(
            read_bound_activation_metadata(&original_capture).unwrap().0,
            rollback_metadata
        );
    }

    #[test]
    fn rebind_detects_concurrent_binding_and_skill_changes_before_capture() {
        for mutate_skill in [false, true] {
            let (root, binding, original) = stale_rebind_fixture();
            let mut before_publish = |config: &Path, skill_path: &Path| {
                if mutate_skill {
                    fs::write(skill_path, "concurrent skill change\n")?;
                } else {
                    fs::write(config, "concurrent binding change\n")?;
                }
                Ok(())
            };
            let mut hook = no_transaction_hook;
            let mut verifier = |workspace: &Path| activation_status(workspace);
            let error = rebind_with(root.path(), &mut before_publish, &mut hook, &mut verifier)
                .unwrap_err();
            assert!(error.to_string().contains("并发变化"));
            if mutate_skill {
                assert_eq!(fs::read(&binding).unwrap(), original);
            } else {
                assert_eq!(
                    fs::read_to_string(&binding).unwrap(),
                    "concurrent binding change\n"
                );
            }
        }
    }
    #[test]
    fn rebind_publication_does_not_overwrite_a_last_window_target() {
        let (root, binding, original) = stale_rebind_fixture();
        let mut capture = None;
        let mut before_publish = |_: &Path, _: &Path| Ok(());
        let mut hook = |phase: ActivationTransactionPhase, target: &Path, saved: &Path| {
            if phase == ActivationTransactionPhase::RebindAfterOriginalCapture {
                capture = Some(saved.to_path_buf());
                fs::write(target, "concurrent activation\n")?;
            }
            Ok(())
        };
        let mut verifier = |workspace: &Path| activation_status(workspace);
        let error = rebind_with(root.path(), &mut before_publish, &mut hook, &mut verifier)
            .unwrap_err()
            .to_string();
        let capture = capture.unwrap();
        assert_eq!(
            fs::read_to_string(&binding).unwrap(),
            "concurrent activation\n"
        );
        assert_eq!(fs::read(&capture).unwrap(), original);
        assert!(error.contains(&display_path(&capture)));
        assert!(error.contains("refusing") || error.contains("concurrent"));
    }
    #[test]
    fn rebind_rollback_restores_bytes_and_keeps_an_independent_snapshot() {
        let (root, binding, original) = stale_rebind_fixture();
        let expected_metadata = set_distinct_activation_metadata(&binding);
        let mut capture = None;
        let mut before_publish = |_: &Path, _: &Path| Ok(());
        let mut hook = |phase: ActivationTransactionPhase, _: &Path, saved: &Path| {
            if phase == ActivationTransactionPhase::RebindAfterOriginalCapture {
                capture = Some(saved.to_path_buf());
            }
            Ok(())
        };
        let mut verifier = |_: &Path| -> Result<WorkspaceActivationReport> {
            bail!("injected verification failure")
        };
        let error = rebind_with(root.path(), &mut before_publish, &mut hook, &mut verifier)
            .unwrap_err()
            .to_string();
        let capture = capture.unwrap();
        assert!(error.contains("injected verification failure"));
        assert!(error.contains(&display_path(&capture)));
        assert_eq!(fs::read(&binding).unwrap(), original);
        let restored_metadata = read_bound_activation_metadata(&binding).unwrap().0;
        let captured_metadata = read_bound_activation_metadata(&capture).unwrap().0;
        assert_eq!(restored_metadata, expected_metadata);
        assert_eq!(captured_metadata, expected_metadata);
        #[cfg(windows)]
        {
            assert_protected_distinct_dacl(&restored_metadata);
            assert_protected_distinct_dacl(&captured_metadata);
        }
        fs::write(&binding, "post-rollback in-place write\n").unwrap();
        assert_eq!(fs::read(&capture).unwrap(), original);
    }
    #[test]
    fn rebind_rollback_does_not_overwrite_a_recreated_target() {
        let (root, binding, original) = stale_rebind_fixture();
        let mut original_capture = None;
        let mut before_publish = |_: &Path, _: &Path| Ok(());
        let mut hook = |phase: ActivationTransactionPhase, target: &Path, saved: &Path| {
            match phase {
                ActivationTransactionPhase::RebindAfterOriginalCapture => {
                    original_capture = Some(saved.to_path_buf());
                }
                ActivationTransactionPhase::RebindAfterRollbackCapture => {
                    fs::write(target, "rollback-window activation\n")?;
                }
                _ => {}
            }
            Ok(())
        };
        let mut verifier = |_: &Path| -> Result<WorkspaceActivationReport> {
            bail!("injected verification failure")
        };
        let error = rebind_with(root.path(), &mut before_publish, &mut hook, &mut verifier)
            .unwrap_err()
            .to_string();
        let original_capture = original_capture.unwrap();
        assert_eq!(
            fs::read_to_string(&binding).unwrap(),
            "rollback-window activation\n"
        );
        assert_eq!(fs::read(&original_capture).unwrap(), original);
        assert!(error.contains(&display_path(&original_capture)));
    }
    #[test]
    fn rebind_detects_same_bytes_with_a_different_file_identity() {
        let (root, binding, original) = stale_rebind_fixture();
        #[cfg(not(windows))]
        let _ = &original;
        let displaced = binding.with_file_name("displaced-rebind-publication.yaml");
        let mut concurrent_capture = None;
        let mut before_publish = |_: &Path, _: &Path| Ok(());
        let mut hook = |phase: ActivationTransactionPhase, _: &Path, saved: &Path| {
            if phase == ActivationTransactionPhase::RebindAfterRollbackCapture {
                concurrent_capture = Some(saved.to_path_buf());
            }
            Ok(())
        };
        let mut verifier = |workspace: &Path| -> Result<WorkspaceActivationReport> {
            let identical = fs::read(&binding)?;
            fs::rename(&binding, &displaced)?;
            fs::write(&binding, &identical)?;
            activation_status(workspace)
        };
        let error = rebind_with(root.path(), &mut before_publish, &mut hook, &mut verifier)
            .unwrap_err()
            .to_string();
        let concurrent_capture = concurrent_capture.unwrap();
        #[cfg(windows)]
        {
            // The publication handle deliberately denies delete sharing, so a
            // non-cooperating path replacement cannot begin. Rollback removes
            // only its own captured publication and restores the original.
            assert!(!concurrent_capture.exists());
            assert!(!displaced.exists());
            assert_eq!(fs::read(&binding).unwrap(), original);
            assert!(
                error.contains("os error 32")
                    || error.contains("另一个程序正在使用")
                    || error.contains("sharing")
            );
        }
        #[cfg(not(windows))]
        {
            let retained = fs::read(&concurrent_capture).unwrap();
            assert_eq!(fs::read(&binding).unwrap(), retained);
            assert!(error.contains("identity") || error.contains("different activation"));
            fs::write(&binding, "later target write\n").unwrap();
            assert_eq!(fs::read(&concurrent_capture).unwrap(), retained);
        }
    }
    #[test]
    fn rebind_error_after_publication_move_uses_rollback_path() {
        let (root, binding, original) = stale_rebind_fixture();
        let mut capture = None;
        let mut before_publish = |_: &Path, _: &Path| Ok(());
        let mut hook = |phase: ActivationTransactionPhase, _: &Path, saved: &Path| {
            if phase == ActivationTransactionPhase::RebindAfterOriginalCapture {
                capture = Some(saved.to_path_buf());
            }
            if phase == ActivationTransactionPhase::RebindAfterPublicationMove {
                bail!("injected directory sync failure after publication move");
            }
            Ok(())
        };
        let mut verifier = |workspace: &Path| activation_status(workspace);
        let error = rebind_with(root.path(), &mut before_publish, &mut hook, &mut verifier)
            .unwrap_err()
            .to_string();
        let capture = capture.unwrap();
        assert!(error.contains("injected directory sync failure"));
        assert_eq!(fs::read(&binding).unwrap(), original);
        assert_eq!(fs::read(&capture).unwrap(), original);
    }
    #[test]
    fn rebind_success_cleanup_refuses_a_replaced_private_capture() {
        let (root, _, _) = stale_rebind_fixture();
        let mut replaced_capture = None;
        let mut before_publish = |_: &Path, _: &Path| Ok(());
        let mut hook = |phase: ActivationTransactionPhase, _: &Path, saved: &Path| {
            if phase == ActivationTransactionPhase::RebindBeforeSuccessCleanup {
                fs::remove_file(saved)?;
                fs::write(saved, "private capture replacement\n")?;
                replaced_capture = Some(saved.to_path_buf());
            }
            Ok(())
        };
        let mut verifier = |workspace: &Path| activation_status(workspace);
        let error = rebind_with(root.path(), &mut before_publish, &mut hook, &mut verifier)
            .unwrap_err()
            .to_string();
        let replaced_capture = replaced_capture.unwrap();
        assert!(activation_status(root.path()).unwrap().active);
        assert_eq!(
            fs::read_to_string(&replaced_capture).unwrap(),
            "private capture replacement\n"
        );
        assert!(error.contains(&display_path(&replaced_capture)));
    }
    #[cfg(unix)]
    #[test]
    fn rebind_rejects_a_symlinked_skill_path_component() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().unwrap();
        let external = tempfile::tempdir().unwrap();
        fs::write(external.path().join("SKILL.md"), "canonical skill\n").unwrap();
        symlink(external.path(), root.path().join("linked")).unwrap();
        let binding = write_test_binding(
            root.path(),
            "linked/SKILL.md",
            &"0".repeat(64),
            "rayman-cli-contract-v6",
            "2.2.0",
        );
        let original = fs::read(&binding).unwrap();
        let status = activation_status(root.path()).unwrap();
        assert!(!status.rebind_eligible);
        assert!(rebind(root.path()).is_err());
        assert_eq!(fs::read(&binding).unwrap(), original);
    }
    fn assert_unsafe_recorded_path_is_inactive_and_not_rebound(
        root: &Path,
        recorded: &str,
        hash: &str,
    ) {
        let binding = write_test_binding(
            root,
            recorded,
            hash,
            crate::CLI_CONTRACT,
            crate::CLI_VERSION,
        );
        let original = fs::read(&binding).unwrap();

        let status = activation_status(root).unwrap();
        assert!(!status.active, "{recorded}");
        assert!(!status.rebind_eligible, "{recorded}");
        assert!(status.recovery_command.is_none(), "{recorded}");
        assert!(status.actual_sha256.is_none(), "{recorded}");
        assert!(
            status
                .issues
                .iter()
                .any(|issue| issue.contains("安全规范形式")),
            "{recorded}: {:?}",
            status.issues
        );
        assert!(rebind(root).is_err(), "{recorded}");
        assert_eq!(fs::read(&binding).unwrap(), original, "{recorded}");
    }

    #[test]
    fn unsafe_relative_recorded_paths_never_become_active_or_rebindable() {
        let container = tempfile::tempdir().unwrap();
        let root = container.path().join("workspace").join("nested");
        let outside = container.path().join("outside");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&outside).unwrap();
        let outside_skill = outside.join("SKILL.md");
        fs::write(&outside_skill, "outside skill\n").unwrap();
        let outside_hash = sha256_file(&outside_skill).unwrap();
        assert_unsafe_recorded_path_is_inactive_and_not_rebound(
            &root,
            "../../outside/SKILL.md",
            &outside_hash,
        );

        let local_skill = root.join("SKILL.md");
        fs::write(&local_skill, "local skill\n").unwrap();
        let local_hash = sha256_file(&local_skill).unwrap();
        fs::create_dir_all(root.join("child")).unwrap();
        for recorded in ["./SKILL.md", "child/../SKILL.md", "child//../SKILL.md"] {
            assert_unsafe_recorded_path_is_inactive_and_not_rebound(&root, recorded, &local_hash);
        }

        let nested = root.join("dir");
        fs::create_dir_all(&nested).unwrap();
        #[cfg(windows)]
        let backslash_skill = nested.join("SKILL.md");
        #[cfg(not(windows))]
        let backslash_skill = root.join(r"dir\SKILL.md");
        fs::write(&backslash_skill, "backslash skill\n").unwrap();
        let backslash_hash = sha256_file(&backslash_skill).unwrap();
        assert_unsafe_recorded_path_is_inactive_and_not_rebound(
            &root,
            r"dir\SKILL.md",
            &backslash_hash,
        );
    }

    #[test]
    fn absolute_recorded_paths_reject_dot_and_parent_components() {
        let root = tempfile::tempdir().unwrap();
        let external = tempfile::tempdir().unwrap();
        fs::create_dir_all(external.path().join("child")).unwrap();
        let skill = external.path().join("SKILL.md");
        fs::write(&skill, "external skill\n").unwrap();
        let hash = sha256_file(&skill).unwrap();
        for recorded in [
            external.path().join("child").join("..").join("SKILL.md"),
            external.path().join(".").join("SKILL.md"),
        ] {
            let recorded = recorded.to_string_lossy().into_owned();
            assert!(Path::new(&recorded).is_absolute());
            assert_unsafe_recorded_path_is_inactive_and_not_rebound(root.path(), &recorded, &hash);
        }
    }

    #[test]
    fn install_bind_creates_noops_and_rebinds_only_identity_drift() {
        let root = tempfile::tempdir().unwrap();
        let skill = root.path().join("SKILL.md");
        write_running_canonical_skill(&skill);

        let created = install_bind(root.path(), &skill).unwrap();
        assert!(created.changed);
        assert!(created.activation.active);
        let binding = root.path().join(".RaymanCodingSkill/workspace_skill.yaml");
        let created_bytes = fs::read(&binding).unwrap();

        let no_op = install_bind(root.path(), &skill).unwrap();
        assert!(!no_op.changed);
        assert!(no_op.activation.active);
        assert_eq!(fs::read(&binding).unwrap(), created_bytes);

        let current = fs::read_to_string(&binding).unwrap();
        let stale = current
            .replace(
                &format!("cli_contract: {}", crate::CLI_CONTRACT),
                "cli_contract: rayman-cli-contract-v1",
            )
            .replace(
                &format!("cli_version: {}", crate::CLI_VERSION),
                "cli_version: 0.1.0",
            );
        fs::write(&binding, stale).unwrap();
        let expected_metadata = set_distinct_activation_metadata(&binding);

        let rebound = install_bind(root.path(), &skill).unwrap();
        assert!(rebound.changed);
        assert!(rebound.activation.active);
        let rebound_text = fs::read_to_string(&binding).unwrap();
        assert!(rebound_text.contains(&format!("cli_contract: {}", crate::CLI_CONTRACT)));
        assert!(rebound_text.contains(&format!("cli_version: {}", crate::CLI_VERSION)));
        assert_eq!(
            read_bound_activation_metadata(&binding).unwrap().0,
            expected_metadata
        );
    }

    #[test]
    fn install_bind_rejects_path_changes_and_ineligible_contracts_without_writing() {
        let root = tempfile::tempdir().unwrap();
        let first = root.path().join("SKILL.md");
        let second = root.path().join("other-skill.md");
        write_running_canonical_skill(&first);
        write_running_canonical_skill(&second);
        install_bind(root.path(), &first).unwrap();
        let binding = root.path().join(".RaymanCodingSkill/workspace_skill.yaml");

        let active_bytes = fs::read(&binding).unwrap();
        let path_error = install_bind(root.path(), &second).unwrap_err().to_string();
        assert!(path_error.contains("path change"));
        assert_eq!(fs::read(&binding).unwrap(), active_bytes);

        deactivate(root.path()).unwrap();
        let disabled_bytes = fs::read(&binding).unwrap();
        assert!(install_bind(root.path(), &first).is_err());
        assert_eq!(fs::read(&binding).unwrap(), disabled_bytes);

        fs::write(&binding, "skill: raymancodingskill\nunknown: true\n").unwrap();
        let malformed_bytes = fs::read(&binding).unwrap();
        assert!(install_bind(root.path(), &first).is_err());
        assert_eq!(fs::read(&binding).unwrap(), malformed_bytes);
    }

    #[test]
    fn install_bind_rejects_noncanonical_skill_without_creating_a_binding() {
        let root = tempfile::tempdir().unwrap();
        let skill = root.path().join("SKILL.md");
        fs::write(&skill, "not the embedded canonical skill\n").unwrap();

        let error = install_bind(root.path(), &skill).unwrap_err().to_string();
        assert!(error.contains("embedded"));
        assert!(
            !root
                .path()
                .join(".RaymanCodingSkill/workspace_skill.yaml")
                .exists()
        );
    }

    fn missing_install_fixture() -> (tempfile::TempDir, PathBuf, PreparedActivation) {
        let root = tempfile::tempdir().unwrap();
        let skill = root.path().join("SKILL.md");
        write_running_canonical_skill(&skill);
        let binding =
            state_paths::managed_state_file(root.path(), Path::new(ACTIVATION_RELATIVE), true)
                .unwrap();
        let prepared = prepare_activation(root.path(), &skill).unwrap();
        (root, binding, prepared)
    }
    #[test]
    fn install_bind_verification_failure_removes_only_its_publication() {
        let (root, binding, prepared) = missing_install_fixture();
        let mut hook = no_transaction_hook;
        let mut verifier = |_: &Path| -> Result<WorkspaceActivationReport> {
            bail!("injected install-bind verification failure")
        };
        let error = publish_missing_install_binding(
            root.path(),
            &binding,
            &prepared,
            &mut hook,
            &mut verifier,
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("injected install-bind verification failure"));
        assert!(!binding.exists());
    }
    #[test]
    fn install_bind_create_does_not_overwrite_a_last_window_target() {
        let (root, binding, prepared) = missing_install_fixture();
        let mut hook = |phase: ActivationTransactionPhase, target: &Path, _: &Path| {
            if phase == ActivationTransactionPhase::InstallBeforeCreatePublication {
                fs::write(target, "concurrent install activation\n")?;
            }
            Ok(())
        };
        let mut verifier = |workspace: &Path| activation_status(workspace);
        let error = publish_missing_install_binding(
            root.path(),
            &binding,
            &prepared,
            &mut hook,
            &mut verifier,
        )
        .unwrap_err()
        .to_string();
        assert_eq!(
            fs::read_to_string(&binding).unwrap(),
            "concurrent install activation\n"
        );
        assert!(error.contains("refusing overwrite") || error.contains("appeared"));
    }
    #[test]
    fn install_bind_rollback_does_not_delete_a_recreated_target() {
        let (root, binding, prepared) = missing_install_fixture();
        let mut hook = |phase: ActivationTransactionPhase, target: &Path, _: &Path| {
            if phase == ActivationTransactionPhase::InstallAfterRollbackCapture {
                fs::write(target, "rollback-window install activation\n")?;
            }
            Ok(())
        };
        let mut verifier = |_: &Path| -> Result<WorkspaceActivationReport> {
            bail!("injected install-bind verification failure")
        };
        let error = publish_missing_install_binding(
            root.path(),
            &binding,
            &prepared,
            &mut hook,
            &mut verifier,
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("injected install-bind verification failure"));
        assert_eq!(
            fs::read_to_string(&binding).unwrap(),
            "rollback-window install activation\n"
        );
    }
    #[test]
    fn install_bind_detects_replacement_after_publication_move() {
        let (root, binding, prepared) = missing_install_fixture();
        let displaced = binding.with_file_name("displaced-install-publication.yaml");
        let mut capture = None;
        let mut hook = |phase: ActivationTransactionPhase, target: &Path, saved: &Path| {
            match phase {
                ActivationTransactionPhase::InstallAfterPublicationMove => {
                    fs::rename(target, &displaced)?;
                    fs::write(target, "post-move install replacement\n")?;
                }
                ActivationTransactionPhase::InstallAfterRollbackCapture => {
                    capture = Some(saved.to_path_buf());
                }
                _ => {}
            }
            Ok(())
        };
        let mut verifier = |workspace: &Path| activation_status(workspace);
        let error = publish_missing_install_binding(
            root.path(),
            &binding,
            &prepared,
            &mut hook,
            &mut verifier,
        )
        .unwrap_err()
        .to_string();
        let capture = capture.unwrap();
        #[cfg(windows)]
        {
            assert!(!binding.exists());
            assert!(!capture.exists());
            assert!(!displaced.exists());
            assert!(
                error.contains("os error 32")
                    || error.contains("另一个程序正在使用")
                    || error.contains("sharing")
            );
        }
        #[cfg(not(windows))]
        {
            assert_eq!(
                fs::read_to_string(&binding).unwrap(),
                "post-move install replacement\n"
            );
            assert_eq!(
                fs::read_to_string(&capture).unwrap(),
                "post-move install replacement\n"
            );
            assert!(error.contains(&display_path(&capture)));
        }
    }
    #[test]
    fn install_bind_uses_the_shared_activation_lock() {
        let root = tempfile::tempdir().unwrap();
        let skill = root.path().join("SKILL.md");
        write_running_canonical_skill(&skill);
        let binding =
            state_paths::managed_state_file(root.path(), Path::new(ACTIVATION_RELATIVE), true)
                .unwrap();
        let held = crate::state_lock::acquire_state_lock(&binding).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let worker_root = root.path().to_path_buf();
        let worker_skill = skill.clone();
        let worker = std::thread::spawn(move || {
            let _ = tx.send(install_bind(&worker_root, &worker_skill));
        });

        assert!(matches!(
            rx.recv_timeout(std::time::Duration::from_millis(150)),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout)
        ));
        drop(held);
        let result = rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap()
            .unwrap();
        worker.join().unwrap();
        assert!(result.changed);
        assert!(result.activation.active);
    }

    #[test]
    fn retained_evidence_serializes_identity_metadata_and_content_hashes() {
        let (root, binding, _) = stale_rebind_fixture();
        let snapshot = read_file_snapshot(&binding).unwrap();
        let evidence = retained_evidence("test retained activation", &binding, &snapshot, None);
        let value = serde_json::to_value(&evidence).unwrap();
        assert_eq!(value["path"], display_path(&binding));
        assert_eq!(value["sha256"].as_str().unwrap().len(), 64);
        assert_eq!(value["metadata_sha256"].as_str().unwrap().len(), 64);
        assert_eq!(value["length"], snapshot.identity.len);
        assert!(!value["identity"].as_str().unwrap().is_empty());
        assert!(value.get("cleanup_error").is_none());
        drop(root);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_publication_preflight_names_and_rebinds_the_actual_stage() {
        let root = tempfile::tempdir().unwrap();
        let binding =
            state_paths::managed_state_file(root.path(), Path::new(ACTIVATION_RELATIVE), true)
                .unwrap();
        fs::write(&binding, "original activation\n").unwrap();
        let original = read_file_snapshot(&binding).unwrap();
        let transaction = ActivationTransaction::open(&binding).unwrap();

        let stage = create_owned_activation_temp(
            &transaction,
            b"preflight publication\n",
            Some(&original.metadata),
        )
        .unwrap();
        let retained = stage.path.as_deref().expect("named Linux stage");
        assert!(!stage.published);
        assert_eq!(retained.parent(), Some(transaction.retained.path.as_path()));
        assert_eq!(stage.snapshot.identity.nlink, 1);
        assert_eq!(
            read_transaction_retained_snapshot(&transaction, retained).unwrap(),
            stage.snapshot
        );
        if unsafe { libc::geteuid() } == 0 {
            eprintln!(
                "Linux publication preflight ran as root; Ubuntu CI must also run this test as an ordinary user"
            );
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_capture_enoent_race_preserves_verified_bytes_under_a_durable_name() {
        let (_root, binding, original) = stale_rebind_fixture();
        let transaction = ActivationTransaction::open(&binding).unwrap();
        let concurrent = b"concurrent activation after ENOENT\n";
        let mut hook = |phase: ActivationCapturePhase, target: &Path| -> Result<()> {
            match phase {
                ActivationCapturePhase::BeforeMove => fs::remove_file(target)?,
                ActivationCapturePhase::AfterMoveNotFound => fs::write(target, concurrent)?,
            }
            Ok(())
        };

        let error =
            capture_activation_target_with(&transaction, "injected ENOENT capture race", &mut hook)
                .err()
                .expect("injected ENOENT race must fail closed")
                .to_string();
        assert_eq!(fs::read(&binding).unwrap(), concurrent);
        let retained_originals = fs::read_dir(&transaction.retained.path)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| fs::read(entry.path()).is_ok_and(|bytes| bytes == original))
            .count();
        assert!(retained_originals >= 1, "{error}");
        assert!(error.contains("persisted recovery evidence"), "{error}");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_activation_retained_directory_rejects_other_writers() {
        use std::os::unix::fs::PermissionsExt as _;

        let root = tempfile::tempdir().unwrap();
        let state = state_paths::managed_state_root(root.path(), true)
            .unwrap()
            .unwrap();
        let retained = state.join("tmp/activation-retained");
        fs::create_dir_all(&retained).unwrap();
        fs::set_permissions(&retained, fs::Permissions::from_mode(0o777)).unwrap();
        let binding = state.join(ACTIVATION_RELATIVE);
        fs::write(&binding, "activation\n").unwrap();

        let error = ActivationTransaction::open(&binding)
            .err()
            .expect("other-writable retained directory must fail closed")
            .to_string();
        assert!(error.contains("group/other writes disabled"), "{error}");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_inode_flag_policy_replays_only_nodump_and_rejects_unsafe_or_unknown_flags() {
        let path = Path::new("workspace_skill.yaml");
        assert_eq!(LINUX_SAFE_COPY_INODE_FLAGS, FS_NODUMP_FL);
        validate_linux_inode_flags(0, path).unwrap();
        validate_linux_inode_flags(FS_NODUMP_FL | FS_EXTENT_FL, path).unwrap();
        for flag in [
            FS_IMMUTABLE_FL,
            FS_APPEND_FL,
            FS_ENCRYPT_FL,
            FS_VERITY_FL,
            FS_NOATIME_FL,
        ] {
            assert!(validate_linux_inode_flags(flag, path).is_err());
        }
        assert!(validate_linux_inode_flags(0x0100_0000, path).is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_rebind_preserves_nodump_and_reports_retained_evidence_outside_issues() {
        use std::os::fd::AsRawFd as _;

        let (root, binding, original) = stale_rebind_fixture();
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&binding)
            .unwrap();
        let current = linux_inode_flags(&file, &binding).unwrap();
        let mut requested = (current | FS_NODUMP_FL) as libc::c_int;
        let updated =
            unsafe { libc::ioctl(file.as_raw_fd(), libc::FS_IOC_SETFLAGS, &mut requested) };
        assert_eq!(updated, 0, "{}", io::Error::last_os_error());
        drop(file);

        let rebound = rebind(root.path()).unwrap();
        assert!(rebound.changed);
        assert!(rebound.activation.active);
        assert!(rebound.activation.issues.is_empty());
        assert_eq!(rebound.activation.retained_evidence.len(), 1);
        let evidence = &rebound.activation.retained_evidence[0];
        assert_eq!(evidence.sha256, sha256_bytes(&original));
        assert_eq!(evidence.length, original.len() as u64);
        assert_eq!(evidence.metadata_sha256.len(), 64);
        assert!(evidence.identity.contains("nlink=1"));
        assert!(Path::new(&evidence.path).is_file());

        let (metadata, _) = read_bound_activation_metadata(&binding).unwrap();
        assert_ne!(metadata.inode_flags & FS_NODUMP_FL, 0);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_rebind_rejects_a_hardlinked_activation_before_mutation() {
        let (root, binding, original) = stale_rebind_fixture();
        let alias = binding.with_file_name("workspace_skill.hardlink.yaml");
        fs::hard_link(&binding, &alias).unwrap();

        let error = rebind(root.path()).unwrap_err().to_string();
        assert!(error.contains("exactly one hard link"), "{error}");
        assert_eq!(fs::read(&binding).unwrap(), original);
        assert_eq!(fs::read(&alias).unwrap(), original);
    }

    #[cfg(all(unix, not(target_os = "linux")))]
    #[test]
    fn non_linux_unix_write_entries_fail_before_creating_managed_state() {
        let root = tempfile::tempdir().unwrap();
        let skill = root.path().join("SKILL.md");
        write_running_canonical_skill(&skill);

        assert!(activate(root.path(), &skill).is_err());
        assert!(deactivate(root.path()).is_err());
        assert!(rebind(root.path()).is_err());
        assert!(install_bind(root.path(), &skill).is_err());
        assert!(!root.path().join(".RaymanCodingSkill").exists());
    }

    #[cfg(windows)]
    #[test]
    fn windows_alias_and_device_paths_are_not_rebindable() {
        let root = tempfile::tempdir().unwrap();
        let skill = root.path().join("SKILL.md");
        write_running_canonical_skill(&skill);
        let hash = sha256_file(&skill).unwrap();
        assert_unsafe_recorded_path_is_inactive_and_not_rebound(root.path(), "SKILL.md.", &hash);
        assert!(!safe_recorded_skill_file("SKILL.md:alternate"));
        assert!(!safe_recorded_skill_file(r"\\.\PhysicalDrive0"));

        let redundant_absolute = format!(
            "{}\\\\SKILL.md",
            root.path().canonicalize().unwrap().to_string_lossy()
        );
        assert!(!safe_recorded_skill_file(&redundant_absolute));
    }
}
