//! Verified update worker orchestration.

use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use serde::Serialize;
use sha2::{Digest, Sha256};

use super::ReleaseVersion;
use super::state;
use super::transport::{
    AssetTransport, AssetTransportError, OfficialAssetSource, manifest_maximum_bytes,
    manifest_source, signature_source,
};
use super::trust::{AssetRole, TrustError, VerifiedManifest, verify_production_manifest};

#[cfg(windows)]
mod windows;

#[derive(Debug, Clone)]
pub struct VerifiedAsset {
    role: AssetRole,
    bytes: Vec<u8>,
    sha256: String,
}

impl VerifiedAsset {
    pub fn role(&self) -> AssetRole {
        self.role
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn sha256(&self) -> &str {
        &self.sha256
    }
}

#[derive(Debug, Clone)]
pub struct VerifiedBundle {
    manifest: VerifiedManifest,
    assets: BTreeMap<AssetRole, VerifiedAsset>,
}

impl VerifiedBundle {
    pub fn manifest(&self) -> &VerifiedManifest {
        &self.manifest
    }

    pub fn asset(&self, role: AssetRole) -> &VerifiedAsset {
        self.assets
            .get(&role)
            .expect("verified bundle contains every required role")
    }
}

/// A production build may expose automatic installation only after its
/// compiled signing root has been provisioned. Discovery and notification do
/// not depend on this capability.
pub fn trusted_install_available() -> bool {
    cfg!(windows) && super::trust::production_trust_ready()
}

#[derive(Debug, Clone, Serialize)]
pub struct WorkerLaunch {
    pub program: PathBuf,
    pub arguments: Vec<String>,
    pub request_id: String,
    pub candidate: ReleaseVersion,
}

#[derive(Debug, Clone, Serialize)]
pub struct WorkerOutcome {
    pub status: String,
    pub installed_version: ReleaseVersion,
    pub manifest_sha256: String,
    pub restart_required: bool,
    pub cleanup_warning: Option<String>,
}

pub fn prepare_worker_request(
    candidate: ReleaseVersion,
    now: DateTime<Utc>,
) -> Result<WorkerLaunch> {
    if !trusted_install_available() {
        bail!("trusted automatic installation is unavailable in this build");
    }
    let config = state::load_update_state()?.unwrap_or_default();
    if !config.auto_install {
        bail!("automatic installation requires explicit `update configure --auto-install --yes`");
    }
    let (receipt, receipt_sha256) = state::load_install_receipt()?
        .ok_or_else(|| anyhow::anyhow!("supported Rayman install receipt is missing"))?;
    verify_receipt_tuple(&receipt, RunningImage::Cli)?;
    if candidate <= receipt.version {
        bail!("worker request candidate is not newer than the installed version");
    }
    let request_id = state::random_request_id()?;
    let request = state::WorkerRequest {
        schema_version: state::WORKER_REQUEST_SCHEMA_VERSION,
        request_id: request_id.clone(),
        candidate: candidate.clone(),
        installation_id: receipt.installation_id.clone(),
        receipt_sha256,
        prior_version: receipt.version.clone(),
        cli_path: receipt.cli_path.clone(),
        worker_path: receipt.worker_path.clone(),
        worker_sha256: receipt.worker_sha256.clone(),
        created_at: now,
    };
    request.validate(now)?;
    let request_path =
        state::worker_request_path(&request_id, true)?.expect("create=true returns a request path");
    crate::file_io::write_json(&request_path, &request)?;
    Ok(WorkerLaunch {
        program: receipt.worker_path,
        arguments: vec!["apply".into(), "--request-id".into(), request_id.clone()],
        request_id,
        candidate,
    })
}

pub fn pending_recovery_launch() -> Result<Option<WorkerLaunch>> {
    if !trusted_install_available() {
        return Ok(None);
    }
    let Some(active) = state::load_active_update()? else {
        return Ok(None);
    };
    if active.status != state::ActiveUpdateStatus::Active {
        return Ok(None);
    }
    let request = state::load_worker_request(&active.request_id)?
        .ok_or_else(|| anyhow::anyhow!("active update request is missing"))?;
    request.validate_identity()?;
    if request.request_id != active.request_id
        || request.candidate != active.candidate
        || request.installation_id != active.installation_id
        || request.receipt_sha256 != active.prior_receipt_sha256
        || request.prior_version != active.prior_version
        || request.cli_path != active.cli_path
        || request.worker_path != active.worker_path
        || request.worker_sha256 != active.worker_sha256
    {
        bail!("active update recovery state does not match its worker request");
    }
    let current = std::env::current_exe().context("cannot resolve the recovery caller image")?;
    if !state::paths_equal(&current, &active.cli_path) {
        bail!("only the managed CLI may request update recovery");
    }
    verify_hash(
        &active.worker_path,
        &active.worker_sha256,
        64 * 1024 * 1024,
        "recovery update worker",
    )?;
    Ok(Some(WorkerLaunch {
        program: active.worker_path,
        arguments: vec![
            "apply".into(),
            "--request-id".into(),
            active.request_id.clone(),
        ],
        request_id: active.request_id,
        candidate: active.candidate,
    }))
}

pub fn run_worker_request(request_id: &str, now: DateTime<Utc>) -> Result<WorkerOutcome> {
    if !trusted_install_available() {
        bail!("trusted automatic installation is unavailable in this build");
    }
    let request = state::load_worker_request(request_id)?
        .ok_or_else(|| anyhow::anyhow!("update worker request is missing"))?;
    request.validate_identity()?;
    let active_before = state::load_active_update()?
        .filter(|active| active.status == state::ActiveUpdateStatus::Active);
    let recovery = active_before.is_some();
    if recovery {
        ensure_active_matches_request(active_before.as_ref().unwrap(), &request)?;
    } else {
        request.validate(now)?;
    }
    let (receipt, receipt_sha256) = state::load_install_receipt()?
        .ok_or_else(|| anyhow::anyhow!("supported Rayman install receipt is missing"))?;
    if receipt.installation_id != request.installation_id || receipt.cli_path != request.cli_path {
        bail!("worker request installation identity or managed CLI path changed");
    }
    if recovery {
        verify_recovery_worker(active_before.as_ref().unwrap())?;
    } else {
        let config = state::load_update_state()?.unwrap_or_default();
        if !config.auto_install {
            bail!("automatic installation consent is not active");
        }
        if receipt_sha256 != request.receipt_sha256
            || request.candidate <= receipt.version
            || receipt.version != request.prior_version
        {
            bail!("worker request no longer matches the installed Rayman identity");
        }
        verify_receipt_tuple(&receipt, RunningImage::Worker)?;
    }

    #[cfg(windows)]
    {
        let _mutex = windows::UpdateMutex::acquire(&request.installation_id)?;
        let current_config = state::load_update_state()?.unwrap_or_default();
        let (current_receipt, current_receipt_sha256) = state::load_install_receipt()?
            .ok_or_else(|| anyhow::anyhow!("Rayman install receipt disappeared"))?;
        if current_receipt.installation_id != request.installation_id
            || current_receipt.cli_path != request.cli_path
        {
            bail!("installed identity changed before the update worker lock");
        }
        let active_path = state::active_update_path(true)?
            .expect("active update state parent is created by the worker");
        let mut active = if recovery {
            let active = state::load_active_update()?
                .ok_or_else(|| anyhow::anyhow!("active update state disappeared"))?;
            ensure_active_matches_request(&active, &request)?;
            verify_recovery_worker(&active)?;
            active
        } else {
            if !current_config.auto_install
                || current_receipt_sha256 != request.receipt_sha256
                || current_receipt != receipt
            {
                bail!("update consent or installed identity changed before the worker lock");
            }
            verify_receipt_tuple(&current_receipt, RunningImage::Worker)?;
            state::ActiveUpdate {
                schema_version: state::ACTIVE_UPDATE_SCHEMA_VERSION,
                status: state::ActiveUpdateStatus::Active,
                request_id: request.request_id.clone(),
                candidate: request.candidate.clone(),
                installation_id: request.installation_id.clone(),
                prior_receipt_sha256: request.receipt_sha256.clone(),
                prior_version: request.prior_version.clone(),
                cli_path: request.cli_path.clone(),
                worker_path: request.worker_path.clone(),
                worker_sha256: request.worker_sha256.clone(),
                created_at: now,
                resolved_at: None,
            }
        };

        let bundle = fetch_production_bundle(
            &super::transport::OfficialAssetTransport,
            request.candidate.clone(),
            now,
        )?;
        if !recovery {
            let floor = state::load_trusted_floor()?;
            if let Some(floor) = &floor {
                floor.classify(&bundle.manifest, &request.prior_version, now)?;
            } else if bundle.manifest.manifest().version <= request.prior_version {
                bail!("verified update bundle is not newer than the installed version");
            }
            // Publication recovery becomes mandatory only after the complete
            // signed bundle is verified and immediately before the first
            // transaction-directory mutation. An opt-out during download can
            // therefore cancel without leaving an artificial recovery grant.
            let latest_config = state::load_update_state()?.unwrap_or_default();
            if !latest_config.auto_install {
                bail!("automatic installation consent was revoked before publication");
            }
            state::save_active_update(&active_path, &active)?;
        }
        let applied =
            windows::apply_verified_bundle(&bundle, &current_receipt, &request, now, recovery)?;
        if applied.recovered_old {
            active.status = state::ActiveUpdateStatus::RecoveredOld;
            active.resolved_at = Some(Utc::now());
            state::save_active_update(&active_path, &active)?;
            return Ok(WorkerOutcome {
                status: "recovered_old".into(),
                installed_version: request.prior_version,
                manifest_sha256: bundle.manifest.sha256().into(),
                restart_required: true,
                cleanup_warning: applied.cleanup_warning,
            });
        }
        let trusted_path = state::trusted_state_path(true)?
            .expect("trusted state parent is created by the worker");
        let trusted = state::TrustedFloor::from_verified(&bundle.manifest, now);
        let trust_warning = state::save_trusted_floor(&trusted_path, &trusted)
            .err()
            .map(|error| {
                format!("installed tuple committed but trusted floor was not cached: {error:#}")
            });
        let cleanup_warning = match (applied.cleanup_warning, trust_warning) {
            (Some(left), Some(right)) => Some(format!("{left}; {right}")),
            (Some(warning), None) | (None, Some(warning)) => Some(warning),
            (None, None) => None,
        };
        active.status = state::ActiveUpdateStatus::Committed;
        active.resolved_at = Some(Utc::now());
        state::save_active_update(&active_path, &active)?;
        Ok(WorkerOutcome {
            status: "installed".into(),
            installed_version: applied.version,
            manifest_sha256: bundle.manifest.sha256().into(),
            restart_required: true,
            cleanup_warning,
        })
    }

    #[cfg(not(windows))]
    {
        let _ = (request, receipt);
        bail!("trusted automatic installation is currently supported only on Windows x86_64")
    }
}

fn ensure_active_matches_request(
    active: &state::ActiveUpdate,
    request: &state::WorkerRequest,
) -> Result<()> {
    active.validate()?;
    if active.status != state::ActiveUpdateStatus::Active
        || active.request_id != request.request_id
        || active.candidate != request.candidate
        || active.installation_id != request.installation_id
        || active.prior_receipt_sha256 != request.receipt_sha256
        || active.prior_version != request.prior_version
        || active.cli_path != request.cli_path
        || active.worker_path != request.worker_path
        || active.worker_sha256 != request.worker_sha256
    {
        bail!("active update recovery state does not match the worker request");
    }
    Ok(())
}

fn verify_recovery_worker(active: &state::ActiveUpdate) -> Result<()> {
    let current = std::env::current_exe().context("cannot resolve the recovery worker image")?;
    if !state::paths_equal(&current, &active.worker_path) {
        bail!("only the receipt-bound prior worker may recover an interrupted update");
    }
    verify_hash(
        &active.worker_path,
        &active.worker_sha256,
        64 * 1024 * 1024,
        "recovery update worker",
    )
}

#[derive(Debug, Clone, Copy)]
enum RunningImage {
    Cli,
    Worker,
}

fn verify_receipt_tuple(receipt: &state::InstallReceipt, running: RunningImage) -> Result<()> {
    receipt.validate()?;
    let current = std::env::current_exe().context("cannot resolve the running Rayman image")?;
    let expected = match running {
        RunningImage::Cli => &receipt.cli_path,
        RunningImage::Worker => &receipt.worker_path,
    };
    if !state::paths_equal(&current, expected) {
        bail!(
            "running image is not the receipt-bound managed {}: running={} expected={}",
            match running {
                RunningImage::Cli => "CLI",
                RunningImage::Worker => "worker",
            },
            current.display(),
            expected.display()
        );
    }

    verify_hash(
        &receipt.cli_path,
        &receipt.cli_sha256,
        64 * 1024 * 1024,
        "installed CLI",
    )?;
    verify_hash(
        &receipt.worker_path,
        &receipt.worker_sha256,
        64 * 1024 * 1024,
        "installed update worker",
    )?;
    for resource in &receipt.resources {
        let path = receipt.skill_root.join(&resource.relative_path);
        verify_hash(
            &path,
            &resource.sha256,
            resource.role.max_size(),
            "installed skill resource",
        )?;
    }
    Ok(())
}

fn verify_hash(path: &std::path::Path, expected: &str, maximum: u64, label: &str) -> Result<()> {
    let (actual, _) = state::bound_file_sha256(path, label, maximum)?;
    if actual != expected {
        bail!(
            "{label} hash no longer matches the supported install receipt: {}",
            path.display()
        );
    }
    Ok(())
}

pub fn fetch_production_bundle<T: AssetTransport>(
    transport: &T,
    candidate: ReleaseVersion,
    now: DateTime<Utc>,
) -> Result<VerifiedBundle, BundleError> {
    fetch_bundle_with_verifier(transport, candidate, now, verify_production_manifest)
}

fn fetch_bundle_with_verifier<T, F>(
    transport: &T,
    candidate: ReleaseVersion,
    now: DateTime<Utc>,
    verify: F,
) -> Result<VerifiedBundle, BundleError>
where
    T: AssetTransport,
    F: FnOnce(&[u8], &[u8], DateTime<Utc>, &ReleaseVersion) -> Result<VerifiedManifest, TrustError>,
{
    let manifest_bytes = transport.fetch(
        &manifest_source(candidate.clone()),
        manifest_maximum_bytes(),
    )?;
    let signature = transport.fetch(
        &signature_source(candidate.clone()),
        super::transport::MAX_SIGNATURE_BYTES,
    )?;
    if signature.len() != super::transport::MAX_SIGNATURE_BYTES {
        return Err(BundleError::SignatureLength);
    }
    let manifest = verify(&manifest_bytes, &signature, now, &candidate)?;

    let mut assets = BTreeMap::new();
    for role in AssetRole::ALL {
        let expected = manifest
            .manifest()
            .asset(role)
            .ok_or(BundleError::AssetSet)?;
        let maximum = usize::try_from(expected.size).map_err(|_| BundleError::AssetSet)?;
        let source = OfficialAssetSource::new(candidate.clone(), role.expected_name())?;
        let bytes = transport.fetch(&source, maximum)?;
        let sha256 = format!("{:x}", Sha256::digest(&bytes));
        if bytes.len() as u64 != expected.size || sha256 != expected.sha256 {
            return Err(BundleError::AssetIntegrity { role });
        }
        let asset = VerifiedAsset {
            role,
            bytes,
            sha256,
        };
        if assets.insert(role, asset).is_some() {
            return Err(BundleError::AssetSet);
        }
    }
    Ok(VerifiedBundle { manifest, assets })
}

#[derive(Debug)]
pub enum BundleError {
    Transport(AssetTransportError),
    Trust(TrustError),
    SignatureLength,
    AssetSet,
    AssetIntegrity { role: AssetRole },
}

impl fmt::Display for BundleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport(error) => write!(formatter, "update asset transport failed: {error}"),
            Self::Trust(error) => write!(formatter, "update manifest trust failed: {error}"),
            Self::SignatureLength => {
                formatter.write_str("update signature must be exactly 64 bytes")
            }
            Self::AssetSet => {
                formatter.write_str("verified update manifest asset set is incomplete")
            }
            Self::AssetIntegrity { role } => {
                write!(
                    formatter,
                    "verified update asset failed size/hash check: {role:?}"
                )
            }
        }
    }
}

impl std::error::Error for BundleError {}

impl From<AssetTransportError> for BundleError {
    fn from(error: AssetTransportError) -> Self {
        Self::Transport(error)
    }
}

impl From<TrustError> for BundleError {
    fn from(error: TrustError) -> Self {
        Self::Trust(error)
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use chrono::{Duration, TimeZone};

    use super::*;
    use crate::update::transport::{MANIFEST_ASSET_NAME, SIGNATURE_ASSET_NAME};
    use crate::update::trust::{
        MANIFEST_PROTOCOL, MANIFEST_SCHEMA_VERSION, ManifestAsset, PRODUCTION_KEY_EPOCH,
        PRODUCTION_KEY_ID, ReleaseManifest, sign_manifest_for_test, verify_manifest_for_test,
    };

    struct FakeTransport {
        values: BTreeMap<&'static str, Vec<u8>>,
        calls: RefCell<Vec<&'static str>>,
    }

    impl AssetTransport for FakeTransport {
        fn fetch(
            &self,
            source: &OfficialAssetSource,
            maximum_bytes: usize,
        ) -> Result<Vec<u8>, AssetTransportError> {
            self.calls.borrow_mut().push(source.asset_name());
            let value = self
                .values
                .get(source.asset_name())
                .cloned()
                .ok_or(AssetTransportError::Unavailable)?;
            if value.len() > maximum_bytes {
                return Err(AssetTransportError::ResponseTooLarge);
            }
            Ok(value)
        }
    }

    fn time(day: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 8, day, 0, 0, 0)
            .single()
            .unwrap()
    }

    fn fixture() -> (ReleaseVersion, FakeTransport) {
        let version = ReleaseVersion::parse("2.11.0").unwrap();
        let mut values = BTreeMap::new();
        let assets = AssetRole::ALL
            .into_iter()
            .map(|role| {
                let bytes = format!("trusted fixture for {role:?}").into_bytes();
                let sha256 = format!("{:x}", Sha256::digest(&bytes));
                values.insert(role.expected_name(), bytes.clone());
                ManifestAsset {
                    role,
                    name: role.expected_name().into(),
                    size: bytes.len() as u64,
                    sha256,
                }
            })
            .collect();
        let manifest = ReleaseManifest {
            schema_version: MANIFEST_SCHEMA_VERSION,
            protocol: MANIFEST_PROTOCOL.into(),
            key_id: PRODUCTION_KEY_ID.into(),
            key_epoch: PRODUCTION_KEY_EPOCH,
            sequence: 12,
            issued_at: time(20),
            expires_at: time(20) + Duration::days(7),
            release_tag: version.release_tag(),
            version: version.clone(),
            commit_sha: "1".repeat(40),
            platform: "windows-x86_64-msvc".into(),
            cli_contract: "rayman-cli-contract-v17".into(),
            install_manifest_sha256: "2".repeat(64),
            assets,
        };
        let (manifest_bytes, signature) = sign_manifest_for_test(&manifest);
        values.insert(MANIFEST_ASSET_NAME, manifest_bytes);
        values.insert(SIGNATURE_ASSET_NAME, signature);
        (
            version,
            FakeTransport {
                values,
                calls: RefCell::new(Vec::new()),
            },
        )
    }

    #[test]
    fn every_asset_is_fetched_only_after_manifest_signature_verification() {
        let (version, transport) = fixture();
        let bundle = fetch_bundle_with_verifier(
            &transport,
            version.clone(),
            time(21),
            verify_manifest_for_test,
        )
        .unwrap();
        assert_eq!(bundle.manifest().manifest().version, version);
        for role in AssetRole::ALL {
            assert_eq!(bundle.asset(role).role(), role);
            assert!(!bundle.asset(role).bytes().is_empty());
            assert_eq!(bundle.asset(role).sha256().len(), 64);
        }
        let calls = transport.calls.borrow();
        assert_eq!(calls[0], MANIFEST_ASSET_NAME);
        assert_eq!(calls[1], SIGNATURE_ASSET_NAME);
        assert_eq!(calls.len(), 2 + AssetRole::ALL.len());
    }

    #[test]
    fn one_modified_asset_fails_before_a_bundle_can_exist() {
        let (version, mut transport) = fixture();
        transport
            .values
            .get_mut(AssetRole::Cli.expected_name())
            .unwrap()[0] ^= 1;
        assert!(matches!(
            fetch_bundle_with_verifier(&transport, version, time(21), verify_manifest_for_test),
            Err(BundleError::AssetIntegrity {
                role: AssetRole::Cli
            })
        ));
    }
}
