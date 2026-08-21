//! Authenticated release-manifest boundary for automatic updates.
//!
//! GitHub release metadata and cached observations never enter this module as
//! authority.  Only a canonical manifest whose detached Ed25519 signature
//! verifies under the compiled production root can construct
//! [`VerifiedManifest`].

use std::collections::BTreeSet;
use std::fmt;

use chrono::{DateTime, Duration, Utc};
use ed25519_dalek::{Signature, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::ReleaseVersion;

pub const MANIFEST_SCHEMA_VERSION: u32 = 1;
pub const MANIFEST_PROTOCOL: &str = "rayman.update.manifest.v1";
pub const MANIFEST_SIGNATURE_DOMAIN: &[u8] = b"RaymanCodingSkill update manifest v1\0";
pub const MAX_MANIFEST_BYTES: usize = 64 * 1024;
pub const PRODUCTION_KEY_ID: &str = "rayman-update-root-1";
pub const PRODUCTION_KEY_EPOCH: u32 = 1;

// Provisioning this public value is intentionally a separate, visible release
// boundary.  An empty value disables trusted apply; it is never interpreted as
// an all-zero key and there is no unsigned or test-key fallback.
const PRODUCTION_PUBLIC_KEY_HEX: &str =
    "6a2d2fa646d9da7cc0b66ce22895564e7725a3b923e58e95b8b088c8a99ad00c";
const FORBIDDEN_TEST_PUBLIC_KEY_HEX: &str =
    "ea4a6c63e29c520abef5507b132ec5f9954776aebebe7b92421eea691446d22c";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseManifest {
    pub schema_version: u32,
    pub protocol: String,
    pub key_id: String,
    pub key_epoch: u32,
    pub sequence: u64,
    pub issued_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub release_tag: String,
    pub version: ReleaseVersion,
    pub commit_sha: String,
    pub platform: String,
    pub cli_contract: String,
    pub install_manifest_sha256: String,
    pub assets: Vec<ManifestAsset>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestAsset {
    pub role: AssetRole,
    pub name: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssetRole {
    Cli,
    UpdateWorker,
    Skill,
    AgentContract,
    WorkflowContract,
    InstallerScript,
}

impl AssetRole {
    pub const ALL: [Self; 6] = [
        Self::Cli,
        Self::UpdateWorker,
        Self::Skill,
        Self::AgentContract,
        Self::WorkflowContract,
        Self::InstallerScript,
    ];

    pub const fn expected_name(self) -> &'static str {
        match self {
            Self::Cli => "rayman-windows-x86_64.exe",
            Self::UpdateWorker => "rayman-update-worker-windows-x86_64.exe",
            Self::Skill => "raymancodingskill-SKILL.md",
            Self::AgentContract => "raymancodingskill-AGENTS.md",
            Self::WorkflowContract => "raymancodingskill-workflow-contract.md",
            Self::InstallerScript => "install-rayman.ps1",
        }
    }

    pub const fn max_size(self) -> u64 {
        match self {
            Self::Cli | Self::UpdateWorker => 64 * 1024 * 1024,
            Self::Skill | Self::AgentContract | Self::WorkflowContract | Self::InstallerScript => {
                2 * 1024 * 1024
            }
        }
    }
}

impl ReleaseManifest {
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, TrustError> {
        serde_json::to_vec(self).map_err(|_| TrustError::MalformedManifest)
    }

    pub fn asset(&self, role: AssetRole) -> Option<&ManifestAsset> {
        self.assets.iter().find(|asset| asset.role == role)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedManifest {
    manifest: ReleaseManifest,
    canonical_bytes: Vec<u8>,
    sha256: String,
}

impl VerifiedManifest {
    pub fn manifest(&self) -> &ReleaseManifest {
        &self.manifest
    }

    pub fn canonical_bytes(&self) -> &[u8] {
        &self.canonical_bytes
    }

    pub fn sha256(&self) -> &str {
        &self.sha256
    }
}

#[derive(Clone)]
struct TrustRoot {
    key_id: &'static str,
    epoch: u32,
    key: VerifyingKey,
}

pub fn production_trust_ready() -> bool {
    production_trust_root().is_ok()
}

pub fn verify_production_manifest(
    manifest_bytes: &[u8],
    signature_bytes: &[u8],
    now: DateTime<Utc>,
    expected_version: &ReleaseVersion,
) -> Result<VerifiedManifest, TrustError> {
    let root = production_trust_root()?;
    verify_manifest_with_root(
        manifest_bytes,
        signature_bytes,
        now,
        expected_version,
        &root,
    )
}

fn production_trust_root() -> Result<TrustRoot, TrustError> {
    let key = production_verifying_key(PRODUCTION_PUBLIC_KEY_HEX)?;
    Ok(TrustRoot {
        key_id: PRODUCTION_KEY_ID,
        epoch: PRODUCTION_KEY_EPOCH,
        key,
    })
}

fn production_verifying_key(value: &str) -> Result<VerifyingKey, TrustError> {
    if value.is_empty() {
        return Err(TrustError::ProductionRootNotConfigured);
    }
    let bytes = decode_hex_array::<32>(value).ok_or(TrustError::ProductionRootInvalid)?;
    if bytes.iter().all(|byte| *byte == 0) || value == FORBIDDEN_TEST_PUBLIC_KEY_HEX {
        return Err(TrustError::ProductionRootInvalid);
    }
    let key = VerifyingKey::from_bytes(&bytes).map_err(|_| TrustError::ProductionRootInvalid)?;
    if key.is_weak() {
        return Err(TrustError::ProductionRootInvalid);
    }
    Ok(key)
}

fn verify_manifest_with_root(
    manifest_bytes: &[u8],
    signature_bytes: &[u8],
    now: DateTime<Utc>,
    expected_version: &ReleaseVersion,
    root: &TrustRoot,
) -> Result<VerifiedManifest, TrustError> {
    if manifest_bytes.is_empty() || manifest_bytes.len() > MAX_MANIFEST_BYTES {
        return Err(TrustError::ManifestSize);
    }
    if manifest_bytes.starts_with(&[0xef, 0xbb, 0xbf]) {
        return Err(TrustError::NonCanonicalManifest);
    }
    let manifest: ReleaseManifest =
        serde_json::from_slice(manifest_bytes).map_err(|_| TrustError::MalformedManifest)?;
    let canonical = manifest.canonical_bytes()?;
    // This simultaneously rejects whitespace variants, duplicate JSON keys,
    // alternate key order, and any other parser/serializer ambiguity.
    if canonical != manifest_bytes {
        return Err(TrustError::NonCanonicalManifest);
    }
    validate_manifest(&manifest, now, expected_version, root.key_id, root.epoch)?;

    let signature = Signature::from_slice(signature_bytes).map_err(|_| TrustError::BadSignature)?;
    let mut signed = Vec::with_capacity(MANIFEST_SIGNATURE_DOMAIN.len() + manifest_bytes.len());
    signed.extend_from_slice(MANIFEST_SIGNATURE_DOMAIN);
    signed.extend_from_slice(manifest_bytes);
    root.key
        .verify_strict(&signed, &signature)
        .map_err(|_| TrustError::BadSignature)?;

    let sha256 = format!("{:x}", Sha256::digest(manifest_bytes));
    Ok(VerifiedManifest {
        manifest,
        canonical_bytes: canonical,
        sha256,
    })
}

fn validate_manifest(
    manifest: &ReleaseManifest,
    now: DateTime<Utc>,
    expected_version: &ReleaseVersion,
    expected_key_id: &str,
    expected_key_epoch: u32,
) -> Result<(), TrustError> {
    if manifest.schema_version != MANIFEST_SCHEMA_VERSION || manifest.protocol != MANIFEST_PROTOCOL
    {
        return Err(TrustError::UnsupportedProtocol);
    }
    if manifest.key_id != expected_key_id || manifest.key_epoch != expected_key_epoch {
        return Err(TrustError::UnknownTrustRoot);
    }
    if manifest.sequence == 0 {
        return Err(TrustError::InvalidSequence);
    }
    if manifest.issued_at > now + Duration::minutes(5)
        || manifest.expires_at <= now
        || manifest.expires_at <= manifest.issued_at
        || manifest.expires_at - manifest.issued_at > Duration::days(31)
    {
        return Err(TrustError::InvalidValidityWindow);
    }
    if &manifest.version != expected_version
        || manifest.release_tag != expected_version.release_tag()
    {
        return Err(TrustError::VersionMismatch);
    }
    if manifest.platform != "windows-x86_64-msvc" {
        return Err(TrustError::PlatformMismatch);
    }
    if !is_lower_hex(&manifest.commit_sha, 40)
        || !is_lower_hex(&manifest.install_manifest_sha256, 64)
        || !is_cli_contract(&manifest.cli_contract)
    {
        return Err(TrustError::InvalidIdentity);
    }
    if manifest.assets.len() != AssetRole::ALL.len() {
        return Err(TrustError::InvalidAssetSet);
    }
    let mut roles = BTreeSet::new();
    let mut names = BTreeSet::new();
    for (index, asset) in manifest.assets.iter().enumerate() {
        if asset.role != AssetRole::ALL[index]
            || asset.name != asset.role.expected_name()
            || asset.size == 0
            || asset.size > asset.role.max_size()
            || !is_lower_hex(&asset.sha256, 64)
            || !roles.insert(asset.role)
            || !names.insert(asset.name.to_ascii_lowercase())
        {
            return Err(TrustError::InvalidAssetSet);
        }
    }
    Ok(())
}

/// Release-side shape check.  It proves canonical schema/identity before a
/// signing payload is emitted, but it does not claim that a private key exists
/// or that a signature was produced.
pub fn validate_unsigned_release_manifest(
    manifest: &ReleaseManifest,
    now: DateTime<Utc>,
) -> Result<(), TrustError> {
    validate_manifest(
        manifest,
        now,
        &manifest.version,
        PRODUCTION_KEY_ID,
        PRODUCTION_KEY_EPOCH,
    )
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

fn decode_hex_array<const N: usize>(value: &str) -> Option<[u8; N]> {
    if value.len() != N * 2 {
        return None;
    }
    let mut output = [0u8; N];
    for (index, byte) in output.iter_mut().enumerate() {
        let start = index * 2;
        *byte = u8::from_str_radix(&value[start..start + 2], 16).ok()?;
    }
    Some(output)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustError {
    ProductionRootNotConfigured,
    ProductionRootInvalid,
    ManifestSize,
    MalformedManifest,
    NonCanonicalManifest,
    UnsupportedProtocol,
    UnknownTrustRoot,
    InvalidSequence,
    InvalidValidityWindow,
    VersionMismatch,
    PlatformMismatch,
    InvalidIdentity,
    InvalidAssetSet,
    BadSignature,
}

impl fmt::Display for TrustError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::ProductionRootNotConfigured => "production update trust root is not configured",
            Self::ProductionRootInvalid => "production update trust root is invalid",
            Self::ManifestSize => "update manifest size is invalid",
            Self::MalformedManifest => "update manifest is malformed",
            Self::NonCanonicalManifest => "update manifest is not canonical JSON",
            Self::UnsupportedProtocol => "update manifest protocol is unsupported",
            Self::UnknownTrustRoot => "update manifest names an unknown trust root",
            Self::InvalidSequence => "update manifest sequence is invalid",
            Self::InvalidValidityWindow => "update manifest validity window is invalid",
            Self::VersionMismatch => "update manifest version does not match the candidate",
            Self::PlatformMismatch => "update manifest platform is unsupported",
            Self::InvalidIdentity => "update manifest identity fields are invalid",
            Self::InvalidAssetSet => "update manifest asset set is invalid",
            Self::BadSignature => "update manifest signature is invalid",
        })
    }
}

impl std::error::Error for TrustError {}

#[cfg(test)]
pub(crate) fn sign_manifest_for_test(manifest: &ReleaseManifest) -> (Vec<u8>, Vec<u8>) {
    use ed25519_dalek::{Signer as _, SigningKey};

    let signing = SigningKey::from_bytes(&[7u8; 32]);
    let bytes = manifest
        .canonical_bytes()
        .expect("test manifest must serialize");
    let mut payload = MANIFEST_SIGNATURE_DOMAIN.to_vec();
    payload.extend_from_slice(&bytes);
    (bytes, signing.sign(&payload).to_bytes().to_vec())
}

#[cfg(test)]
pub(crate) fn verify_manifest_for_test(
    bytes: &[u8],
    signature: &[u8],
    now: DateTime<Utc>,
    expected_version: &ReleaseVersion,
) -> Result<VerifiedManifest, TrustError> {
    use ed25519_dalek::SigningKey;

    let signing = SigningKey::from_bytes(&[7u8; 32]);
    let root = TrustRoot {
        key_id: PRODUCTION_KEY_ID,
        epoch: PRODUCTION_KEY_EPOCH,
        key: signing.verifying_key(),
    };
    verify_manifest_with_root(bytes, signature, now, expected_version, &root)
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use ed25519_dalek::{Signer, SigningKey};

    use super::*;

    fn time(day: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 8, day, 0, 0, 0)
            .single()
            .unwrap()
    }

    fn test_key() -> (SigningKey, TrustRoot) {
        let signing = SigningKey::from_bytes(&[7u8; 32]);
        let root = TrustRoot {
            key_id: PRODUCTION_KEY_ID,
            epoch: PRODUCTION_KEY_EPOCH,
            key: signing.verifying_key(),
        };
        (signing, root)
    }

    fn manifest() -> ReleaseManifest {
        ReleaseManifest {
            schema_version: MANIFEST_SCHEMA_VERSION,
            protocol: MANIFEST_PROTOCOL.into(),
            key_id: PRODUCTION_KEY_ID.into(),
            key_epoch: PRODUCTION_KEY_EPOCH,
            sequence: 11,
            issued_at: time(20),
            expires_at: time(25),
            release_tag: "v2.11.0".into(),
            version: ReleaseVersion::parse("2.11.0").unwrap(),
            commit_sha: "1".repeat(40),
            platform: "windows-x86_64-msvc".into(),
            cli_contract: "rayman-cli-contract-v17".into(),
            install_manifest_sha256: "2".repeat(64),
            assets: AssetRole::ALL
                .into_iter()
                .map(|role| ManifestAsset {
                    role,
                    name: role.expected_name().into(),
                    size: 128,
                    sha256: "3".repeat(64),
                })
                .collect(),
        }
    }

    fn signed_bytes(manifest: &ReleaseManifest, signing: &SigningKey) -> (Vec<u8>, Vec<u8>) {
        let bytes = manifest.canonical_bytes().unwrap();
        let mut payload = MANIFEST_SIGNATURE_DOMAIN.to_vec();
        payload.extend_from_slice(&bytes);
        (bytes, signing.sign(&payload).to_bytes().to_vec())
    }

    #[test]
    fn canonical_signed_manifest_is_the_only_verified_constructor() {
        let (signing, root) = test_key();
        let manifest = manifest();
        let (bytes, signature) = signed_bytes(&manifest, &signing);
        let verified =
            verify_manifest_with_root(&bytes, &signature, time(21), &manifest.version, &root)
                .unwrap();
        assert_eq!(verified.manifest(), &manifest);
        assert_eq!(verified.canonical_bytes(), bytes);
        assert_eq!(verified.sha256().len(), 64);
    }

    #[test]
    fn mutation_wrong_key_and_unsigned_input_never_verify() {
        let (signing, root) = test_key();
        let manifest = manifest();
        let (bytes, signature) = signed_bytes(&manifest, &signing);

        let mut mutated = bytes.clone();
        let index = mutated.iter().position(|byte| *byte == b'2').unwrap();
        mutated[index] = b'4';
        assert!(
            verify_manifest_with_root(&mutated, &signature, time(21), &manifest.version, &root)
                .is_err()
        );

        let other = SigningKey::from_bytes(&[8u8; 32]);
        let (_, wrong_signature) = signed_bytes(&manifest, &other);
        assert_eq!(
            verify_manifest_with_root(&bytes, &wrong_signature, time(21), &manifest.version, &root),
            Err(TrustError::BadSignature)
        );
        assert_eq!(
            verify_manifest_with_root(&bytes, &[], time(21), &manifest.version, &root),
            Err(TrustError::BadSignature)
        );
    }

    #[test]
    fn duplicate_or_noncanonical_json_is_rejected_before_signature_authority() {
        let (signing, root) = test_key();
        let manifest = manifest();
        let canonical = manifest.canonical_bytes().unwrap();
        let remainder = canonical.strip_prefix(br#"{"schema_version":1,"#).unwrap();
        let mut bytes = br#"{"schema_version":1,"schema_version":1,"#.to_vec();
        bytes.extend_from_slice(remainder);
        let mut payload = MANIFEST_SIGNATURE_DOMAIN.to_vec();
        payload.extend_from_slice(&bytes);
        let duplicate_signature = signing.sign(&payload).to_bytes();
        assert_eq!(
            verify_manifest_with_root(
                &bytes,
                &duplicate_signature,
                time(21),
                &manifest.version,
                &root
            ),
            Err(TrustError::MalformedManifest)
        );

        let mut spaced = b" ".to_vec();
        spaced.extend_from_slice(&canonical);
        let mut payload = MANIFEST_SIGNATURE_DOMAIN.to_vec();
        payload.extend_from_slice(&spaced);
        let signature = signing.sign(&payload).to_bytes();
        assert_eq!(
            verify_manifest_with_root(&spaced, &signature, time(21), &manifest.version, &root),
            Err(TrustError::NonCanonicalManifest)
        );
    }

    #[test]
    fn manifest_identity_asset_and_time_mismatches_fail_closed() {
        let (signing, root) = test_key();
        for mutate in 0..5 {
            let mut manifest = manifest();
            match mutate {
                0 => manifest.release_tag = "v2.12.0".into(),
                1 => manifest.platform = "linux-x86_64".into(),
                2 => {
                    manifest.assets.pop();
                }
                3 => manifest.assets[0].name = "other.exe".into(),
                _ => manifest.expires_at = time(20),
            }
            let (bytes, signature) = signed_bytes(&manifest, &signing);
            assert!(
                verify_manifest_with_root(
                    &bytes,
                    &signature,
                    time(21),
                    &ReleaseVersion::parse("2.11.0").unwrap(),
                    &root
                )
                .is_err(),
                "mutation={mutate}"
            );
        }
    }

    #[test]
    fn production_key_parser_rejects_missing_zero_weak_and_test_roots() {
        assert_eq!(
            production_verifying_key("").err(),
            Some(TrustError::ProductionRootNotConfigured)
        );
        assert_eq!(
            production_verifying_key(&"0".repeat(64)).err(),
            Some(TrustError::ProductionRootInvalid)
        );
        let weak_identity = format!("01{}", "00".repeat(31));
        assert_eq!(
            production_verifying_key(&weak_identity).err(),
            Some(TrustError::ProductionRootInvalid)
        );
        assert_eq!(
            production_verifying_key(FORBIDDEN_TEST_PUBLIC_KEY_HEX).err(),
            Some(TrustError::ProductionRootInvalid)
        );
    }

    #[test]
    fn provisioned_production_root_is_valid_and_distinct_from_test_authority() {
        assert!(production_trust_ready());
        let root = production_trust_root().expect("production root must be provisioned");
        assert_eq!(root.key_id, PRODUCTION_KEY_ID);
        assert_eq!(root.epoch, PRODUCTION_KEY_EPOCH);
        assert!(!root.key.is_weak());
        assert_ne!(PRODUCTION_PUBLIC_KEY_HEX, FORBIDDEN_TEST_PUBLIC_KEY_HEX);
    }
}
