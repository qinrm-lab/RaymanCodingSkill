use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use clap::{Parser, Subcommand};
use sha2::{Digest, Sha256};

#[derive(Parser)]
#[command(
    name = "rayman-update-worker",
    version,
    about = "Receipt-bound worker for verified RaymanCodingSkill updates"
)]
struct WorkerCli {
    #[command(subcommand)]
    command: WorkerCommand,
}

#[derive(Subcommand)]
enum WorkerCommand {
    /// Apply one opaque request from the fixed user-level update root.
    Apply {
        #[arg(long)]
        request_id: String,
    },
    /// Release-side only: create canonical manifest bytes and signing payload.
    #[command(hide = true)]
    CreateManifest {
        #[arg(long)]
        asset_root: PathBuf,
        #[arg(long)]
        install_manifest: PathBuf,
        #[arg(long)]
        commit: String,
        #[arg(long)]
        sequence: u64,
        #[arg(long)]
        issued_at: String,
        #[arg(long)]
        expires_at: String,
        #[arg(long)]
        output: PathBuf,
        #[arg(long)]
        signing_payload: PathBuf,
    },
    /// Release-side only: verify a detached signature with the compiled production root.
    #[command(hide = true)]
    VerifyManifest {
        #[arg(long)]
        manifest: PathBuf,
        #[arg(long)]
        signature: PathBuf,
        /// Release-monitor only: verify a specific stable version with this
        /// worker's compiled production root. Apply never accepts this input.
        #[arg(long)]
        expected_version: Option<rayman::update::ReleaseVersion>,
    },
}

fn main() {
    let cli = WorkerCli::parse();
    let result = match cli.command {
        WorkerCommand::Apply { request_id } => {
            rayman::update::install::run_worker_request(&request_id, Utc::now())
                .and_then(|outcome| serde_json::to_value(outcome).map_err(Into::into))
        }
        WorkerCommand::CreateManifest {
            asset_root,
            install_manifest,
            commit,
            sequence,
            issued_at,
            expires_at,
            output,
            signing_payload,
        } => create_manifest(
            &asset_root,
            &install_manifest,
            &commit,
            sequence,
            &issued_at,
            &expires_at,
            &output,
            &signing_payload,
        ),
        WorkerCommand::VerifyManifest {
            manifest,
            signature,
            expected_version,
        } => verify_manifest(&manifest, &signature, expected_version),
    };
    match result {
        Ok(outcome) => match serde_json::to_string_pretty(&outcome) {
            Ok(text) => println!("{text}"),
            Err(error) => fail(anyhow::Error::new(error)),
        },
        Err(error) => fail(error),
    }
}

fn verify_manifest(
    manifest_path: &Path,
    signature_path: &Path,
    expected_version: Option<rayman::update::ReleaseVersion>,
) -> Result<serde_json::Value> {
    use rayman::update::trust::{MAX_MANIFEST_BYTES, verify_production_manifest};

    let manifest = read_ordinary_file(manifest_path, MAX_MANIFEST_BYTES as u64)?;
    let signature = read_ordinary_file(signature_path, 64)?;
    if signature.len() != 64 {
        bail!("Ed25519 detached signature is not exactly 64 bytes");
    }
    let expected_version =
        expected_version.unwrap_or_else(rayman::update::compiled_release_version);
    let verified = verify_production_manifest(&manifest, &signature, Utc::now(), &expected_version)
        .context(
            "manifest failed production-root signature or expected release identity validation",
        )?;
    Ok(serde_json::json!({
        "status": "manifest_verified",
        "manifest_sha256": verified.sha256(),
        "version": verified.manifest().version,
        "sequence": verified.manifest().sequence,
        "key_id": verified.manifest().key_id,
        "key_epoch": verified.manifest().key_epoch,
        "release_tag": verified.manifest().release_tag,
        "commit_sha": verified.manifest().commit_sha,
        "issued_at": verified.manifest().issued_at,
        "expires_at": verified.manifest().expires_at,
    }))
}

#[allow(clippy::too_many_arguments)]
fn create_manifest(
    asset_root: &Path,
    install_manifest_path: &Path,
    commit: &str,
    sequence: u64,
    issued_at: &str,
    expires_at: &str,
    output: &Path,
    signing_payload: &Path,
) -> Result<serde_json::Value> {
    use rayman::update::trust::{
        AssetRole, MANIFEST_PROTOCOL, MANIFEST_SCHEMA_VERSION, MANIFEST_SIGNATURE_DOMAIN,
        ManifestAsset, PRODUCTION_KEY_EPOCH, PRODUCTION_KEY_ID, ReleaseManifest,
        validate_unsigned_release_manifest,
    };

    if !rayman::update::trust::production_trust_ready() {
        bail!(
            "production update public key is not provisioned; refusing to emit a releasable signing payload"
        );
    }
    if sequence == 0 {
        bail!("release manifest sequence must be positive");
    }
    let issued_at = parse_time(issued_at)?;
    let expires_at = parse_time(expires_at)?;
    let asset_root = asset_root
        .canonicalize()
        .context("cannot canonicalize release asset root")?;
    let mut assets = Vec::new();
    for role in AssetRole::ALL {
        let path = asset_root.join(role.expected_name());
        let bytes = read_ordinary_file(&path, role.max_size())?;
        assets.push(ManifestAsset {
            role,
            name: role.expected_name().into(),
            size: bytes.len() as u64,
            sha256: format!("{:x}", Sha256::digest(&bytes)),
        });
    }
    let install_manifest = read_ordinary_file(install_manifest_path, 2 * 1024 * 1024)?;
    let version = rayman::update::compiled_release_version();
    let manifest = ReleaseManifest {
        schema_version: MANIFEST_SCHEMA_VERSION,
        protocol: MANIFEST_PROTOCOL.into(),
        key_id: PRODUCTION_KEY_ID.into(),
        key_epoch: PRODUCTION_KEY_EPOCH,
        sequence,
        issued_at,
        expires_at,
        release_tag: version.release_tag(),
        version,
        commit_sha: commit.into(),
        platform: "windows-x86_64-msvc".into(),
        cli_contract: rayman::CLI_CONTRACT.into(),
        install_manifest_sha256: format!("{:x}", Sha256::digest(&install_manifest)),
        assets,
    };
    validate_unsigned_release_manifest(&manifest, issued_at)?;
    let bytes = manifest.canonical_bytes()?;
    let mut payload = MANIFEST_SIGNATURE_DOMAIN.to_vec();
    payload.extend_from_slice(&bytes);
    write_new_file(output, &bytes)?;
    write_new_file(signing_payload, &payload)?;
    Ok(serde_json::json!({
        "status": "manifest_created",
        "manifest": output,
        "signing_payload": signing_payload,
        "manifest_sha256": format!("{:x}", Sha256::digest(&bytes)),
        "version": manifest.version,
        "sequence": sequence,
        "key_id": PRODUCTION_KEY_ID,
        "key_epoch": PRODUCTION_KEY_EPOCH,
    }))
}

fn parse_time(value: &str) -> Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|value| value.with_timezone(&Utc))
        .with_context(|| format!("invalid RFC3339 timestamp: {value}"))
}

fn read_ordinary_file(path: &Path, maximum: u64) -> Result<Vec<u8>> {
    let metadata = std::fs::symlink_metadata(path)
        .with_context(|| format!("release input is missing: {}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() > maximum {
        bail!(
            "release input is not a bounded ordinary file: {}",
            path.display()
        );
    }
    std::fs::read(path).with_context(|| format!("cannot read release input: {}", path.display()))
}

fn write_new_file(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("release output has no parent: {}", path.display()))?;
    let metadata = std::fs::symlink_metadata(parent)
        .with_context(|| format!("release output directory is missing: {}", parent.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        bail!(
            "release output parent is not an ordinary directory: {}",
            parent.display()
        );
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .with_context(|| {
            format!(
                "release output already exists or cannot be created: {}",
                path.display()
            )
        })?;
    use std::io::Write as _;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

fn fail(error: anyhow::Error) -> ! {
    let causes = error.chain().map(ToString::to_string).collect::<Vec<_>>();
    let payload = serde_json::json!({
        "status": "failed",
        "error": format!("{error:#}"),
        "causes": causes,
    });
    eprintln!(
        "{}",
        serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "{\"status\":\"failed\"}".into())
    );
    std::process::exit(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verify_manifest_accepts_only_a_strict_optional_expected_version() {
        let parsed = WorkerCli::try_parse_from([
            "rayman-update-worker",
            "verify-manifest",
            "--manifest",
            "manifest.json",
            "--signature",
            "manifest.sig",
            "--expected-version",
            "7.6.5",
        ])
        .unwrap();
        let WorkerCommand::VerifyManifest {
            expected_version, ..
        } = parsed.command
        else {
            panic!("wrong worker command")
        };
        assert_eq!(expected_version.unwrap().to_string(), "7.6.5");

        assert!(
            WorkerCli::try_parse_from([
                "rayman-update-worker",
                "verify-manifest",
                "--manifest",
                "manifest.json",
                "--signature",
                "manifest.sig",
                "--expected-version",
                "7.06.5",
            ])
            .is_err()
        );
    }

    #[test]
    fn verify_manifest_keeps_compiled_version_as_the_default() {
        let parsed = WorkerCli::try_parse_from([
            "rayman-update-worker",
            "verify-manifest",
            "--manifest",
            "manifest.json",
            "--signature",
            "manifest.sig",
        ])
        .unwrap();
        let WorkerCommand::VerifyManifest {
            expected_version, ..
        } = parsed.command
        else {
            panic!("wrong worker command")
        };
        assert!(expected_version.is_none());
    }
}
