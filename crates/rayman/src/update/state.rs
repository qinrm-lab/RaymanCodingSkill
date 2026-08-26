//! Durable identities used by the trusted updater.

use std::fmt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::trust::{AssetRole, VerifiedManifest};
use super::{ReleaseVersion, UpdateState};

pub const TRUSTED_STATE_SCHEMA_VERSION: u32 = 1;
pub const INSTALL_RECEIPT_SCHEMA_VERSION: u32 = 1;
pub const WORKER_REQUEST_SCHEMA_VERSION: u32 = 2;
pub const ACTIVE_UPDATE_SCHEMA_VERSION: u32 = 1;
pub const MAX_UPDATE_STATE_BYTES: u64 = 256 * 1024;
pub const MAX_INSTALL_RECEIPT_BYTES: u64 = 256 * 1024;
pub const MAX_TRUSTED_STATE_BYTES: u64 = 64 * 1024;
pub const MAX_WORKER_REQUEST_BYTES: u64 = 64 * 1024;

const UPDATE_RELATIVE_DIR: &str = "Rayman/update";
const INSTALL_RELATIVE_DIR: &str = "Rayman/install";
const REQUESTS_RELATIVE_DIR: &str = "Rayman/update/requests";
const UPDATE_STATE_FILE: &str = "update.json";
const TRUSTED_STATE_FILE: &str = "trusted.json";
const ACTIVE_UPDATE_FILE: &str = "active.json";
const INSTALL_RECEIPT_FILE: &str = "receipt.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrustedFloor {
    pub schema_version: u32,
    pub key_epoch: u32,
    pub sequence: u64,
    pub highest_version: ReleaseVersion,
    pub manifest_sha256: String,
    pub last_seen_at: DateTime<Utc>,
}

impl TrustedFloor {
    pub fn from_verified(manifest: &VerifiedManifest, now: DateTime<Utc>) -> Self {
        Self {
            schema_version: TRUSTED_STATE_SCHEMA_VERSION,
            key_epoch: manifest.manifest().key_epoch,
            sequence: manifest.manifest().sequence,
            highest_version: manifest.manifest().version.clone(),
            manifest_sha256: manifest.sha256().into(),
            last_seen_at: now,
        }
    }

    pub fn validate(&self) -> Result<(), TrustedStateError> {
        if self.schema_version != TRUSTED_STATE_SCHEMA_VERSION
            || self.sequence == 0
            || !is_lower_hex(&self.manifest_sha256, 64)
        {
            return Err(TrustedStateError::CorruptFloor);
        }
        Ok(())
    }

    /// Enforce monotonic release metadata. An exact manifest may resume an
    /// interrupted transaction, but a lower sequence/key epoch/version or a
    /// same-version different digest can never replace the floor.
    pub fn classify(
        &self,
        candidate: &VerifiedManifest,
        installed: &ReleaseVersion,
        now: DateTime<Utc>,
    ) -> Result<FloorDecision, TrustedStateError> {
        self.validate()?;
        let manifest = candidate.manifest();
        if now + Duration::minutes(5) < self.last_seen_at {
            return Err(TrustedStateError::ClockRollback);
        }
        if manifest.version <= *installed
            || manifest.key_epoch < self.key_epoch
            || manifest.sequence < self.sequence
            || manifest.version < self.highest_version
        {
            return Err(TrustedStateError::ReplayOrDowngrade);
        }
        if manifest.version == self.highest_version {
            if candidate.sha256() != self.manifest_sha256 {
                return Err(TrustedStateError::Equivocation);
            }
            if manifest.sequence != self.sequence || manifest.key_epoch != self.key_epoch {
                return Err(TrustedStateError::Equivocation);
            }
            return Ok(FloorDecision::ResumeExact);
        }
        if manifest.sequence <= self.sequence {
            return Err(TrustedStateError::ReplayOrDowngrade);
        }
        Ok(FloorDecision::Advance)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FloorDecision {
    ResumeExact,
    Advance,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstallReceipt {
    pub schema_version: u32,
    pub installation_id: String,
    pub version: ReleaseVersion,
    pub cli_contract: String,
    pub cli_path: PathBuf,
    pub cli_sha256: String,
    pub worker_path: PathBuf,
    pub worker_sha256: String,
    pub skill_root: PathBuf,
    pub resources: Vec<InstalledResource>,
    pub install_manifest_sha256: String,
    pub installed_at: DateTime<Utc>,
    pub source: InstallSource,
    #[serde(default)]
    pub signed_release: Option<SignedReleaseReceipt>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstalledResource {
    pub role: AssetRole,
    pub relative_path: String,
    pub sha256: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallSource {
    SourceFreshInstaller,
    SignedRelease,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignedReleaseReceipt {
    pub manifest_sha256: String,
    pub key_epoch: u32,
    pub sequence: u64,
}

impl InstallReceipt {
    pub fn validate(&self) -> Result<(), TrustedStateError> {
        if self.schema_version != INSTALL_RECEIPT_SCHEMA_VERSION
            || !is_lower_hex(&self.installation_id, 32)
            || !self.cli_path.is_absolute()
            || !self.worker_path.is_absolute()
            || !self.skill_root.is_absolute()
            || !is_cli_contract(&self.cli_contract)
            || !is_lower_hex(&self.cli_sha256, 64)
            || !is_lower_hex(&self.worker_sha256, 64)
            || !is_lower_hex(&self.install_manifest_sha256, 64)
            || self.resources.len() != 3
        {
            return Err(TrustedStateError::CorruptReceipt);
        }
        let expected_cli_name = if cfg!(windows) {
            "rayman.exe"
        } else {
            "rayman"
        };
        let expected_worker_name = if cfg!(windows) {
            format!("rayman-update-worker-{}.exe", self.version)
        } else {
            format!("rayman-update-worker-{}", self.version)
        };
        if self.cli_path.file_name().and_then(|name| name.to_str()) != Some(expected_cli_name)
            || self.worker_path.file_name().and_then(|name| name.to_str())
                != Some(expected_worker_name.as_str())
            || self.cli_path.parent() != self.worker_path.parent()
        {
            return Err(TrustedStateError::CorruptReceipt);
        }
        match (self.source, &self.signed_release) {
            (InstallSource::SourceFreshInstaller, None) => {}
            (InstallSource::SignedRelease, Some(signed))
                if is_lower_hex(&signed.manifest_sha256, 64)
                    && signed.key_epoch > 0
                    && signed.sequence > 0 => {}
            _ => return Err(TrustedStateError::CorruptReceipt),
        }
        let expected = [
            (AssetRole::Skill, "SKILL.md"),
            (AssetRole::AgentContract, "AGENTS.md"),
            (
                AssetRole::WorkflowContract,
                "references/workflow-contract.md",
            ),
        ];
        for (resource, (role, path)) in self.resources.iter().zip(expected) {
            if resource.role != role
                || resource.relative_path != path
                || !is_lower_hex(&resource.sha256, 64)
            {
                return Err(TrustedStateError::CorruptReceipt);
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerRequest {
    pub schema_version: u32,
    pub request_id: String,
    pub candidate: ReleaseVersion,
    pub installation_id: String,
    pub receipt_sha256: String,
    pub prior_version: ReleaseVersion,
    pub cli_path: PathBuf,
    pub worker_path: PathBuf,
    pub worker_sha256: String,
    pub created_at: DateTime<Utc>,
}

impl WorkerRequest {
    pub fn validate_identity(&self) -> Result<(), TrustedStateError> {
        if self.schema_version != WORKER_REQUEST_SCHEMA_VERSION
            || !is_lower_hex(&self.request_id, 32)
            || !is_lower_hex(&self.installation_id, 32)
            || !is_lower_hex(&self.receipt_sha256, 64)
            || !self.cli_path.is_absolute()
            || !self.worker_path.is_absolute()
            || !is_lower_hex(&self.worker_sha256, 64)
            || self.cli_path.parent() != self.worker_path.parent()
        {
            return Err(TrustedStateError::CorruptRequest);
        }
        Ok(())
    }

    pub fn validate(&self, now: DateTime<Utc>) -> Result<(), TrustedStateError> {
        self.validate_identity()?;
        if self.created_at > now + Duration::minutes(5)
            || now - self.created_at > Duration::hours(24)
        {
            return Err(TrustedStateError::CorruptRequest);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActiveUpdateStatus {
    Active,
    RecoveredOld,
    Committed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActiveUpdate {
    pub schema_version: u32,
    pub status: ActiveUpdateStatus,
    pub request_id: String,
    pub candidate: ReleaseVersion,
    pub installation_id: String,
    pub prior_receipt_sha256: String,
    pub prior_version: ReleaseVersion,
    pub cli_path: PathBuf,
    pub worker_path: PathBuf,
    pub worker_sha256: String,
    pub created_at: DateTime<Utc>,
    pub resolved_at: Option<DateTime<Utc>>,
}

impl ActiveUpdate {
    pub fn validate(&self) -> Result<(), TrustedStateError> {
        if self.schema_version != ACTIVE_UPDATE_SCHEMA_VERSION
            || !is_lower_hex(&self.request_id, 32)
            || !is_lower_hex(&self.installation_id, 32)
            || !is_lower_hex(&self.prior_receipt_sha256, 64)
            || !is_lower_hex(&self.worker_sha256, 64)
            || !self.cli_path.is_absolute()
            || !self.worker_path.is_absolute()
            || self.cli_path.parent() != self.worker_path.parent()
            || (self.status == ActiveUpdateStatus::Active && self.resolved_at.is_some())
            || (self.status != ActiveUpdateStatus::Active && self.resolved_at.is_none())
        {
            return Err(TrustedStateError::CorruptActiveUpdate);
        }
        Ok(())
    }
}

fn is_lower_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn is_cli_contract(value: &str) -> bool {
    let Some(number) = value.strip_prefix("rayman-cli-contract-v") else {
        return false;
    };
    !number.is_empty()
        && number.bytes().all(|byte| byte.is_ascii_digit())
        && !number.starts_with('0')
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustedStateError {
    CorruptFloor,
    ClockRollback,
    ReplayOrDowngrade,
    Equivocation,
    CorruptReceipt,
    CorruptRequest,
    CorruptActiveUpdate,
}

impl fmt::Display for TrustedStateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::CorruptFloor => "trusted update floor is corrupt",
            Self::ClockRollback => "trusted update clock moved backwards",
            Self::ReplayOrDowngrade => "update manifest is a replay or downgrade",
            Self::Equivocation => "the same update version has a different trusted manifest",
            Self::CorruptReceipt => "Rayman install receipt is corrupt",
            Self::CorruptRequest => "Rayman update worker request is corrupt",
            Self::CorruptActiveUpdate => "Rayman active update recovery state is corrupt",
        })
    }
}

impl std::error::Error for TrustedStateError {}

pub fn user_data_root() -> Result<PathBuf> {
    #[cfg(all(windows, debug_assertions))]
    if let Some(root) = debug_test_user_data_root()? {
        return Ok(root);
    }
    #[cfg(windows)]
    return windows_local_app_data();

    #[cfg(not(windows))]
    if let Some(xdg) = std::env::var_os("XDG_DATA_HOME") {
        return Ok(PathBuf::from(xdg));
    }
    #[cfg(not(windows))]
    if let Some(home) = std::env::var_os("HOME") {
        return Ok(PathBuf::from(home).join(".local").join("share"));
    }
    #[cfg(not(windows))]
    if let Some(profile) = std::env::var_os("USERPROFILE") {
        return Ok(PathBuf::from(profile));
    }
    #[cfg(not(windows))]
    bail!("unable to determine the user data directory for Rayman update state")
}

#[cfg(all(windows, debug_assertions))]
fn debug_test_user_data_root() -> Result<Option<PathBuf>> {
    let Some(root) = std::env::var_os("RAYMAN_INTERNAL_TEST_UPDATE_ROOT") else {
        return Ok(None);
    };
    let executable = std::env::current_exe().context("cannot inspect debug update executable")?;
    if !is_debug_cargo_executable(&executable) {
        bail!("internal update-root injection is accepted only by a debug Cargo executable");
    }
    let root = PathBuf::from(root);
    if !root.is_absolute() {
        bail!("internal update test root must be absolute");
    }
    Ok(Some(root))
}

#[cfg(all(windows, debug_assertions))]
fn is_debug_cargo_executable(path: &Path) -> bool {
    let Some(parent) = path.parent() else {
        return false;
    };
    let parent_name = parent
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    if parent_name.eq_ignore_ascii_case("debug") {
        return true;
    }
    parent_name.eq_ignore_ascii_case("deps")
        && parent
            .parent()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.eq_ignore_ascii_case("debug"))
}

#[cfg(windows)]
fn windows_local_app_data() -> Result<PathBuf> {
    use std::ptr::null_mut;
    use windows_sys::Win32::System::Com::CoTaskMemFree;
    use windows_sys::Win32::UI::Shell::{FOLDERID_LocalAppData, SHGetKnownFolderPath};

    let mut raw = null_mut();
    let status = unsafe { SHGetKnownFolderPath(&FOLDERID_LocalAppData, 0, null_mut(), &mut raw) };
    if status < 0 || raw.is_null() {
        bail!("cannot resolve the Windows LocalAppData known folder");
    }
    let length = (0..32768usize)
        .find(|index| unsafe { *raw.add(*index) } == 0)
        .ok_or_else(|| anyhow::anyhow!("LocalAppData known-folder path is unterminated"));
    let decoded = length.and_then(|length| {
        String::from_utf16(unsafe { std::slice::from_raw_parts(raw, length) })
            .context("LocalAppData known-folder path is invalid UTF-16")
    });
    unsafe {
        CoTaskMemFree(raw.cast());
    }
    Ok(PathBuf::from(decoded?))
}

pub fn update_state_path(create: bool) -> Result<Option<PathBuf>> {
    managed_user_file(UPDATE_RELATIVE_DIR, UPDATE_STATE_FILE, create)
}

pub fn trusted_state_path(create: bool) -> Result<Option<PathBuf>> {
    managed_user_file(UPDATE_RELATIVE_DIR, TRUSTED_STATE_FILE, create)
}

pub fn active_update_path(create: bool) -> Result<Option<PathBuf>> {
    managed_user_file(UPDATE_RELATIVE_DIR, ACTIVE_UPDATE_FILE, create)
}

pub fn install_receipt_path(create: bool) -> Result<Option<PathBuf>> {
    managed_user_file(INSTALL_RELATIVE_DIR, INSTALL_RECEIPT_FILE, create)
}

pub fn worker_request_path(request_id: &str, create: bool) -> Result<Option<PathBuf>> {
    if !is_lower_hex(request_id, 32) {
        bail!("worker request id is not a strict 128-bit lowercase hex identifier");
    }
    managed_user_file(REQUESTS_RELATIVE_DIR, &format!("{request_id}.json"), create)
}

pub fn load_update_state() -> Result<Option<UpdateState>> {
    let Some(path) = update_state_path(false)? else {
        return Ok(None);
    };
    let mut state = read_optional_json_bounded::<UpdateState>(
        &path,
        "Rayman update preference/cache",
        MAX_UPDATE_STATE_BYTES,
    )?;
    if let Some(state) = state.as_mut() {
        state.migrate()?;
        state.validate()?;
    }
    Ok(state)
}

pub fn save_update_state(path: &Path, state: &UpdateState) -> Result<()> {
    state.validate()?;
    crate::file_io::write_json(path, state)
}

pub fn load_install_receipt() -> Result<Option<(InstallReceipt, String)>> {
    let Some(path) = install_receipt_path(false)? else {
        return Ok(None);
    };
    let Some((bytes, _)) = crate::file_io::read_optional_handle_bound_file_bounded(
        &path,
        "Rayman install receipt",
        MAX_INSTALL_RECEIPT_BYTES,
    )?
    else {
        return Ok(None);
    };
    let receipt: InstallReceipt = serde_json::from_slice(&bytes)
        .with_context(|| format!("invalid Rayman install receipt: {}", path.display()))?;
    receipt.validate()?;
    let sha256 = format!("{:x}", Sha256::digest(&bytes));
    Ok(Some((receipt, sha256)))
}

pub fn load_trusted_floor() -> Result<Option<TrustedFloor>> {
    let Some(path) = trusted_state_path(false)? else {
        return Ok(None);
    };
    let floor = read_optional_json_bounded::<TrustedFloor>(
        &path,
        "Rayman trusted update floor",
        MAX_TRUSTED_STATE_BYTES,
    )?;
    if let Some(floor) = &floor {
        floor.validate()?;
    }
    Ok(floor)
}

pub fn save_trusted_floor(path: &Path, floor: &TrustedFloor) -> Result<()> {
    floor.validate()?;
    crate::file_io::write_json(path, floor)
}

pub fn load_worker_request(request_id: &str) -> Result<Option<WorkerRequest>> {
    let Some(path) = worker_request_path(request_id, false)? else {
        return Ok(None);
    };
    read_optional_json_bounded(
        &path,
        "Rayman update worker request",
        MAX_WORKER_REQUEST_BYTES,
    )
}

pub fn load_active_update() -> Result<Option<ActiveUpdate>> {
    let Some(path) = active_update_path(false)? else {
        return Ok(None);
    };
    let active = read_optional_json_bounded::<ActiveUpdate>(
        &path,
        "Rayman active update recovery state",
        MAX_WORKER_REQUEST_BYTES,
    )?;
    if let Some(active) = &active {
        active.validate()?;
    }
    Ok(active)
}

pub fn save_active_update(path: &Path, active: &ActiveUpdate) -> Result<()> {
    active.validate()?;
    crate::file_io::write_json(path, active)
}

pub fn random_request_id() -> Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).context("cannot obtain randomness for update request")?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

pub fn bound_file_sha256(path: &Path, label: &str, maximum_bytes: u64) -> Result<(String, u64)> {
    let Some((bytes, identity)) =
        crate::file_io::read_optional_handle_bound_file_bounded(path, label, maximum_bytes)?
    else {
        bail!("{label} is missing: {}", path.display());
    };
    Ok((format!("{:x}", Sha256::digest(&bytes)), identity.len))
}

pub fn paths_equal(left: &Path, right: &Path) -> bool {
    let left = left.canonicalize().unwrap_or_else(|_| left.to_path_buf());
    let right = right.canonicalize().unwrap_or_else(|_| right.to_path_buf());
    #[cfg(windows)]
    {
        left.to_string_lossy()
            .eq_ignore_ascii_case(&right.to_string_lossy())
    }
    #[cfg(not(windows))]
    {
        left == right
    }
}

fn managed_user_file(relative_dir: &str, name: &str, create: bool) -> Result<Option<PathBuf>> {
    let root = user_data_root()?;
    let Some(directory) =
        crate::state_paths::managed_external_dir(&root, Path::new(relative_dir), create)?
    else {
        return Ok(None);
    };
    let path = directory.join(name);
    match std::fs::symlink_metadata(&path) {
        Ok(metadata)
            if crate::file_io::is_link_or_reparse(&metadata) || !metadata.file_type().is_file() =>
        {
            bail!(
                "managed update path is not an ordinary file: {}",
                path.display()
            );
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error).with_context(|| {
                format!("cannot inspect managed update file: {}", path.display())
            });
        }
    }
    Ok(Some(path))
}

fn read_optional_json_bounded<T: serde::de::DeserializeOwned>(
    path: &Path,
    label: &str,
    maximum_bytes: u64,
) -> Result<Option<T>> {
    let Some((bytes, _)) =
        crate::file_io::read_optional_handle_bound_file_bounded(path, label, maximum_bytes)?
    else {
        return Ok(None);
    };
    let value = serde_json::from_slice(&bytes)
        .with_context(|| format!("cannot parse {label}: {}", path.display()))?;
    Ok(Some(value))
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    fn time(day: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 8, day, 0, 0, 0)
            .single()
            .unwrap()
    }

    #[cfg(all(windows, debug_assertions))]
    #[test]
    fn internal_update_root_accepts_any_cargo_debug_target_but_not_path_substrings() {
        assert!(is_debug_cargo_executable(Path::new(
            r"C:\repo\target\debug\rayman.exe"
        )));
        assert!(is_debug_cargo_executable(Path::new(
            r"E:\managed\cargo-target\debug\deps\cli-hash.exe"
        )));
        assert!(!is_debug_cargo_executable(Path::new(
            r"C:\repo\target\release\rayman.exe"
        )));
        assert!(!is_debug_cargo_executable(Path::new(
            r"C:\repo\target\debug\nested\rayman.exe"
        )));
        assert!(!is_debug_cargo_executable(Path::new(
            r"C:\repo\target\debugger\rayman.exe"
        )));
    }

    #[test]
    fn receipt_requires_the_exact_managed_resource_tuple() {
        let (cli_path, worker_path, skill_root) = if cfg!(windows) {
            (
                PathBuf::from(r"C:\Users\owner\AppData\Local\Rayman\bin\rayman.exe"),
                PathBuf::from(
                    r"C:\Users\owner\AppData\Local\Rayman\bin\rayman-update-worker-2.11.0.exe",
                ),
                PathBuf::from(r"C:\Users\owner\.codex\skills\raymancodingskill"),
            )
        } else {
            (
                PathBuf::from("/opt/rayman/bin/rayman"),
                PathBuf::from("/opt/rayman/bin/rayman-update-worker-2.11.0"),
                PathBuf::from("/opt/rayman/skill"),
            )
        };
        let receipt = InstallReceipt {
            schema_version: INSTALL_RECEIPT_SCHEMA_VERSION,
            installation_id: "1".repeat(32),
            version: ReleaseVersion::parse("2.11.0").unwrap(),
            cli_contract: "rayman-cli-contract-v18".into(),
            cli_path,
            cli_sha256: "2".repeat(64),
            worker_path,
            worker_sha256: "3".repeat(64),
            skill_root,
            resources: [
                (AssetRole::Skill, "SKILL.md"),
                (AssetRole::AgentContract, "AGENTS.md"),
                (
                    AssetRole::WorkflowContract,
                    "references/workflow-contract.md",
                ),
            ]
            .into_iter()
            .map(|(role, relative_path)| InstalledResource {
                role,
                relative_path: relative_path.into(),
                sha256: "4".repeat(64),
            })
            .collect(),
            install_manifest_sha256: "5".repeat(64),
            installed_at: time(21),
            source: InstallSource::SourceFreshInstaller,
            signed_release: None,
        };
        receipt.validate().unwrap();
        let mut bad = receipt.clone();
        bad.resources.swap(0, 1);
        assert_eq!(bad.validate(), Err(TrustedStateError::CorruptReceipt));
    }
}
