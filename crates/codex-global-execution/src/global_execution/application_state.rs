//! Application data writes. These preserve file-level integrity only; they
//! never convert application JSON into proof that a validation was executed.
use super::*;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

struct Lease {
    worktree_id: String,
    object: StateObject,
    expires: i64,
    _parent: super::native::SourceDirectory,
    _lock: crate::state_lock::StateLock,
}

#[derive(Default)]
pub(super) struct ApplicationState {
    leases: BTreeMap<String, Lease>,
}

fn relative(object: &StateObject) -> PathBuf {
    match object {
        StateObject::Goal { id } => PathBuf::from("goals").join(format!("{id}.json")),
        StateObject::GoalsStore => PathBuf::from("goals/.store"),
        StateObject::Pending => PathBuf::from("pending.json"),
        StateObject::ContextIndex => PathBuf::from("context/index.json"),
        StateObject::Checkpoints => {
            unreachable!("checkpoint store uses the protected backend root")
        }
    }
}

fn target(workspace: &Path, object: &StateObject) -> Result<PathBuf> {
    let root = super::native::SourceDirectory::open(workspace)?;
    let state = workspace.join(".RaymanCodingSkill");
    match std::fs::create_dir(&state) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e.into()),
    };
    let state_pin = super::native::SourceDirectory::open(&state)?;
    let path = state.join(relative(object));
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("state parent missing"))?;
    if parent != state {
        match std::fs::create_dir(parent) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.into()),
        };
        let _ = super::native::SourceDirectory::open(parent)?;
    }
    drop(state_pin);
    drop(root);
    Ok(path)
}

pub(super) fn checkpoint_path(root: &ProtectedDirectory, enrollment: &Enrollment) -> PathBuf {
    root.path().join(format!(
        "checkpoint-{}.sqlite3",
        enrollment.registration.worktree_id
    ))
}
pub(super) fn checkpoint_workspace_id(enrollment: &Enrollment) -> String {
    crate::hash::sha256_bytes(
        crate::pathfmt::display_path(&enrollment.workspace)
            .to_ascii_lowercase()
            .as_bytes(),
    )[..32]
        .to_string()
}
fn require_checkpoint_route(root: &ProtectedDirectory, enrollment: &Enrollment) -> Result<()> {
    let name = format!(
        "checkpoint-routing-{}.json",
        enrollment.registration.worktree_id
    );
    let route: serde_json::Value =
        super::decode_json(&root.read_file(&name, MAX_REQUEST_BYTES as u64)?)?;
    if route["active"] != true
        || route["registration_sha256"] != enrollment.registration.digest()?
    {
        bail!("checkpoint storage routing is inactive or changed");
    }
    Ok(())
}

fn read_transfer(
    root: &ProtectedDirectory,
    registration: &Registration,
    sha: &str,
    total: u64,
) -> Result<Vec<u8>> {
    if !is_sha256(sha) || total > 512 * 1024 * 1024 {
        bail!("invalid stored transfer identity or length");
    }
    let mut bytes = Vec::with_capacity(total as usize);
    for index in 0..total.div_ceil(65536) {
        let name = format!("blob-{}-{sha}-{index:04x}", registration.worktree_id);
        bytes.extend(root.read_file(&name, 65536)?);
    }
    if bytes.len() as u64 != total || crate::hash::sha256_bytes(&bytes) != sha {
        bail!("stored transfer identity mismatch");
    }
    Ok(bytes)
}

impl ApplicationState {
    pub(super) fn process(
        &mut self,
        root: &ProtectedDirectory,
        enrollment: &Enrollment,
        action: &StorageAction,
        now: i64,
    ) -> Result<serde_json::Value> {
        self.leases.retain(|_, lease| lease.expires > now);
        let registration = &enrollment.registration;
        let source = super::native::SourceDirectory::open(&enrollment.workspace)?;
        if source.identity() != registration.root_identity {
            bail!("application workspace identity changed");
        }
        match action {
            StorageAction::CheckpointApply {
                lease_id,
                transaction_id,
                patch_sha256,
                total_bytes,
            } => {
                require_checkpoint_route(root, enrollment)?;
                let lease = self
                    .leases
                    .get(lease_id)
                    .ok_or_else(|| anyhow::anyhow!("checkpoint lease expired or missing"))?;
                if lease.worktree_id != registration.worktree_id
                    || lease.object != StateObject::Checkpoints
                {
                    bail!("lease does not cover this checkpoint store");
                }
                let patch = read_transfer(root, registration, patch_sha256, *total_bytes)?;
                let changes: Vec<checkpoint_store::RowChange> = serde_json::from_slice(&patch)?;
                if changes.len() > 10000 {
                    bail!("checkpoint patch row limit exceeded");
                }
                let mut blob_sizes = BTreeMap::new();
                for change in &changes {
                    if change.table == "workspaces"
                        && let Some(after) = &change.after
                        && !matches!(after.get(1), Some(checkpoint_store::Cell::Text{value}) if value == &crate::pathfmt::display_path(&enrollment.workspace))
                    {
                        bail!("checkpoint canonical path differs from enrollment");
                    }
                    for cell in change.after.iter().flatten() {
                        if let checkpoint_store::Cell::Blob { sha256, size } = cell {
                            if !is_sha256(sha256) || *size > 512 * 1024 * 1024 {
                                bail!("checkpoint blob exceeds identity or size bounds");
                            }
                            if blob_sizes
                                .insert(sha256.clone(), *size)
                                .is_some_and(|old| old != *size)
                            {
                                bail!("checkpoint blob sizes disagree");
                            }
                        }
                    }
                }
                let sum = blob_sizes
                    .values()
                    .try_fold(0u64, |sum, size| sum.checked_add(*size))
                    .ok_or_else(|| anyhow::anyhow!("checkpoint blob size overflow"))?;
                if sum > 1024 * 1024 * 1024 {
                    bail!("checkpoint transaction blob budget exceeded");
                }
                let blobs = blob_sizes
                    .into_iter()
                    .map(|(sha, size)| {
                        Ok((sha.clone(), read_transfer(root, registration, &sha, size)?))
                    })
                    .collect::<Result<BTreeMap<_, _>>>()?;
                let name = format!("checkpoint-{}.sqlite3", registration.worktree_id);
                // Protected ownership is checked before SQLite receives the fixed path.
                let _database_pin = root.pin_data_file(&name)?;
                let mut connection = rusqlite::Connection::open_with_flags(
                    checkpoint_path(root, enrollment),
                    rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE,
                )?;
                connection.busy_timeout(std::time::Duration::from_secs(5))?;
                connection.pragma_update(None, "journal_mode", "DELETE")?;
                connection.pragma_update(None, "synchronous", "FULL")?;
                let workspace_id = checkpoint_workspace_id(enrollment);
                let digest =
                    crate::hash::sha256_bytes(&serde_json::to_vec(&(&workspace_id, patch_sha256))?);
                let replayed = checkpoint_store::apply_once(
                    &mut connection,
                    &workspace_id,
                    &changes,
                    &blobs,
                    transaction_id,
                    &digest,
                )?;
                Ok(
                    serde_json::json!({"applied":true,"transaction_id":transaction_id,"patch_sha256":patch_sha256,"replayed":replayed}),
                )
            }
            StorageAction::Acquire { object } => {
                if self.leases.len() >= 256 {
                    bail!("application lease limit reached");
                }
                if self.leases.values().any(|lease| {
                    lease.worktree_id == registration.worktree_id && lease.object == *object
                }) {
                    bail!("application state is busy");
                }
                let path = if matches!(object, StateObject::Checkpoints) {
                    require_checkpoint_route(root, enrollment)?;
                    let path = checkpoint_path(root, enrollment);
                    if !path.is_file() {
                        bail!("checkpoint store is not migrated; owner enrollment is required");
                    }
                    path
                } else {
                    target(&enrollment.workspace, object)?
                };
                let parent = super::native::SourceDirectory::open(path.parent().unwrap())?;
                let lock = crate::state_lock::acquire_state_lock(&path)?;
                let mut random = [0u8; 16];
                getrandom::fill(&mut random)
                    .map_err(|e| anyhow::anyhow!("lease entropy failed: {e}"))?;
                let id: String = random.iter().map(|b| format!("{b:02x}")).collect();
                self.leases.insert(
                    id.clone(),
                    Lease {
                        worktree_id: registration.worktree_id.clone(),
                        object: object.clone(),
                        expires: now.saturating_add(120),
                        _parent: parent,
                        _lock: lock,
                    },
                );
                Ok(serde_json::json!({"lease_id":id,"expires_at":now.saturating_add(120)}))
            }
            StorageAction::Release { lease_id } | StorageAction::Renew { lease_id } => {
                if matches!(action, StorageAction::Release { .. })
                    && !self.leases.contains_key(lease_id)
                {
                    return Ok(serde_json::json!({"released":true,"already_absent":true}));
                }
                let lease = self
                    .leases
                    .get_mut(lease_id)
                    .ok_or_else(|| anyhow::anyhow!("application lease expired or missing"))?;
                if lease.worktree_id != registration.worktree_id {
                    bail!("state lease belongs to another worktree");
                }
                if matches!(action, StorageAction::Release { .. }) {
                    self.leases.remove(lease_id);
                    Ok(serde_json::json!({"released":true}))
                } else {
                    lease.expires = now.saturating_add(120);
                    Ok(serde_json::json!({"expires_at":lease.expires}))
                }
            }
            StorageAction::Chunk {
                object_sha256,
                index,
                total_bytes,
                hex,
            } => {
                let bytes = hex
                    .as_bytes()
                    .chunks_exact(2)
                    .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16))
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                let expected = (*total_bytes)
                    .saturating_sub(u64::from(*index) * 65536)
                    .min(65536);
                if bytes.len() as u64 != expected || expected == 0 {
                    bail!("state chunk offset/length differs");
                }
                let name = format!(
                    "blob-{}-{object_sha256}-{index:04x}",
                    registration.worktree_id
                );
                let path = root.path().join(&name);
                if path.try_exists()? {
                    if root.read_file(&name, 65536)? != bytes {
                        bail!("immutable state chunk differs");
                    }
                } else {
                    use std::io::Write;
                    let mut file = std::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(path)?;
                    file.write_all(&bytes)?;
                    file.sync_all()?;
                }
                Ok(serde_json::json!({"stored":true,"index":index}))
            }
            StorageAction::Write {
                object,
                lease_id,
                expected_sha256,
                object_sha256,
                total_bytes,
            } => {
                let lease = self
                    .leases
                    .get(lease_id)
                    .ok_or_else(|| anyhow::anyhow!("application lease expired or missing"))?;
                if lease.worktree_id != registration.worktree_id
                    || !(lease.object == *object
                        || (matches!(lease.object, StateObject::GoalsStore)
                            && matches!(object, StateObject::Goal { .. })))
                {
                    bail!("state lease does not cover this object");
                }
                let mut bytes = Vec::with_capacity(*total_bytes as usize);
                for index in 0..total_bytes.div_ceil(65536) {
                    let name = format!(
                        "blob-{}-{object_sha256}-{index:04x}",
                        registration.worktree_id
                    );
                    bytes.extend(root.read_file(&name, 65536)?);
                }
                if bytes.len() as u64 != *total_bytes
                    || crate::hash::sha256_bytes(&bytes) != *object_sha256
                {
                    bail!("state transfer digest/length mismatch");
                }
                let text = std::str::from_utf8(&bytes)?;
                let _: serde_json::Value = serde_json::from_slice(&bytes)?;
                let path = target(&enrollment.workspace, object)?;
                let parent = super::native::SourceDirectory::open(path.parent().unwrap())?;
                let current = crate::file_io::read_optional_handle_bound_file_bounded(
                    &path,
                    "application state",
                    64 * 1024 * 1024,
                )?
                .map(|(b, _)| crate::hash::sha256_bytes(&b));
                if current.as_ref() == Some(object_sha256) {
                    return Ok(
                        serde_json::json!({"written":true,"sha256":object_sha256,"already_current":true}),
                    );
                }
                if current.as_ref() != expected_sha256.as_ref() {
                    bail!("application state compare-and-swap conflict");
                }
                crate::file_io::write_atomic(&path, text)?;
                let observed =
                    parent.read_file(Path::new(path.file_name().unwrap()), 64 * 1024 * 1024)?;
                if crate::hash::sha256_bytes(&observed) != *object_sha256 {
                    bail!("application state readback differs");
                }
                Ok(serde_json::json!({"written":true,"sha256":object_sha256}))
            }
        }
    }
}
