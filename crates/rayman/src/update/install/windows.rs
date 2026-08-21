use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::ptr::{null, null_mut};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use serde::Serialize;
use sha2::{Digest, Sha256};
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, WAIT_ABANDONED, WAIT_OBJECT_0};
use windows_sys::Win32::System::Com::CoTaskMemFree;
use windows_sys::Win32::System::Threading::{
    CreateMutexW, INFINITE, ReleaseMutex, WaitForSingleObject,
};
use windows_sys::Win32::UI::Shell::{FOLDERID_ProgramFiles, SHGetKnownFolderPath};

use super::VerifiedBundle;
use crate::state_paths::{WindowsDirectoryChildIdentity, WindowsDirectoryObjectGuard};
use crate::update::ReleaseVersion;
use crate::update::state::{
    self, InstallReceipt, InstallSource, InstalledResource, SignedReleaseReceipt, WorkerRequest,
};
use crate::update::trust::AssetRole;

const TRANSACTIONS_RELATIVE_DIR: &str = "Rayman/update/transactions";
const APPLY_PLAN_SCHEMA_VERSION: u32 = 1;

pub(super) struct UpdateMutex {
    handle: HANDLE,
}

impl UpdateMutex {
    pub(super) fn acquire(installation_id: &str) -> Result<Self> {
        let name = format!("Local\\RaymanCodingSkillUpdate-{installation_id}");
        let wide = name
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        let handle = unsafe { CreateMutexW(null(), 0, wide.as_ptr()) };
        if handle.is_null() {
            return Err(std::io::Error::last_os_error())
                .context("cannot create the Rayman update transaction mutex");
        }
        let wait = unsafe { WaitForSingleObject(handle, INFINITE) };
        if !matches!(wait, WAIT_OBJECT_0 | WAIT_ABANDONED) {
            unsafe {
                CloseHandle(handle);
            }
            bail!("cannot acquire the Rayman update transaction mutex: wait={wait}");
        }
        Ok(Self { handle })
    }
}

impl Drop for UpdateMutex {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe {
                ReleaseMutex(self.handle);
                CloseHandle(self.handle);
            }
        }
    }
}

pub(super) struct AppliedUpdate {
    pub(super) version: ReleaseVersion,
    pub(super) recovered_old: bool,
    pub(super) cleanup_warning: Option<String>,
}

#[derive(Serialize)]
struct ApplyPlan {
    schema_version: u32,
    transaction_id: String,
    candidate_version: ReleaseVersion,
    cli_contract: String,
    installation_id: String,
    manifest_sha256: String,
    bundle_root: PathBuf,
    journal_path: PathBuf,
    result_path: PathBuf,
    files: Vec<ApplyFile>,
}

#[derive(Serialize)]
struct ApplyFile {
    role: String,
    source: PathBuf,
    destination: PathBuf,
    new_sha256: String,
    expected_current_sha256: Option<String>,
    expect_absent: bool,
    allow_existing_new: bool,
}

pub(super) fn apply_verified_bundle(
    bundle: &VerifiedBundle,
    receipt: &InstallReceipt,
    request: &WorkerRequest,
    now: DateTime<Utc>,
    recovery: bool,
) -> Result<AppliedUpdate> {
    let user_root = state::user_data_root()?;
    let transactions = crate::state_paths::managed_external_dir(
        &user_root,
        Path::new(TRANSACTIONS_RELATIVE_DIR),
        true,
    )?
    .expect("create=true returns the transaction root");
    let root_guard = crate::state_paths::hold_windows_directory_object(
        &transactions,
        "Rayman update transaction root",
    )?;
    let transaction_name = OsStr::new(&request.request_id);
    let (transaction, existing) =
        match root_guard.create_child_exclusive(transaction_name, "Rayman update transaction") {
            Ok(transaction) => (transaction, false),
            Err(error) if is_already_exists(&error) => {
                let guard = root_guard.open_or_create_direct_child(
                    transaction_name,
                    "existing Rayman update transaction",
                )?;
                let transaction = guard.child_identity();
                drop(guard);
                (transaction, true)
            }
            Err(error) => return Err(error),
        };
    root_guard.probe_direct_child(&transaction, transaction_name, "Rayman update transaction")?;

    if recovery && !existing {
        root_guard.remove_relative_tree_verified_with_snapshot(
            Path::new(&request.request_id),
            |_, _| {
                root_guard.verify_direct_child(
                    &transaction,
                    transaction_name,
                    "empty update intent recovery",
                )
            },
            |_, _| {
                root_guard.verify_direct_child(
                    &transaction,
                    transaction_name,
                    "empty update intent recovery snapshot",
                )
            },
        )?;
        return Ok(AppliedUpdate {
            version: request.prior_version.clone(),
            recovered_old: true,
            cleanup_warning: Some(
                "recovery intent had no publication transaction and was cancelled".into(),
            ),
        });
    }

    if existing {
        return recover_existing_transaction(
            bundle,
            request,
            &root_guard,
            &transaction,
            transaction_name,
        );
    }

    let mut sources = BTreeMap::new();
    for role in AssetRole::ALL {
        let asset = bundle.asset(role);
        let path = write_transaction_file(
            &root_guard,
            &transaction,
            transaction_name,
            OsStr::new(role.expected_name()),
            asset.bytes(),
            "verified update asset",
        )?;
        sources.insert(role, path);
    }

    let worker_destination = receipt
        .cli_path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("installed CLI has no parent directory"))?
        .join(format!(
            "rayman-update-worker-{}.exe",
            bundle.manifest().manifest().version
        ));
    let worker_expected = expected_existing_new_or_absent(
        &worker_destination,
        bundle.asset(AssetRole::UpdateWorker).sha256(),
    )?;

    let new_receipt = InstallReceipt {
        schema_version: state::INSTALL_RECEIPT_SCHEMA_VERSION,
        installation_id: receipt.installation_id.clone(),
        version: bundle.manifest().manifest().version.clone(),
        cli_contract: bundle.manifest().manifest().cli_contract.clone(),
        cli_path: receipt.cli_path.clone(),
        cli_sha256: bundle.asset(AssetRole::Cli).sha256().into(),
        worker_path: worker_destination.clone(),
        worker_sha256: bundle.asset(AssetRole::UpdateWorker).sha256().into(),
        skill_root: receipt.skill_root.clone(),
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
            sha256: bundle.asset(role).sha256().into(),
        })
        .collect(),
        install_manifest_sha256: bundle.manifest().manifest().install_manifest_sha256.clone(),
        installed_at: now,
        source: InstallSource::SignedRelease,
        signed_release: Some(SignedReleaseReceipt {
            manifest_sha256: bundle.manifest().sha256().into(),
            key_epoch: bundle.manifest().manifest().key_epoch,
            sequence: bundle.manifest().manifest().sequence,
        }),
    };
    new_receipt.validate()?;
    let receipt_bytes = serde_json::to_vec_pretty(&new_receipt)?;
    let new_receipt_hash = sha256_bytes(&receipt_bytes);
    let receipt_source = write_transaction_file(
        &root_guard,
        &transaction,
        transaction_name,
        OsStr::new("new-install-receipt.json"),
        &receipt_bytes,
        "new install receipt",
    )?;
    let receipt_destination =
        state::install_receipt_path(true)?.expect("installed receipt directory already exists");

    let old_resource_hash = |role| {
        receipt
            .resources
            .iter()
            .find(|resource| resource.role == role)
            .map(|resource| resource.sha256.clone())
            .ok_or_else(|| anyhow::anyhow!("installed receipt is missing resource {role:?}"))
    };
    let mut files = vec![
        ApplyFile {
            role: "skill".into(),
            source: sources[&AssetRole::Skill].clone(),
            destination: receipt.skill_root.join("SKILL.md"),
            new_sha256: bundle.asset(AssetRole::Skill).sha256().into(),
            expected_current_sha256: Some(old_resource_hash(AssetRole::Skill)?),
            expect_absent: false,
            allow_existing_new: false,
        },
        ApplyFile {
            role: "agent_contract".into(),
            source: sources[&AssetRole::AgentContract].clone(),
            destination: receipt.skill_root.join("AGENTS.md"),
            new_sha256: bundle.asset(AssetRole::AgentContract).sha256().into(),
            expected_current_sha256: Some(old_resource_hash(AssetRole::AgentContract)?),
            expect_absent: false,
            allow_existing_new: false,
        },
        ApplyFile {
            role: "workflow_contract".into(),
            source: sources[&AssetRole::WorkflowContract].clone(),
            destination: receipt
                .skill_root
                .join("references")
                .join("workflow-contract.md"),
            new_sha256: bundle.asset(AssetRole::WorkflowContract).sha256().into(),
            expected_current_sha256: Some(old_resource_hash(AssetRole::WorkflowContract)?),
            expect_absent: false,
            allow_existing_new: false,
        },
        ApplyFile {
            role: "update_worker".into(),
            source: sources[&AssetRole::UpdateWorker].clone(),
            destination: worker_destination,
            new_sha256: bundle.asset(AssetRole::UpdateWorker).sha256().into(),
            expected_current_sha256: worker_expected.clone(),
            expect_absent: worker_expected.is_none(),
            allow_existing_new: true,
        },
        // The main CLI is published after every supporting file. The current
        // process is the versioned worker, so no running-image lock remains.
        ApplyFile {
            role: "cli".into(),
            source: sources[&AssetRole::Cli].clone(),
            destination: receipt.cli_path.clone(),
            new_sha256: bundle.asset(AssetRole::Cli).sha256().into(),
            expected_current_sha256: Some(receipt.cli_sha256.clone()),
            expect_absent: false,
            allow_existing_new: false,
        },
        ApplyFile {
            role: "install_receipt".into(),
            source: receipt_source,
            destination: receipt_destination.clone(),
            new_sha256: new_receipt_hash,
            expected_current_sha256: Some(state::load_install_receipt()?.unwrap().1),
            expect_absent: false,
            allow_existing_new: false,
        },
    ];
    // The remote installer script is not a destination. It is fed from the
    // already verified in-memory bytes to a fixed Program Files pwsh process.
    let transaction_root = transaction.path().to_path_buf();
    let plan_path = transaction_root.join("apply-plan.json");
    let plan = ApplyPlan {
        schema_version: APPLY_PLAN_SCHEMA_VERSION,
        transaction_id: request.request_id.clone(),
        candidate_version: bundle.manifest().manifest().version.clone(),
        cli_contract: bundle.manifest().manifest().cli_contract.clone(),
        installation_id: receipt.installation_id.clone(),
        manifest_sha256: bundle.manifest().sha256().into(),
        bundle_root: transaction_root.clone(),
        journal_path: transaction_root.join("journal.json"),
        result_path: transaction_root.join("result.json"),
        files: std::mem::take(&mut files),
    };
    let plan_bytes = serde_json::to_vec_pretty(&plan)?;
    let plan_sha256 = sha256_bytes(&plan_bytes);
    let written_plan = write_transaction_file(
        &root_guard,
        &transaction,
        transaction_name,
        OsStr::new("apply-plan.json"),
        &plan_bytes,
        "verified update apply plan",
    )?;
    debug_assert_eq!(written_plan, plan_path);
    root_guard.verify_direct_child(
        &transaction,
        transaction_name,
        "Rayman update transaction before worker launch",
    )?;

    run_installer_script_from_memory(
        bundle.asset(AssetRole::InstallerScript).bytes(),
        &plan_path,
        &plan_sha256,
        &plan_bytes,
    )?;

    let (installed, _) = state::load_install_receipt()?
        .ok_or_else(|| anyhow::anyhow!("update worker completed without an install receipt"))?;
    if installed != new_receipt {
        bail!("update worker result receipt does not match the verified bundle");
    }
    verify_installed_receipt_files(&installed)?;

    Ok(AppliedUpdate {
        version: installed.version,
        recovered_old: false,
        // Keep the committed journal until an explicitly reviewed retention
        // policy removes it. If the worker dies before active.json is marked
        // committed, the next invocation can verify this exact generation.
        cleanup_warning: Some(format!(
            "committed update transaction evidence retained at {}",
            transaction_root.display()
        )),
    })
}

fn recover_existing_transaction(
    bundle: &VerifiedBundle,
    request: &WorkerRequest,
    root: &WindowsDirectoryObjectGuard,
    transaction: &WindowsDirectoryChildIdentity,
    transaction_name: &OsStr,
) -> Result<AppliedUpdate> {
    root.verify_direct_child(
        transaction,
        transaction_name,
        "existing update transaction recovery",
    )?;
    let transaction_root = transaction.path();
    let journal_path = transaction_root.join("journal.json");
    match std::fs::symlink_metadata(&journal_path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            root.remove_relative_tree_verified_with_snapshot(
                Path::new(&request.request_id),
                |_, _| {
                    root.verify_direct_child(
                        transaction,
                        transaction_name,
                        "staged-only update cancellation",
                    )
                },
                |_, _| {
                    root.verify_direct_child(
                        transaction,
                        transaction_name,
                        "staged-only update cancellation snapshot",
                    )
                },
            )?;
            return Ok(AppliedUpdate {
                version: request.prior_version.clone(),
                recovered_old: true,
                cleanup_warning: Some(
                    "interrupted update had no publication journal and was cancelled".into(),
                ),
            });
        }
        Ok(metadata) if crate::file_io::is_link_or_reparse(&metadata) || !metadata.is_file() => {
            bail!(
                "recovery journal is not an ordinary file: {}",
                journal_path.display()
            )
        }
        Ok(_) => {}
        Err(error) => return Err(error).context("cannot inspect the recovery journal"),
    }
    for role in AssetRole::ALL {
        let path = transaction_root.join(role.expected_name());
        let (actual, size) =
            state::bound_file_sha256(&path, "staged recovery asset", role.max_size())?;
        let expected = bundle.asset(role);
        if actual != expected.sha256() || size != expected.bytes().len() as u64 {
            bail!("staged recovery asset no longer matches the verified bundle: {role:?}");
        }
    }
    let plan_path = transaction_root.join("apply-plan.json");
    let (plan_sha256, _) =
        state::bound_file_sha256(&plan_path, "staged recovery apply plan", 512 * 1024)?;
    let plan_bytes =
        std::fs::read(&plan_path).context("cannot read the staged recovery apply plan")?;
    let plan: serde_json::Value = serde_json::from_slice(&plan_bytes)
        .context("cannot parse the staged recovery apply plan")?;
    if plan["schema_version"] != APPLY_PLAN_SCHEMA_VERSION
        || plan["transaction_id"] != request.request_id
        || plan["candidate_version"] != request.candidate.to_string()
        || plan["installation_id"] != request.installation_id
        || plan["manifest_sha256"] != bundle.manifest().sha256()
    {
        bail!("staged recovery plan does not match the verified request/manifest");
    }
    root.verify_direct_child(
        transaction,
        transaction_name,
        "existing update transaction before recovery script",
    )?;

    let script_result = run_installer_script_from_memory(
        bundle.asset(AssetRole::InstallerScript).bytes(),
        &plan_path,
        &plan_sha256,
        &plan_bytes,
    );
    if let Err(error) = script_result {
        let journal: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&journal_path).with_context(|| {
                format!("cannot read recovery journal after failure: {error:#}")
            })?)
            .context("cannot parse recovery journal after installer failure")?;
        if journal["phase"] == "rolled_back" && journal["committed"] == false {
            let (restored, _) = state::load_install_receipt()?
                .ok_or_else(|| anyhow::anyhow!("rollback completed without restoring a receipt"))?;
            if restored.installation_id != request.installation_id
                || restored.version != request.prior_version
            {
                bail!("rollback journal completed but the prior install receipt was not restored");
            }
            verify_installed_receipt_files(&restored)?;
            return Ok(AppliedUpdate {
                version: restored.version,
                recovered_old: true,
                cleanup_warning: Some(format!(
                    "interrupted update was rolled back; transaction evidence retained at {}",
                    transaction_root.display()
                )),
            });
        }
        return Err(error);
    }

    let (installed, _) = state::load_install_receipt()?
        .ok_or_else(|| anyhow::anyhow!("recovered update has no install receipt"))?;
    if installed.installation_id != request.installation_id
        || installed.version != request.candidate
        || installed
            .signed_release
            .as_ref()
            .map(|signed| signed.manifest_sha256.as_str())
            != Some(bundle.manifest().sha256())
    {
        bail!("committed recovery receipt does not match the verified manifest");
    }
    verify_installed_receipt_files(&installed)?;
    Ok(AppliedUpdate {
        version: installed.version,
        recovered_old: false,
        cleanup_warning: Some(format!(
            "committed update was recovered; transaction cleanup is deferred at {}",
            transaction_root.display()
        )),
    })
}

fn is_already_exists(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause.downcast_ref::<std::io::Error>().is_some_and(|error| {
            error.kind() == std::io::ErrorKind::AlreadyExists
                || matches!(error.raw_os_error(), Some(80 | 183))
        })
    })
}

fn write_transaction_file(
    root: &WindowsDirectoryObjectGuard,
    transaction: &WindowsDirectoryChildIdentity,
    transaction_name: &OsStr,
    file_name: &OsStr,
    bytes: &[u8],
    label: &str,
) -> Result<PathBuf> {
    let guard = root.open_verified_direct_child(transaction, transaction_name, label)?;
    let path = guard.write_file_exclusive(file_name, bytes, label)?;
    drop(guard);
    root.verify_direct_child(transaction, transaction_name, label)?;
    Ok(path)
}

fn expected_existing_new_or_absent(path: &Path, new_hash: &str) -> Result<Option<String>> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if crate::file_io::is_link_or_reparse(&metadata) || !metadata.is_file() => {
            bail!(
                "candidate worker destination is not an ordinary file: {}",
                path.display()
            )
        }
        Ok(_) => {
            let (actual, _) = state::bound_file_sha256(
                path,
                "candidate versioned update worker",
                AssetRole::UpdateWorker.max_size(),
            )?;
            if actual != new_hash {
                bail!(
                    "candidate worker path is occupied by different bytes: {}",
                    path.display()
                );
            }
            Ok(Some(actual))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| {
            format!(
                "cannot inspect candidate update worker path: {}",
                path.display()
            )
        }),
    }
}

fn run_installer_script_from_memory(
    script: &[u8],
    plan_path: &Path,
    plan_sha256: &str,
    plan_bytes: &[u8],
) -> Result<()> {
    std::str::from_utf8(script).context("verified installer script is not UTF-8")?;
    let plan_json = std::str::from_utf8(plan_bytes).context("verified apply plan is not UTF-8")?;
    if plan_json.encode_utf16().count() > 24_000 {
        bail!("verified apply plan exceeds the bounded worker environment contract");
    }
    let pwsh = trusted_pwsh_path()?;
    let mut child = Command::new(&pwsh)
        .args(["-NoProfile", "-NonInteractive", "-Command", "-"])
        .env("RAYMAN_UPDATE_WORKER_PLAN", plan_path)
        .env("RAYMAN_UPDATE_WORKER_PLAN_SHA256", plan_sha256)
        .env("RAYMAN_UPDATE_WORKER_PLAN_JSON", plan_json)
        .env("RAYMAN_UPDATE_WORKER", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("cannot start trusted PowerShell host: {}", pwsh.display()))?;
    child
        .stdin
        .take()
        .ok_or_else(|| anyhow::anyhow!("trusted PowerShell host has no stdin"))?
        .write_all(script)
        .context("cannot send verified installer script to trusted PowerShell stdin")?;
    let output = child
        .wait_with_output()
        .context("cannot wait for the verified update installer")?;
    if !output.status.success() {
        bail!(
            "verified update installer failed with {}: stdout={} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

fn trusted_pwsh_path() -> Result<PathBuf> {
    let mut raw = null_mut();
    let status = unsafe { SHGetKnownFolderPath(&FOLDERID_ProgramFiles, 0, null_mut(), &mut raw) };
    if status < 0 || raw.is_null() {
        bail!("cannot resolve the Windows Program Files known folder");
    }
    let length = (0..32768)
        .find(|index| unsafe { *raw.add(*index) } == 0)
        .ok_or_else(|| anyhow::anyhow!("Program Files known-folder path is unterminated"))?;
    let text = String::from_utf16(unsafe { std::slice::from_raw_parts(raw, length) });
    unsafe {
        CoTaskMemFree(raw.cast());
    }
    let text = text.context("Program Files known-folder path is invalid UTF-16")?;
    let path = PathBuf::from(text).join("PowerShell/7/pwsh.exe");
    let metadata = std::fs::symlink_metadata(&path)
        .with_context(|| format!("trusted PowerShell host is missing: {}", path.display()))?;
    if crate::file_io::is_link_or_reparse(&metadata) || !metadata.is_file() {
        bail!(
            "trusted PowerShell host is not an ordinary file: {}",
            path.display()
        );
    }
    let _ = state::bound_file_sha256(&path, "trusted PowerShell host", 256 * 1024 * 1024)?;
    Ok(path)
}

fn verify_installed_receipt_files(receipt: &InstallReceipt) -> Result<()> {
    verify_hash(
        &receipt.cli_path,
        &receipt.cli_sha256,
        AssetRole::Cli.max_size(),
    )?;
    verify_hash(
        &receipt.worker_path,
        &receipt.worker_sha256,
        AssetRole::UpdateWorker.max_size(),
    )?;
    for resource in &receipt.resources {
        verify_hash(
            &receipt.skill_root.join(&resource.relative_path),
            &resource.sha256,
            resource.role.max_size(),
        )?;
    }
    Ok(())
}

fn verify_hash(path: &Path, expected: &str, maximum: u64) -> Result<()> {
    let (actual, _) = state::bound_file_sha256(path, "committed update destination", maximum)?;
    if actual != expected {
        bail!(
            "committed update destination hash mismatch: {}",
            path.display()
        );
    }
    Ok(())
}

fn sha256_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
