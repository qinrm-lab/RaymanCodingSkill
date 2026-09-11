//! Protected on-disk installation and project registrations. Parsing alone
//! is not attestation: the worker opens these only through ProtectedDirectory.
use super::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Installation {
    pub schema_version: u32,
    pub installation_id: String,
    pub owner_sid: String,
    pub root_identity: String,
    pub worker_sha256: String,
    pub client_sha256: String,
    pub source_project_id: String,
}

impl Installation {
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != 1
            || !is_id(&self.installation_id)
            || !is_sha256(&self.root_identity)
            || !is_sha256(&self.worker_sha256)
            || !is_sha256(&self.client_sha256)
            || !is_id(&self.source_project_id)
            || !self.owner_sid.starts_with("S-1-5-21-")
        {
            bail!("invalid global installation manifest");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Enrollment {
    pub schema_version: u32,
    pub installation_id: String,
    pub registration: Registration,
    pub workspace: std::path::PathBuf,
    pub source_policy: SourcePolicy,
    pub git: Option<GitBinding>,
    pub commit_identity: Option<CommitIdentity>,
    #[serde(default)]
    pub install_policies: std::collections::BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitBinding {
    pub content_policy: GitContentPolicy,
    pub hook_policy: Option<GitHookPolicy>,
    pub executable: std::path::PathBuf,
    pub executable_sha256: String,
    pub git_directory: std::path::PathBuf,
    pub common_directory: std::path::PathBuf,
    pub branch_ref: String,
    pub config_sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommitIdentity {
    pub name: String,
    pub email: String,
}

/// No validation gate or executable path is selected by a state request.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum SourcePolicy {
    #[serde(rename = "global_workspace_fingerprint_v1")]
    WorkspaceFingerprintV1,
}

impl Enrollment {
    pub fn policy_digest(&self) -> Result<String> {
        Ok(crate::hash::sha256_bytes(&serde_json::to_vec(
            &serde_json::json!({
                "policy":"rayman-global-execution-policy-v1",
                "installation_id":self.installation_id,
                "workspace":self.workspace,
                "source_policy":self.source_policy,
                "git":self.git,
                "commit_identity":self.commit_identity,
                "install_policies":self.install_policies
            }),
        )?))
    }

    pub fn validate(&self, install: &Installation) -> Result<()> {
        self.registration.validate()?;
        if self.schema_version != 1
            || self.installation_id != install.installation_id
            || self.registration.owner_sid != install.owner_sid
            || !self.workspace.is_absolute()
            || self.registration.policy_sha256 != self.policy_digest()?
            || self
                .install_policies
                .keys()
                .cloned()
                .collect::<BTreeSet<_>>()
                != self.registration.capabilities.install_adapters
            || self.install_policies.values().any(|h| !is_sha256(h))
            || (self.registration.capabilities.local_commit
                && (self.git.is_none() || self.commit_identity.is_none()))
        {
            bail!("project enrollment does not match the global installation");
        }
        Ok(())
    }
}

#[cfg(windows)]
pub fn initialize(
    root: &std::path::Path,
    source_workspace: &std::path::Path,
) -> Result<Installation> {
    let sid = crate::execution_context::execution_context_probe()
        .principal_sid
        .ok_or_else(|| anyhow::anyhow!("initialization principal unavailable"))?;
    let protected = ProtectedDirectory::open(root, &sid)?;
    let _lock = crate::state_lock::acquire_state_lock(&protected.path().join("installation"))?;
    if protected.path().join("installation.json").try_exists()? {
        bail!("global installation already exists; use its checked upgrade route");
    }
    if std::fs::canonicalize(std::env::current_exe()?)? != protected.path().join("worker.exe") {
        bail!("initialization must run the candidate worker in its protected root");
    }
    let worker = protected.read_file("worker.exe", 128 * 1024 * 1024)?;
    let client = protected.read_file("client.exe", 128 * 1024 * 1024)?;
    let mut nonce = [0u8; 16];
    getrandom::fill(&mut nonce)
        .map_err(|e| anyhow::anyhow!("initialization entropy unavailable: {e}"))?;
    let source = super::native::SourceDirectory::open(source_workspace)?;
    let git_marker = source.path().join(".git");
    let git = if git_marker.is_dir() {
        git_marker
    } else {
        let bytes = source.read_file(std::path::Path::new(".git"), 4096)?;
        source.path().join(
            std::str::from_utf8(&bytes)?
                .trim()
                .strip_prefix("gitdir: ")
                .ok_or_else(|| anyhow::anyhow!("source Git marker is invalid"))?,
        )
    };
    let git = super::native::SourceDirectory::open(&std::fs::canonicalize(git)?)?;
    let common = if git.path().join("commondir").try_exists()? {
        let bytes = git.read_file(std::path::Path::new("commondir"), 4096)?;
        super::native::SourceDirectory::open(&std::fs::canonicalize(
            git.path().join(std::str::from_utf8(&bytes)?.trim()),
        )?)?
    } else {
        super::native::SourceDirectory::open(git.path())?
    };
    let source_project_id = common.identity()[..32].to_owned();
    let install = Installation {
        schema_version: 1,
        source_project_id,
        installation_id: nonce.iter().map(|b| format!("{b:02x}")).collect(),
        owner_sid: sid,
        root_identity: protected.identity().into(),
        worker_sha256: crate::hash::sha256_bytes(&worker),
        client_sha256: crate::hash::sha256_bytes(&client),
    };
    install.validate()?;
    crate::file_io::write_json(&protected.path().join("installation.json"), &install)?;
    Ok(install)
}

#[cfg(windows)]
pub fn enroll(
    root: &std::path::Path,
    workspace: &std::path::Path,
    commit: Option<(&std::path::Path, &str, &str)>,
    formal_state: bool,
    publish: bool,
) -> Result<Enrollment> {
    use std::path::Path;
    let sid = crate::execution_context::execution_context_probe()
        .principal_sid
        .ok_or_else(|| anyhow::anyhow!("enrollment principal unavailable"))?;
    let protected = ProtectedDirectory::open(root, &sid)?;
    let installation: Installation =
        super::decode_json(&protected.read_file("installation.json", MAX_REQUEST_BYTES as u64)?)?;
    installation.validate()?;
    if installation.owner_sid != sid || installation.root_identity != protected.identity() {
        bail!("enrollment installation identity changed");
    }
    if crate::hash::sha256_bytes(&protected.read_file("worker.exe", 128 * 1024 * 1024)?)
        != installation.worker_sha256
    {
        bail!("enrollment worker identity changed");
    }
    let _lock = crate::state_lock::acquire_state_lock(&protected.path().join("registry"))?;
    let source = super::native::SourceDirectory::open(workspace)?;
    let marker = source.path().join(".git");
    let (git, common, branch) = if marker.try_exists()? {
        let path = if std::fs::symlink_metadata(&marker)?.is_dir() {
            marker
        } else {
            let text = source.read_file(Path::new(".git"), 4096)?;
            source.path().join(
                std::str::from_utf8(&text)?
                    .trim()
                    .strip_prefix("gitdir: ")
                    .ok_or_else(|| anyhow::anyhow!("invalid worktree marker"))?,
            )
        };
        let git = std::fs::canonicalize(path)?;
        let pin = super::native::SourceDirectory::open(&git)?;
        let common = if git.join("commondir").try_exists()? {
            let bytes = pin.read_file(Path::new("commondir"), 4096)?;
            std::fs::canonicalize(git.join(std::str::from_utf8(&bytes)?.trim()))?
        } else {
            git.clone()
        };
        let head = pin.read_file(Path::new("HEAD"), 4096)?;
        let branch = std::str::from_utf8(&head)?
            .trim()
            .strip_prefix("ref: ")
            .map(str::to_owned);
        (Some(git), Some(common), branch)
    } else {
        (None, None, None)
    };
    let common_identity = if let Some(common) = &common {
        super::native::SourceDirectory::open(common)?
            .identity()
            .to_owned()
    } else {
        source.identity().into()
    };
    let worktree_identity = if let Some(git) = &git {
        super::native::SourceDirectory::open(git)?
            .identity()
            .to_owned()
    } else {
        source.identity().into()
    };
    let worktree_id =
        crate::hash::sha256_bytes(format!("{}:{worktree_identity}", source.identity()).as_bytes())
            [..32]
            .to_owned();
    let mut registration = Registration {
        schema_version: 1,
        project_id: common_identity[..32].into(),
        worktree_id,
        owner_sid: sid,
        root_identity: source.identity().into(),
        git_common_identity: common_identity,
        object_id_length: 40,
        policy_sha256: crate::hash::sha256_bytes(b"rayman-global-execution-policy-v1"),
        capabilities: Capabilities {
            local_commit: commit.is_some(),
            added_files: commit.is_some(),
            deleted_files: commit.is_some(),
            formal_state,
            install_adapters: BTreeSet::new(),
        },
    };
    let (binding, identity) = if let Some((program, name, email)) = commit {
        let git = git.ok_or_else(|| anyhow::anyhow!("local commits require a Git worktree"))?;
        let common = common.ok_or_else(|| anyhow::anyhow!("Git common directory missing"))?;
        let branch =
            branch.ok_or_else(|| anyhow::anyhow!("enroll an attached branch for local commits"))?;
        validate_relative_path(&branch)?;
        let common_pin = super::native::SourceDirectory::open(&common)?;
        let oid = common_pin.read_file(Path::new(&branch), 4096)?;
        let oid = std::str::from_utf8(&oid)?.trim();
        if !is_hex(oid, 40) && !is_hex(oid, 64) {
            bail!("enrollment needs a valid existing loose branch ref");
        }
        registration.object_id_length = oid.len();
        for value in [name, email] {
            if value.is_empty() || value.len() > 200 || value.contains(['<', '>', '\n', '\r', '\0'])
            {
                bail!("invalid enrolled commit identity");
            }
        }
        let binding = GitBinding {
            content_policy: GitContentPolicy::capture(program, source.path())?,
            hook_policy: GitHookPolicy::capture(program, source.path())?,
            executable: std::fs::canonicalize(program)?,
            executable_sha256: crate::hash::sha256_file(program)?,
            git_directory: git,
            common_directory: common.clone(),
            branch_ref: branch,
            config_sha256: crate::hash::sha256_bytes(
                &common_pin.read_file(Path::new("config"), 1024 * 1024)?,
            ),
        };
        let inspector = GitInspector::open(source.path(), &binding, &registration, &protected)?;
        let _ = inspector.capture(&registration)?;
        (
            Some(binding),
            Some(CommitIdentity {
                name: name.into(),
                email: email.into(),
            }),
        )
    } else {
        (None, None)
    };
    let mut enrollment = Enrollment {
        schema_version: 1,
        installation_id: installation.installation_id.clone(),
        registration,
        workspace: source.path().to_path_buf(),
        source_policy: SourcePolicy::WorkspaceFingerprintV1,
        git: binding,
        commit_identity: identity,
        install_policies: Default::default(),
    };
    enrollment.registration.policy_sha256 = enrollment.policy_digest()?;
    enrollment.validate(&installation)?;
    let target = protected.path().join(format!(
        "project-{}.json",
        enrollment.registration.worktree_id
    ));
    if target.try_exists()? {
        let old: Enrollment = super::decode_json(&protected.read_file(
            target.file_name().unwrap().to_str().unwrap(),
            MAX_REQUEST_BYTES as u64,
        )?)?;
        old.validate(&installation)?;
        // Enrollment does not request revocation of owner-registered adapters.
        // Preserve that authenticated extension, then compare every other field.
        enrollment.install_policies = old.install_policies.clone();
        enrollment.registration.capabilities.install_adapters =
            old.registration.capabilities.install_adapters.clone();
        enrollment.registration.policy_sha256 = enrollment.policy_digest()?;
        enrollment.validate(&installation)?;
        if serde_json::to_vec(&old)? != serde_json::to_vec(&enrollment)? {
            bail!("project already enrolled with different policy; explicit migration required");
        }
        return Ok(old);
    }
    if !publish {
        return Ok(enrollment);
    }
    crate::file_io::write_json(&target, &enrollment)?;
    Ok(enrollment)
}

#[cfg(windows)]
pub fn enable_application_bridge(
    root: &std::path::Path,
    workspace: &std::path::Path,
) -> Result<serde_json::Value> {
    let sid = crate::execution_context::execution_context_probe()
        .principal_sid
        .ok_or_else(|| anyhow::anyhow!("state activation owner unavailable"))?;
    let enrollment = Client::open(root)?.enrollment(workspace)?;
    if enrollment.registration.owner_sid != sid {
        bail!("state activation requires its enrolled owner");
    }
    if !enrollment.registration.capabilities.formal_state {
        bail!("formal state capability is not enrolled");
    }
    let state = enrollment.workspace.join(".RaymanCodingSkill");
    if !state.join("workspace_skill.yaml").try_exists()? {
        bail!("workspace has no active Rayman binding");
    }
    let root = super::native::SourceDirectory::open(&state)?;
    for name in ["goals", "context"] {
        let target = state.join(name);
        match std::fs::create_dir(&target) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.into()),
        };
        let _ = super::native::SourceDirectory::open(&target)?;
    }
    let marker = serde_json::json!({"schema":"rayman.global-state-routing.v1","installation_id":enrollment.installation_id,"worktree_id":enrollment.registration.worktree_id});
    let target = state.join("global-state.json");
    if target.try_exists()? {
        let bytes = root.read_file(std::path::Path::new("global-state.json"), 65536)?;
        let old: serde_json::Value = super::decode_json(&bytes)?;
        if old != marker {
            bail!("existing global state routing differs; explicit migration is required");
        }
    } else {
        crate::file_io::write_json(&target, &marker)?;
    }
    Ok(marker)
}

#[cfg(windows)]
fn checkpoint_runtime_pin(
    source: &super::native::SourceDirectory,
    config: &serde_json::Value,
    expected: &str,
) -> Result<super::native::SourceFile> {
    use std::path::Path;
    let runtime = config["runtime_exe"]
        .as_str()
        .map(std::path::PathBuf::from)
        .ok_or_else(|| anyhow::anyhow!("checkpoint runtime is not installed"))?;
    let parent = super::native::SourceDirectory::open(
        runtime
            .parent()
            .ok_or_else(|| anyhow::anyhow!("runtime parent missing"))?,
    )?;
    let expected_parent =
        super::native::SourceDirectory::open(&source.path().join(".agent-checkpoints/runtime"))?;
    if parent.identity() != expected_parent.identity()
        || config["rust_runtime_sha256"].as_str() != Some(expected)
    {
        bail!("checkpoint runtime binding differs");
    }
    let pin = parent.pin_file(Path::new(
        runtime
            .file_name()
            .ok_or_else(|| anyhow::anyhow!("runtime filename missing"))?,
    ))?;
    use sha2::{Digest, Sha256};
    let mut digest = Sha256::new();
    std::io::copy(&mut pin.file.try_clone()?, &mut digest)?;
    if format!("{:x}", digest.finalize()) != expected {
        bail!("checkpoint runtime bytes changed");
    }
    Ok(pin)
}

#[cfg(windows)]
pub fn prepare_checkpoint_migration(
    root: &std::path::Path,
    workspace: &std::path::Path,
    runtime_sha256: &str,
) -> Result<serde_json::Value> {
    use super::publication::PublicationSlot;
    use std::path::Path;
    if !is_sha256(runtime_sha256) {
        bail!("invalid checkpoint runtime hash");
    }
    let sid = crate::execution_context::execution_context_probe()
        .principal_sid
        .ok_or_else(|| anyhow::anyhow!("migration owner unavailable"))?;
    let protected = ProtectedDirectory::open(root, &sid)?;
    let client = Client::open(root)?;
    let enrollment = client.enrollment(workspace)?;
    if enrollment.registration.owner_sid != sid
        || !enrollment.registration.capabilities.formal_state
    {
        bail!("checkpoint migration requires its enrolled owner");
    }
    let _migration =
        crate::state_lock::acquire_state_lock(&protected.path().join("checkpoint-migration"))?;
    let source = super::native::SourceDirectory::open(&enrollment.workspace)?;
    let data = source.pin_file(Path::new(".agent-checkpoints/status.sqlite3"))?;
    for sidecar in [
        "status.sqlite3-journal",
        "status.sqlite3-wal",
        "status.sqlite3-shm",
    ] {
        if data.path.parent().unwrap().join(sidecar).try_exists()? {
            bail!(
                "checkpoint database has journal sidecars; application recovery is required first"
            );
        }
    }
    let database = rusqlite::Connection::open_with_flags(
        &data.path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?;
    let workspace_id = super::application_state::checkpoint_workspace_id(&enrollment);
    let config: String = database.query_row(
        "SELECT config_json FROM workspaces WHERE workspace_id=?1",
        [&workspace_id],
        |row| row.get(0),
    )?;
    let config: serde_json::Value = serde_json::from_str(&config)?;
    if config["external_mirror"] == true {
        bail!("external checkpoint mirrors require their separate migration route");
    }
    let _runtime_pin = checkpoint_runtime_pin(&source, &config, runtime_sha256)?;
    let pending:i64=database.query_row("SELECT (SELECT count(*) FROM mirror_intents)+(SELECT count(*) FROM session_intents)+(SELECT count(*) FROM workspace_locks)",[],|r|r.get(0))?;
    if pending != 0 {
        bail!(
            "checkpoint database has pending intents or a live/stale lock; recover through its application first"
        );
    }
    let mut random = [0u8; 16];
    getrandom::fill(&mut random).map_err(|e| anyhow::anyhow!("migration entropy failed: {e}"))?;
    let transaction_id: String = random.iter().map(|b| format!("{b:02x}")).collect();
    let wid = &enrollment.registration.worktree_id;
    let backup_name = format!("checkpoint-backup-{wid}-{transaction_id}.sqlite3");
    let stage_name = format!("checkpoint-stage-{wid}-{transaction_id}.sqlite3");
    let journal_name = format!("checkpoint-migration-{wid}-{transaction_id}.json");
    let backup = PublicationSlot::create_stream(
        protected.path(),
        &backup_name,
        &mut data.file.try_clone()?,
        4 * 1024 * 1024 * 1024,
    )?;
    let backup_sha = backup.digest().to_owned();
    drop(backup);
    let stage = PublicationSlot::create(protected.path(), &stage_name, b"")?;
    let stage_identity = stage.identity().to_owned();
    drop(stage);
    let mut normalized = rusqlite::Connection::open_with_flags(
        protected.path().join(&stage_name),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE,
    )?;
    let logical_sha = checkpoint_store::normalize_owned(&database, &mut normalized)?;
    drop(normalized);
    let stage_sha = crate::hash::sha256_file(&protected.path().join(&stage_name))?;
    if crate::hash::sha256_file(&protected.path().join(&backup_name))? != backup_sha {
        bail!("checkpoint backup readback changed");
    }
    let record = serde_json::json!({"schema":"rayman.checkpoint-migration.v1","state":"prepared","installation_id":enrollment.installation_id,"registration_sha256":enrollment.registration.digest()?,"workspace":enrollment.workspace,"worktree_id":wid,"transaction_id":transaction_id,"runtime_sha256":runtime_sha256,"backup_name":backup_name,"backup_sha256":backup_sha,"stage_name":stage_name,"stage_identity":stage_identity,"stage_sha256":stage_sha,"logical_sha256":logical_sha,"routing_published":false});
    crate::file_io::write_json(&protected.path().join(&journal_name), &record)?;
    // Preparation preserves the live vault and does not yet activate routing.
    Ok(record)
}

#[cfg(windows)]
fn same_checkpoint_control(left: &[u8], right: &[u8]) -> Result<bool> {
    Ok(super::decode_json::<serde_json::Value>(left)?
        == super::decode_json::<serde_json::Value>(right)?)
}

#[cfg(windows)]
fn checkpoint_marker_slot(
    state: &super::native::SourceDirectory,
    name: &str,
    bytes: &[u8],
    metadata_source: &std::path::Path,
) -> Result<super::publication::PublicationSlot> {
    use super::publication::PublicationSlot;
    let slot = if state.path().join(name).try_exists()? {
        let pin = state.pin_file(std::path::Path::new(name))?;
        let observed = crate::file_io::read_bytes_from_handle(
            &pin.file,
            pin.file.metadata()?.len(),
            &pin.path,
            "checkpoint marker recovery",
        )?;
        if !same_checkpoint_control(&observed, bytes)? {
            bail!("checkpoint marker stage has unrelated bytes");
        }
        let identity = super::publication::identity(&pin.file, &pin.path)?;
        drop(pin);
        PublicationSlot::resume(
            state.path(),
            name,
            &identity,
            &crate::hash::sha256_bytes(&observed),
        )?
    } else {
        PublicationSlot::create(state.path(), name, bytes)?
    };
    slot.preserve_access_from(metadata_source)?;
    Ok(slot)
}

/// Publish a prepared checkpoint store. The original vault remains as the
/// verified rollback baseline; no path outside the enrolled workspace/root is accepted.
#[cfg(windows)]
pub fn publish_checkpoint_migration(
    root: &std::path::Path,
    workspace: &std::path::Path,
    transaction_id: &str,
) -> Result<serde_json::Value> {
    use super::publication::PublicationSlot;
    use std::path::Path;
    if !is_id(transaction_id) {
        bail!("invalid checkpoint migration identity");
    }
    let sid = crate::execution_context::execution_context_probe()
        .principal_sid
        .ok_or_else(|| anyhow::anyhow!("migration owner unavailable"))?;
    let protected = ProtectedDirectory::open(root, &sid)?;
    let enrollment = Client::open(root)?.enrollment(workspace)?;
    if enrollment.registration.owner_sid != sid
        || !enrollment.registration.capabilities.formal_state
    {
        bail!("checkpoint publication requires its enrolled owner");
    }
    let _migration =
        crate::state_lock::acquire_state_lock(&protected.path().join("checkpoint-migration"))?;
    let wid = &enrollment.registration.worktree_id;
    let target_name = format!("checkpoint-{wid}.sqlite3");
    let _backend = crate::state_lock::acquire_state_lock(&protected.path().join(&target_name))?;
    let journal_name = format!("checkpoint-migration-{wid}-{transaction_id}.json");
    let mut record: serde_json::Value =
        super::decode_json(&protected.read_file(&journal_name, MAX_REQUEST_BYTES as u64)?)?;
    let backup_name = format!("checkpoint-backup-{wid}-{transaction_id}.sqlite3");
    let stage_name = format!("checkpoint-stage-{wid}-{transaction_id}.sqlite3");
    if record["schema"] != "rayman.checkpoint-migration.v1"
        || record["installation_id"] != enrollment.installation_id
        || record["registration_sha256"] != enrollment.registration.digest()?
        || record["worktree_id"] != *wid
        || record["transaction_id"] != transaction_id
        || record["backup_name"] != backup_name
        || record["stage_name"] != stage_name
        || !matches!(
            record["state"].as_str(),
            Some("prepared" | "publishing" | "committed")
        )
    {
        bail!("checkpoint migration record does not match enrollment");
    }
    let field = |name: &str| -> Result<String> {
        record[name]
            .as_str()
            .filter(|s| is_sha256(s))
            .map(str::to_owned)
            .ok_or_else(|| anyhow::anyhow!("invalid migration hash field: {name}"))
    };
    let original_sha = field("backup_sha256")?;
    let stage_sha = field("stage_sha256")?;
    let stage_identity = field("stage_identity")?;
    let runtime_sha = field("runtime_sha256")?;
    let source = super::native::SourceDirectory::open(&enrollment.workspace)?;
    let state_path = enrollment.workspace.join(".agent-checkpoints");
    let state = super::native::SourceDirectory::open(&state_path)?;
    let original = source.pin_file(Path::new(".agent-checkpoints/status.sqlite3"))?;
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    std::io::copy(&mut original.file.try_clone()?, &mut hasher)?;
    if format!("{:x}", hasher.finalize()) != original_sha {
        bail!("checkpoint source changed after migration preparation");
    }
    let source_database = rusqlite::Connection::open_with_flags(
        &original.path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?;
    let config: String = source_database.query_row(
        "SELECT config_json FROM workspaces WHERE workspace_id=?1",
        [super::application_state::checkpoint_workspace_id(
            &enrollment,
        )],
        |row| row.get(0),
    )?;
    let _runtime_pin =
        checkpoint_runtime_pin(&source, &serde_json::from_str(&config)?, &runtime_sha)?;
    let _backup_pin = protected.pin_data_file(&backup_name)?;
    if crate::hash::sha256_file(&protected.path().join(&backup_name))? != original_sha {
        bail!("checkpoint migration backup changed");
    }
    let marker = serde_json::json!({"schema":"rayman.global-state-routing.v1","installation_id":enrollment.installation_id,"worktree_id":wid,"transaction_id":transaction_id,"runtime_sha256":runtime_sha,"original_sha256":original_sha});
    let marker_bytes = serde_json::to_vec_pretty(&marker)?;
    let mut preparing_marker = marker.clone();
    preparing_marker["migration_state"] = serde_json::json!("preparing");
    let preparing_bytes = serde_json::to_vec_pretty(&preparing_marker)?;
    let marker_path = state.path().join("global-state.json");
    let mut transition_bytes = serde_json::to_vec_pretty(
        &serde_json::json!({"schema":"rayman.checkpoint-transition.v1","transaction_id":transaction_id,"registration_sha256":enrollment.registration.digest()?}),
    )?;
    let transition_path = state.path().join("global-state-transition.json");
    if transition_path.try_exists()? {
        let observed = state.read_file(Path::new("global-state-transition.json"), 65536)?;
        if !same_checkpoint_control(&observed, &transition_bytes)? {
            bail!("another checkpoint transition is present");
        }
        transition_bytes = observed;
    }
    let displaced_preparing = format!("global-preparing-{transaction_id}.done");

    if marker_path.try_exists()? {
        let current: serde_json::Value =
            super::decode_json(&state.read_file(Path::new("global-state.json"), 65536)?)?;
        if current != marker && current != preparing_marker {
            bail!("checkpoint routing marker differs; no state overwritten");
        }
        if current == marker {
            if !matches!(record["state"].as_str(), Some("publishing" | "committed")) {
                bail!("checkpoint marker appeared before recorded publication");
            }
            let _target_pin = protected.pin_data_file(&target_name)?;
            if super::publication::identity(&_target_pin, &protected.path().join(&target_name))?
                != stage_identity
            {
                bail!("published checkpoint object was replaced");
            }
            let db = rusqlite::Connection::open_with_flags(
                protected.path().join(&target_name),
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            )?;
            checkpoint_store::validate_schema(&db)?;
            crate::file_io::write_json(
                &protected
                    .path()
                    .join(format!("checkpoint-routing-{wid}.json")),
                &serde_json::json!({"active":true,"transaction_id":transaction_id,"registration_sha256":enrollment.registration.digest()?}),
            )?;
            record["state"] = serde_json::json!("committed");
            record["routing_published"] = serde_json::json!(true);
            crate::file_io::write_json(&protected.path().join(&journal_name), &record)?;
            if transition_path.try_exists()?
                && !state.consume_packet("global-state-transition.json", &transition_bytes)?
            {
                bail!("checkpoint transition marker changed");
            }
            return Ok(record);
        }
    }
    if record["state"] == "committed" {
        bail!("committed checkpoint routing was removed; explicit recovery required");
    }
    record["state"] = serde_json::json!("publishing");
    crate::file_io::write_json(&protected.path().join(&journal_name), &record)?;
    crate::file_io::write_json(
        &protected
            .path()
            .join(format!("checkpoint-routing-{wid}.json")),
        &serde_json::json!({"active":false,"transaction_id":transaction_id,"registration_sha256":enrollment.registration.digest()?}),
    )?;
    if !transition_path.try_exists()? {
        let name = format!("global-transition-{transaction_id}.tmp");
        let mut slot = checkpoint_marker_slot(&state, &name, &transition_bytes, &original.path)?;
        slot.rename("global-state-transition.json", false)?;
    }
    if !marker_path.try_exists()? && !state.path().join(&displaced_preparing).try_exists()? {
        let name = format!("global-preparing-{transaction_id}.tmp");
        let mut slot = checkpoint_marker_slot(&state, &name, &preparing_bytes, &original.path)?;
        slot.rename("global-state.json", false)?;
    }

    if !protected.path().join(&target_name).try_exists()? {
        let mut stage = PublicationSlot::resume_with_limit(
            protected.path(),
            &stage_name,
            &stage_identity,
            &stage_sha,
            4 * 1024 * 1024 * 1024,
        )?;
        stage.rename(&target_name, false)?;
    } else {
        // Crash recovery can recognize only the exact prepared object, never
        // replace an unrelated store just because its filename looks right.
        let _target = PublicationSlot::resume_with_limit(
            protected.path(),
            &target_name,
            &stage_identity,
            &stage_sha,
            4 * 1024 * 1024 * 1024,
        )?;
    }
    let marker_seed = format!("global-state-{transaction_id}.tmp");
    let mut slot = checkpoint_marker_slot(&state, &marker_seed, &marker_bytes, &original.path)?;
    record["marker_identity"] = serde_json::json!(slot.identity());
    record["marker_sha256"] = serde_json::json!(slot.digest());
    crate::file_io::write_json(&protected.path().join(&journal_name), &record)?;
    // Replace only the exact deny marker through its held identity. Preserve
    // it under a transaction-bound name, then publish without replacement.
    if marker_path.try_exists()? {
        let preparing = state.pin_file(Path::new("global-state.json"))?;
        let observed_preparing = state.read_file(Path::new("global-state.json"), 65536)?;
        if !same_checkpoint_control(&observed_preparing, &preparing_bytes)? {
            bail!("checkpoint preparing marker changed");
        }
        let identity = super::publication::identity(&preparing.file, &preparing.path)?;
        drop(preparing);
        let mut previous = PublicationSlot::resume(
            state.path(),
            "global-state.json",
            &identity,
            &crate::hash::sha256_bytes(&observed_preparing),
        )?;
        previous.rename(&displaced_preparing, false)?;
    } else if !same_checkpoint_control(
        &state.read_file(Path::new(&displaced_preparing), 65536)?,
        &preparing_bytes,
    )? {
        bail!("checkpoint displaced deny marker changed");
    }
    slot.rename("global-state.json", false)?;
    crate::file_io::write_json(
        &protected
            .path()
            .join(format!("checkpoint-routing-{wid}.json")),
        &serde_json::json!({"active":true,"transaction_id":transaction_id,"registration_sha256":enrollment.registration.digest()?}),
    )?;
    record["state"] = serde_json::json!("committed");
    record["routing_published"] = serde_json::json!(true);
    crate::file_io::write_json(&protected.path().join(&journal_name), &record)?;
    if !state.consume_packet("global-state-transition.json", &transition_bytes)? {
        bail!("checkpoint transition marker changed");
    }
    Ok(record)
}

/// Withdraw one opt-in route while carrying the latest global rows back to
/// the original logical vault. Both the old baseline and global store remain.
#[cfg(windows)]
pub fn rollback_checkpoint_migration(
    root: &std::path::Path,
    workspace: &std::path::Path,
    transaction_id: &str,
) -> Result<serde_json::Value> {
    use super::publication::PublicationSlot;
    use std::path::Path;
    if !is_id(transaction_id) {
        bail!("invalid migration identity");
    }
    let sid = crate::execution_context::execution_context_probe()
        .principal_sid
        .ok_or_else(|| anyhow::anyhow!("rollback owner unavailable"))?;
    let protected = ProtectedDirectory::open(root, &sid)?;
    let enrollment = Client::open(root)?.enrollment(workspace)?;
    if enrollment.registration.owner_sid != sid {
        bail!("checkpoint rollback requires its owner");
    }
    let _migration =
        crate::state_lock::acquire_state_lock(&protected.path().join("checkpoint-migration"))?;
    let wid = &enrollment.registration.worktree_id;
    let target_name = format!("checkpoint-{wid}.sqlite3");
    let _backend = crate::state_lock::acquire_state_lock(&protected.path().join(&target_name))?;
    let journal_name = format!("checkpoint-migration-{wid}-{transaction_id}.json");
    let mut record: serde_json::Value =
        super::decode_json(&protected.read_file(&journal_name, MAX_REQUEST_BYTES as u64)?)?;
    if record["schema"] != "rayman.checkpoint-migration.v1"
        || record["registration_sha256"] != enrollment.registration.digest()?
        || record["transaction_id"] != transaction_id
        || !matches!(
            record["state"].as_str(),
            Some("committed" | "rollback_preparing" | "rolling_back" | "rolled_back")
        )
    {
        bail!("checkpoint rollback record differs from enrollment");
    }
    let source = super::native::SourceDirectory::open(&enrollment.workspace)?;
    let state =
        super::native::SourceDirectory::open(&enrollment.workspace.join(".agent-checkpoints"))?;
    let marker_path = state.path().join("global-state.json");
    let marker_bytes = if marker_path.try_exists()? {
        Some(state.read_file(Path::new("global-state.json"), 65536)?)
    } else {
        None
    };
    if record["state"] == "rolled_back" {
        if marker_bytes.is_some() {
            bail!("routing was recreated after rollback; explicit migration is required");
        }
        return Ok(record);
    }
    if let Some(bytes) = &marker_bytes {
        let marker: serde_json::Value = super::decode_json(bytes)?;
        if marker["transaction_id"] != transaction_id || marker["worktree_id"] != *wid {
            bail!("checkpoint rollback marker differs");
        }
    } else if record["state"] != "rolling_back" {
        bail!("checkpoint rollback marker disappeared before publication");
    }
    let local_path = state.path().join("status.sqlite3");
    let old = if local_path.try_exists()? {
        Some(source.pin_file(Path::new(".agent-checkpoints/status.sqlite3"))?)
    } else {
        None
    };
    use sha2::{Digest, Sha256};
    let local_sha = old
        .as_ref()
        .map(|old| -> Result<String> {
            let mut hasher = Sha256::new();
            std::io::copy(&mut old.file.try_clone()?, &mut hasher)?;
            Ok(format!("{:x}", hasher.finalize()))
        })
        .transpose()?;
    let old_identity = old
        .as_ref()
        .map(|old| super::publication::identity(&old.file, &old.path))
        .transpose()?;
    let original_sha = record["backup_sha256"]
        .as_str()
        .filter(|sha| is_sha256(sha))
        .ok_or_else(|| anyhow::anyhow!("rollback baseline digest missing"))?
        .to_owned();
    if record["state"] != "rolling_back" && local_sha.as_deref() != Some(original_sha.as_str()) {
        bail!("local checkpoint vault changed; refusing rollback overwrite");
    }
    if record["state"] == "committed" {
        record["state"] = serde_json::json!("rollback_preparing");
        crate::file_io::write_json(&protected.path().join(&journal_name), &record)?;
    }
    // Disable new backend writers before taking the rollback snapshot. A crash
    // in preparation can then retry from the latest protected store safely.
    crate::file_io::write_json(
        &protected
            .path()
            .join(format!("checkpoint-routing-{wid}.json")),
        &serde_json::json!({"active":false,"transaction_id":transaction_id,"registration_sha256":enrollment.registration.digest()?}),
    )?;
    let _global_pin = protected.pin_data_file(&target_name)?;
    let global_sha = crate::hash::sha256_file(&protected.path().join(&target_name))?;
    if record["state"] == "rollback_preparing" {
        let database = rusqlite::Connection::open_with_flags(
            protected.path().join(&target_name),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?;
        let mut random = [0u8; 16];
        getrandom::fill(&mut random)
            .map_err(|e| anyhow::anyhow!("rollback entropy failed: {e}"))?;
        let attempt: String = random.iter().map(|b| format!("{b:02x}")).collect();
        let stage_name = format!("rollback-{transaction_id}-{attempt}.sqlite3");
        let seed = PublicationSlot::create(state.path(), &stage_name, b"")?;
        let identity = seed.identity().to_owned();
        drop(seed);
        let mut local = rusqlite::Connection::open_with_flags(
            state.path().join(&stage_name),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE,
        )?;
        let logical_sha = checkpoint_store::normalize_local(&database, &mut local)?;
        drop(local);
        let sha = crate::hash::sha256_file(&state.path().join(&stage_name))?;
        let slot = PublicationSlot::resume_with_limit(
            state.path(),
            &stage_name,
            &identity,
            &sha,
            4 * 1024 * 1024 * 1024,
        )?;
        slot.preserve_security_from(&old.as_ref().unwrap().path)?;
        drop(slot);
        record["state"] = serde_json::json!("rolling_back");
        record["rollback_sha256"] = serde_json::json!(sha);
        record["rollback_global_sha256"] = serde_json::json!(global_sha);
        record["rollback_attempt"] = serde_json::json!(attempt);
        record["rollback_original_identity"] = serde_json::json!(old_identity);
        record["rollback_stage_identity"] = serde_json::json!(identity);
        record["rollback_logical_sha256"] = serde_json::json!(logical_sha);
        crate::file_io::write_json(&protected.path().join(&journal_name), &record)?;
    }
    if record["rollback_global_sha256"] != global_sha {
        bail!("global checkpoint rows changed during rollback; both stores retained");
    }
    let sha = record["rollback_sha256"]
        .as_str()
        .filter(|sha| is_sha256(sha))
        .ok_or_else(|| anyhow::anyhow!("rollback digest missing"))?;
    let identity = record["rollback_stage_identity"]
        .as_str()
        .filter(|id| is_sha256(id))
        .ok_or_else(|| anyhow::anyhow!("rollback identity missing"))?;
    let attempt = record["rollback_attempt"]
        .as_str()
        .filter(|id| is_id(id))
        .ok_or_else(|| anyhow::anyhow!("rollback attempt missing"))?;
    let stage_name = format!("rollback-{transaction_id}-{attempt}.sqlite3");
    let original_identity = record["rollback_original_identity"]
        .as_str()
        .filter(|id| is_sha256(id))
        .ok_or_else(|| anyhow::anyhow!("rollback original identity missing"))?;
    let displaced_name = format!("rollback-original-{transaction_id}-{attempt}.sqlite3");
    drop(old);
    if local_sha.as_deref() == Some(sha) {
        let _published = PublicationSlot::resume_with_limit(
            state.path(),
            "status.sqlite3",
            identity,
            sha,
            4 * 1024 * 1024 * 1024,
        )?;
    } else {
        if local_sha.as_deref() == Some(original_sha.as_str()) {
            // Claim the exact old object by handle, then publish into the vacant
            // name without replacement. A racing new file is never overwritten.
            let mut previous = PublicationSlot::resume_with_limit(
                state.path(),
                "status.sqlite3",
                original_identity,
                &original_sha,
                4 * 1024 * 1024 * 1024,
            )?;
            previous.rename(&displaced_name, false)?;
        } else if local_sha.is_none() {
            let _previous = PublicationSlot::resume_with_limit(
                state.path(),
                &displaced_name,
                original_identity,
                &original_sha,
                4 * 1024 * 1024 * 1024,
            )?;
        } else {
            bail!("local checkpoint vault differs from both rollback states");
        }
        let mut slot = PublicationSlot::resume_with_limit(
            state.path(),
            &stage_name,
            identity,
            sha,
            4 * 1024 * 1024 * 1024,
        )?;
        slot.rename("status.sqlite3", false)?;
    }
    if let Some(bytes) = marker_bytes
        && !state.consume_packet("global-state.json", &bytes)?
    {
        bail!("checkpoint rollback marker changed; local latest rows retained");
    }
    record["state"] = serde_json::json!("rolled_back");
    record["routing_published"] = serde_json::json!(false);
    crate::file_io::write_json(&protected.path().join(&journal_name), &record)?;
    Ok(record)
}

#[cfg(all(test, windows))]
mod migration_control_tests {
    use super::*;
    #[test]
    fn checkpoint_control_accepts_line_endings_but_rejects_duplicate_or_changed_fields() {
        assert!(
            same_checkpoint_control(
                b"{\n\"active\":true,\"id\":\"x\"\n}",
                b"{\r\n\"id\":\"x\",\"active\":true\r\n}"
            )
            .unwrap()
        );
        assert!(!same_checkpoint_control(b"{\"active\":true}", b"{\"active\":false}").unwrap());
        assert!(
            same_checkpoint_control(b"{\"active\":true}", b"{\"active\":true,\"Active\":false}")
                .is_err()
        );
        assert!(
            same_checkpoint_control(b"{\"active\":true}", b"{\"active\":true,\"active\":true}")
                .is_err()
        );
    }
}
