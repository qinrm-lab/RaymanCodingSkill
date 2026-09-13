//! Sandbox-side client. A protected registration and result are verified;
//! neither a caller JSON document nor an executable supplied by a request is trusted.
use super::*;
use std::{
    io::Write,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

pub struct Client {
    root: ProtectedDirectory,
    installation: Installation,
}

impl Client {
    pub fn bootstrap_linked_worktree(
        &self,
        workspace: &Path,
        publish: bool,
    ) -> Result<serde_json::Value> {
        let (request, anchor) =
            self.prepare_linked_worktree_request(workspace, chrono::Utc::now().timestamp())?;
        let Operation::EnrollLinkedWorktree {
            workspace,
            policy_sha256,
            ..
        } = &request.operation
        else {
            unreachable!()
        };
        let _target = super::native::SourceDirectory::open(workspace)?;
        let _git_marker = _target.pin_file(Path::new(".git"))?;
        let marker_bytes = _target.read_file(Path::new(".git"), 4096)?;
        let git_path = std::str::from_utf8(&marker_bytes)?
            .trim()
            .strip_prefix("gitdir: ")
            .ok_or_else(|| anyhow::anyhow!("linked Git marker changed"))?;
        let _private_git = super::native::SourceDirectory::open(&_target.path().join(git_path))?;
        let anchor_git = anchor
            .git
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("anchor Git missing"))?;
        let git_parent = super::native::SourceDirectory::open(
            anchor_git
                .executable
                .parent()
                .ok_or_else(|| anyhow::anyhow!("Git executable parent missing"))?,
        )?;
        // The official Git image has legitimate hard-link aliases. Deny all
        // writes through the executable handle rather than requiring link count 1.
        use std::os::windows::fs::OpenOptionsExt;
        let git_program = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ)
            .custom_flags(windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT)
            .open(&anchor_git.executable)?;
        let metadata = git_program.metadata()?;
        if !metadata.is_file()
            || crate::file_io::is_link_or_reparse(&metadata)
            || metadata.len() > 128 * 1024 * 1024
        {
            bail!("invalid anchor Git executable");
        }
        let bytes = crate::file_io::read_bytes_from_handle(
            &git_program,
            metadata.len(),
            &anchor_git.executable,
            "bootstrap Git executable",
        )?;
        if crate::hash::sha256_bytes(&bytes) != anchor_git.executable_sha256 {
            bail!("anchor Git executable changed");
        }
        let common = super::native::SourceDirectory::open(&anchor_git.common_directory)?;
        let _git_config = common.pin_file(Path::new("config"))?;
        if crate::hash::sha256_file(&anchor_git.common_directory.join("config"))?
            != anchor_git.config_sha256
        {
            bail!("anchor Git configuration changed");
        }
        let registry = super::native::SourceDirectory::open(self.root.path())?;
        let _anchor = registry.pin_file(Path::new(&format!(
            "project-{}.json",
            anchor.registration.worktree_id
        )))?;
        let _policy = registry.pin_file(Path::new(&format!(
            "worktree-policy-{}.json",
            anchor.registration.worktree_id
        )))?;
        let (policy, digest) =
            super::worktrees::checked_policy(&self.root, &self.installation, &anchor)?;
        if digest != *policy_sha256 {
            bail!("worktree policy changed before bootstrap");
        }
        let existing = self.lookup_enrollment(workspace)?;
        if let Some(child) = &existing {
            super::worktrees::verify_member(&self.root, child, &digest)?;
        }
        let _products = super::worktrees::pin_products(&policy)?;
        if !publish {
            return Ok(
                serde_json::json!({"preview":true,"executed":false,"already_enrolled":existing.is_some(),"rayman_requested":policy.rayman.is_some(),"checkpoint_requested":policy.checkpoint.is_some(),"desktop_owner_required":policy.rayman.is_some() || policy.checkpoint.is_some()}),
            );
        }
        if policy.rayman.is_none() && policy.checkpoint.is_none() {
            if existing.is_none() {
                self.submit(
                    &request,
                    &anchor.registration,
                    chrono::Utc::now().timestamp(),
                )?;
                self.wait(&request, Duration::from_secs(120), |_| {})?;
            }
            let child = self.enrollment(workspace)?;
            return Ok(
                serde_json::json!({"initialized":true,"worktree_id":child.registration.worktree_id,"rayman":false,"checkpoint":false,"parent_history_copied":false}),
            );
        }
        let context = crate::execution_context::execution_context_probe();
        if context.principal_sid.as_deref() != Some(self.installation.owner_sid.as_str()) {
            bail!(
                "application initialization requires the already authorized desktop-owner hook context; no state initialization was attempted"
            );
        }
        let shell = policy
            .powershell
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("owner-pinned PowerShell missing"))?;
        let _client = registry.pin_file(Path::new("client.exe"))?;
        let contract = serde_json::json!({"root":self.root.path(),"workspace":workspace,"owner_sid":self.installation.owner_sid,"policy":policy,"already_enrolled":existing.is_some(),"worktree_id":existing.as_ref().map(|e| &e.registration.worktree_id)});
        let args = vec![
            "-NoProfile".into(),
            "-NonInteractive".into(),
            "-Command".into(),
            include_str!("../../../../scripts/bootstrap-global-codex-worktree.ps1").into(),
        ];
        let mut environment = std::collections::BTreeMap::new();
        // Initialization must not register a watchdog before routing is ready.
        // The normal post-bootstrap auto call owns session registration.
        for key in [
            "SystemRoot",
            "USERPROFILE",
            "LOCALAPPDATA",
            "APPDATA",
            "HOMEDRIVE",
            "HOMEPATH",
            "TEMP",
            "TMP",
            "TMPDIR",
            "RAYMAN_VALIDATION_TEMP_ROOT",
        ] {
            if let Ok(value) = std::env::var(key) {
                environment.insert(key.into(), value);
            }
        }
        let system = PathBuf::from(
            environment
                .get("SystemRoot")
                .ok_or_else(|| anyhow::anyhow!("Windows system root missing"))?,
        );
        environment.insert(
            "PATH".into(),
            std::env::join_paths([
                git_parent.path().to_path_buf(),
                shell.path.parent().unwrap().to_path_buf(),
                system.join("System32"),
                system,
            ])
            .map_err(|e| anyhow::anyhow!("invalid trusted helper path: {e}"))?
            .to_string_lossy()
            .into(),
        );
        environment.insert("NoDefaultCurrentDirectoryInExePath".into(), "1".into());
        environment.insert(
            "RAYMAN_GLOBAL_EXECUTION_ROOT".into(),
            crate::pathfmt::display_path(self.root.path()),
        );
        let output = super::process::run_sandbox_process_tree(
            &shell.path,
            &args,
            workspace,
            &environment,
            &serde_json::to_vec(&contract)?,
            600_000,
            1024 * 1024,
        )?;
        if output.exit_code != 0 {
            bail!(
                "worktree state bootstrap failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        decode_json(&output.stdout)
    }
    pub fn prepare_linked_worktree_request(
        &self,
        workspace: &Path,
        now: i64,
    ) -> Result<(Request, Enrollment)> {
        let (workspace, identity, common) = super::worktrees::linked_common_identity(workspace)?;
        let mut selected = None;
        let mut count = 0;
        for entry in std::fs::read_dir(self.root.path())? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(id) = name
                .strip_prefix("worktree-policy-")
                .and_then(|s| s.strip_suffix(".json"))
            else {
                continue;
            };
            if !is_id(id) {
                bail!("invalid protected worktree policy filename");
            }
            count += 1;
            if count > 1024 {
                bail!("worktree policy registry exceeds limit");
            }
            let anchor: Enrollment = decode_json(
                &self
                    .root
                    .read_file(&format!("project-{id}.json"), MAX_REQUEST_BYTES as u64)?,
            )?;
            anchor.validate(&self.installation)?;
            if anchor.registration.git_common_identity != common {
                continue;
            }
            let (policy, digest) =
                super::worktrees::checked_policy(&self.root, &self.installation, &anchor)?;
            let allowed = super::native::SourceDirectory::open(&policy.allowed_root)?;
            if workspace == allowed.path() || !workspace.starts_with(allowed.path()) {
                continue;
            }
            if selected.is_some() {
                bail!("ambiguous owner worktree delegations");
            }
            selected = Some((anchor, digest));
        }
        let (anchor, digest) = selected.ok_or_else(|| {
            anyhow::anyhow!("no owner-approved policy covers this linked worktree")
        })?;
        let request = self.request(
            &anchor,
            Operation::EnrollLinkedWorktree {
                workspace,
                root_identity: identity,
                policy_sha256: digest,
            },
            now,
        )?;
        Ok((request, anchor))
    }

    pub fn prepare_commit_recovery_request(
        &self,
        workspace: &Path,
        original_id: &str,
        expected_candidate: Option<&str>,
        now: i64,
    ) -> Result<Request> {
        if !is_id(original_id) || expected_candidate.is_some_and(|hash| !is_sha256(hash)) {
            bail!("invalid commit recovery identity");
        }
        let enrollment = self.enrollment(workspace)?;
        let directory = super::native::SourceDirectory::open(
            &self.root.path().join(format!("candidate-{original_id}")),
        )?;
        let bytes = directory.read_file(Path::new("candidate.json"), 64 * 1024 * 1024)?;
        let digest = crate::hash::sha256_bytes(&bytes);
        if expected_candidate.is_some_and(|expected| expected != digest) {
            bail!("recovery candidate changed since review");
        }
        let candidate: CommitCandidate = serde_json::from_slice(&bytes)?;
        if candidate.registration_sha256 != enrollment.registration.digest()? {
            bail!("recovery candidate belongs to a different registration");
        }
        let archive_name = format!("commit-request-{original_id}.json");
        let admitted = StateStorage::at_existing_root(self.root.path(), &enrollment.registration)?
            .snapshot_recorded_request_by_id(original_id, &enrollment.registration)?
            .ok_or_else(|| anyhow::anyhow!("recovery has no admitted original intent"))?;
        if admitted.request_sha256 != candidate.request_sha256
            || !matches!(
                admitted.result,
                RecordedResult::RecoveryRequired | RecordedResult::EffectSucceeded { .. }
            )
        {
            bail!("recovery admission does not match the candidate");
        }
        let request = self.request(
            &enrollment,
            Operation::RecoverCommit {
                original_request_id: original_id.into(),
                candidate_sha256: digest.clone(),
            },
            now,
        )?;
        let mut legacy = false;
        match std::fs::symlink_metadata(self.root.path().join(&archive_name)) {
            Ok(_) => {
                let original = decode_request(
                    &self
                        .root
                        .read_file(&archive_name, MAX_REQUEST_BYTES as u64)?,
                )?;
                if original.request_id != original_id
                    || original.digest()? != candidate.request_sha256
                    || !matches!(original.operation, Operation::Commit { .. })
                {
                    bail!("recovery original request binding differs");
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                legacy = true;
                if enrollment
                    .git
                    .as_ref()
                    .is_none_or(|git| git.hook_policy.is_some())
                {
                    bail!("legacy recovery without a retained request cannot attest hooks");
                }
            }
            Err(e) => return Err(e.into()),
        }
        if legacy && admitted.result == RecordedResult::RecoveryRequired {
            let git_binding = enrollment
                .git
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("Git enrollment missing"))?;
            let identity = enrollment
                .commit_identity
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("commit identity missing"))?;
            let git =
                GitInspector::open(workspace, git_binding, &enrollment.registration, &self.root)?;
            git.preview_legacy_candidate(
                directory.path(),
                &candidate,
                &enrollment.registration,
                &super::git::LegacyRecovery {
                    original_id,
                    original_digest: &admitted.request_sha256,
                    candidate_sha256: &digest,
                    source_sha256: &request.source_sha256,
                    identity,
                },
            )?;
        }
        Ok(request)
    }
    pub fn prepare_install_request(
        &self,
        workspace: &Path,
        adapter_id: &str,
        version: &str,
        sources: &std::collections::BTreeMap<String, String>,
        now: i64,
    ) -> Result<Request> {
        if !is_label(adapter_id) || sources.is_empty() || sources.len() > 64 {
            bail!("invalid installation input roles");
        }
        let enrollment = self.enrollment(workspace)?;
        let name = format!(
            "install-adapter-{}-{adapter_id}.json",
            enrollment.registration.worktree_id
        );
        let adapter: InstallAdapter =
            super::decode_json(&self.root.read_file(&name, MAX_REQUEST_BYTES as u64)?)?;
        adapter.validate(&enrollment)?;
        if enrollment.install_policies.get(adapter_id) != Some(&adapter.digest()?) {
            bail!("install target policy is not enrolled");
        }
        let source = super::native::SourceDirectory::open(workspace)?;
        let fingerprint = crate::source_fingerprint(workspace)?;
        let mut files = Vec::new();
        for (role, path) in sources {
            validate_relative_path(path)?;
            let target = adapter
                .targets
                .get(role)
                .ok_or_else(|| anyhow::anyhow!("unregistered install role"))?;
            let destination = target.resolve(version)?;
            let bytes = source.read_file(Path::new(path), 256 * 1024 * 1024)?;
            let parent = super::native::SourceDirectory::open(destination.parent().unwrap())?;
            if parent.identity() != target.parent_identity {
                bail!("install destination parent changed");
            }
            let before = if destination.try_exists()? {
                Some(crate::hash::sha256_bytes(&parent.read_file(
                    Path::new(destination.file_name().unwrap()),
                    256 * 1024 * 1024,
                )?))
            } else {
                None
            };
            files.push(InstallBundleFile {
                role: role.clone(),
                source: path.clone(),
                sha256: crate::hash::sha256_bytes(&bytes),
                size: bytes.len() as u64,
                expected_current_sha256: before,
            });
        }
        let bundle = InstallBundle {
            schema_version: 1,
            adapter_id: adapter_id.into(),
            version: version.into(),
            source_sha256: fingerprint.clone(),
            files,
        };
        bundle.validate(&adapter)?;
        let digest = bundle.digest()?;
        let packets = source
            .path()
            .join(".RaymanCodingSkill/tmp/install-packages");
        std::fs::create_dir_all(&packets)?;
        let packets = super::native::SourceDirectory::open(&packets)?;
        let name = format!("{digest}.json");
        let path = packets.path().join(&name);
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut file) => {
                file.write_all(&serde_json::to_vec(&bundle)?)?;
                file.sync_all()?;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                let old: InstallBundle = super::decode_json(
                    &packets.read_file(Path::new(&name), MAX_REQUEST_BYTES as u64)?,
                )?;
                if old != bundle {
                    bail!("existing install bundle differs");
                }
            }
            Err(e) => return Err(e.into()),
        }
        let request = self.request(
            &enrollment,
            Operation::Install {
                adapter_id: adapter_id.into(),
                artifact_sha256: digest,
            },
            now,
        )?;
        if request.source_sha256 != fingerprint {
            bail!("source changed while preparing installation");
        }
        Ok(request)
    }

    pub fn status(&self) -> Result<serde_json::Value> {
        let heartbeat = if self.root.path().join("heartbeat.json").try_exists()? {
            Some(super::decode_json::<serde_json::Value>(
                &self
                    .root
                    .read_file("heartbeat.json", MAX_REQUEST_BYTES as u64)?,
            )?)
        } else {
            None
        };
        let now = chrono::Utc::now().timestamp();
        let age = heartbeat
            .as_ref()
            .and_then(|h| h["observed_at"].as_i64())
            .map(|time| now.saturating_sub(time));
        let bound = heartbeat.as_ref().is_some_and(|h| {
            h["schema"] == "rayman.global-heartbeat.v1"
                && h["installation_id"] == self.installation.installation_id
                && h["executor_sid"] == self.installation.owner_sid
        });
        let fresh = bound && age.is_some_and(|age| (0..=10).contains(&age));
        let maintenance =
            match std::fs::symlink_metadata(self.root.path().join("kernel-maintenance.json")) {
                Ok(_) => true,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
                Err(e) => return Err(e.into()),
            } || heartbeat.as_ref().is_some_and(|h| h["maintenance"] == true);
        let install_pending = super::install::pending_request(&self.root)?
            .map(|r| serde_json::json!({"request_id":r.request_id,"worktree_id":r.worktree_id}));
        let queue = if self.root.path().join("queue-health.json").try_exists()? {
            Some(super::decode_json::<serde_json::Value>(
                &self
                    .root
                    .read_file("queue-health.json", MAX_REQUEST_BYTES as u64)?,
            )?)
        } else {
            None
        };
        Ok(
            serde_json::json!({"schema":"rayman.global-status.v1","installation_verified":true,"owner_sid":self.installation.owner_sid,"heartbeat_fresh":fresh,"heartbeat_age_seconds":age,"maintenance_pending":maintenance,"service_healthy":!maintenance && fresh && heartbeat.as_ref().is_some_and(|h|h["error"].is_null()),"heartbeat":heartbeat,"queue":queue,"install_transaction_pending":install_pending,"validation_authority":false}),
        )
    }

    pub fn enqueue_storage(
        &self,
        enrollment: &Enrollment,
        action: StorageAction,
    ) -> Result<String> {
        let now = chrono::Utc::now().timestamp();
        let request = self.request(enrollment, Operation::Storage { action }, now)?;
        self.submit(&request, &enrollment.registration, now)?;
        Ok(request.request_id)
    }
    pub fn storage_call(
        &self,
        enrollment: &Enrollment,
        action: StorageAction,
    ) -> Result<serde_json::Value> {
        let now = chrono::Utc::now().timestamp();
        let request = self.request(enrollment, Operation::Storage { action }, now)?;
        self.submit(&request, &enrollment.registration, now)?;
        let result = self.wait(&request, Duration::from_secs(180), |_| {})?;
        match result.result {
            RecordedResult::Storage { details } => Ok(details),
            _ => bail!("unexpected application storage result"),
        }
    }

    pub fn write_application_state(
        &self,
        enrollment: &Enrollment,
        object: StateObject,
        lease_id: &str,
        expected_sha256: Option<String>,
        bytes: &[u8],
    ) -> Result<serde_json::Value> {
        if bytes.is_empty() || bytes.len() > 64 * 1024 * 1024 {
            bail!("application state exceeds transfer limit");
        }
        let sha = self.upload_blob(enrollment, lease_id, bytes)?;
        let total = bytes.len() as u64;
        self.storage_call(
            enrollment,
            StorageAction::Write {
                object,
                lease_id: lease_id.into(),
                expected_sha256,
                object_sha256: sha,
                total_bytes: total,
            },
        )
    }
    fn upload_blob(&self, enrollment: &Enrollment, lease_id: &str, bytes: &[u8]) -> Result<String> {
        if bytes.len() > 512 * 1024 * 1024 {
            bail!("blob exceeds transfer limit");
        }
        let sha = crate::hash::sha256_bytes(bytes);
        let total = bytes.len() as u64;
        for (batch_index, batch) in bytes.chunks(65536 * 16).enumerate() {
            self.storage_call(
                enrollment,
                StorageAction::Renew {
                    lease_id: lease_id.into(),
                },
            )?;
            let mut requests = Vec::new();
            for (index, chunk) in batch.chunks(65536).enumerate() {
                let now = chrono::Utc::now().timestamp();
                let request = self.request(
                    enrollment,
                    Operation::Storage {
                        action: StorageAction::Chunk {
                            object_sha256: sha.clone(),
                            index: (batch_index * 16 + index) as u32,
                            total_bytes: total,
                            hex: chunk.iter().map(|b| format!("{b:02x}")).collect(),
                        },
                    },
                    now,
                )?;
                self.submit(&request, &enrollment.registration, now)?;
                requests.push(request);
            }
            for request in requests {
                let result = self.wait(&request, Duration::from_secs(180), |_| {})?;
                if !matches!(result.result, RecordedResult::Storage { .. }) {
                    bail!("unexpected chunk transfer result");
                }
            }
        }
        Ok(sha)
    }

    pub fn copy_checkpoint(&self, enrollment: &Enrollment, destination: &Path) -> Result<()> {
        if !enrollment.registration.capabilities.formal_state {
            bail!("checkpoint storage is not enrolled");
        }
        let name = format!("checkpoint-{}.sqlite3", enrollment.registration.worktree_id);
        let _pin = self.root.pin_data_file(&name)?;
        let source = rusqlite::Connection::open_with_flags(
            self.root.path().join(&name),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?;
        checkpoint_store::validate_schema(&source)?;
        let parent = super::native::SourceDirectory::open(
            destination
                .parent()
                .ok_or_else(|| anyhow::anyhow!("checkpoint copy parent missing"))?,
        )?;
        let new_file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(destination)?;
        drop(new_file);
        let mut copy = rusqlite::Connection::open(destination)?;
        {
            let backup = rusqlite::backup::Backup::new(&source, &mut copy)?;
            let started = Instant::now();
            loop {
                if started.elapsed() > Duration::from_secs(60) {
                    bail!("checkpoint copy timed out; incomplete destination retained");
                }
                match backup.step(256)? {
                    rusqlite::backup::StepResult::Done => break,
                    rusqlite::backup::StepResult::More => {}
                    rusqlite::backup::StepResult::Busy | rusqlite::backup::StepResult::Locked => {
                        std::thread::sleep(Duration::from_millis(20))
                    }
                    _ => bail!("unsupported SQLite backup state"),
                }
            }
        }
        checkpoint_store::validate_schema(&copy)?;
        let integrity: String = copy.query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
        if integrity != "ok" {
            bail!("checkpoint copy integrity differs");
        }
        drop(parent);
        Ok(())
    }

    pub fn apply_checkpoint_copies(
        &self,
        enrollment: &Enrollment,
        before: &Path,
        after: &Path,
        lease_id: &str,
        transaction_id: &str,
    ) -> Result<serde_json::Value> {
        if !is_id(transaction_id) || !is_id(lease_id) {
            bail!("checkpoint transaction/lease identity invalid");
        }
        let old = rusqlite::Connection::open_with_flags(
            before,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?;
        let new = rusqlite::Connection::open_with_flags(
            after,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?;
        let workspace_id = super::application_state::checkpoint_workspace_id(enrollment);
        if checkpoint_store::other_rows_digest(&old, &workspace_id)?
            != checkpoint_store::other_rows_digest(&new, &workspace_id)?
        {
            bail!("checkpoint working copy changed another tenant or backend receipt");
        }
        let changes = checkpoint_store::diff(
            &checkpoint_store::snapshot(&old, &workspace_id)?,
            &checkpoint_store::snapshot(&new, &workspace_id)?,
        )?;
        if changes.is_empty() {
            return Ok(
                serde_json::json!({"applied":true,"transaction_id":transaction_id,"unchanged":true}),
            );
        }
        let blobs = checkpoint_store::changed_blobs(&new, &changes)?;
        for bytes in blobs.values() {
            self.upload_blob(enrollment, lease_id, bytes)?;
        }
        let patch = serde_json::to_vec(&changes)?;
        if patch.len() > 64 * 1024 * 1024 {
            bail!("checkpoint row patch exceeds limit");
        }
        let sha = self.upload_blob(enrollment, lease_id, &patch)?;
        self.storage_call(
            enrollment,
            StorageAction::CheckpointApply {
                lease_id: lease_id.into(),
                transaction_id: transaction_id.into(),
                patch_sha256: sha,
                total_bytes: patch.len() as u64,
            },
        )
    }

    pub fn query(&self, id: &str) -> Result<Option<WorkerResult>> {
        if !is_id(id) {
            bail!("invalid result lookup identity");
        }
        let name = format!("result-{id}.json");
        if !self.root.path().join(&name).try_exists()? {
            return Ok(None);
        }
        let result: super::worker::QueueResult =
            super::decode_json(&self.root.read_file(&name, MAX_REQUEST_BYTES as u64)?)?;
        if result.installation_id != self.installation.installation_id
            || result.executor_sid != self.installation.owner_sid
            || result.request_id != id
        {
            bail!("historical result identity mismatch");
        }
        if !result.success {
            bail!(
                "global operation failed: {}",
                result.error.as_deref().unwrap_or("missing error")
            );
        }
        let output: WorkerResult = serde_json::from_value(
            result
                .output
                .ok_or_else(|| anyhow::anyhow!("historical output missing"))?,
        )?;
        if output.request_sha256 != result.request_sha256
            || output.request_id != id
            || output.executor_sid != self.installation.owner_sid
            || output.installation_id != self.installation.installation_id
        {
            bail!("historical worker result mismatch");
        }
        Ok(Some(output))
    }
    pub fn prepare_commit_request(
        &self,
        workspace: &Path,
        paths: &[String],
        message: &str,
        now: i64,
    ) -> Result<Request> {
        let enrollment = self.enrollment(workspace)?;
        if enrollment.registration.project_id == self.installation.source_project_id {
            bail!(
                "global client cannot submit a commit for its own installation source repository"
            );
        }
        let binding = enrollment
            .git
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("local commit capability is not enrolled"))?;
        let inspector =
            GitInspector::open(workspace, binding, &enrollment.registration, &self.root)?;
        let snapshot = inspector.capture(&enrollment.registration)?;
        let mut selected = paths.to_vec();
        selected.sort();
        if selected.is_empty() || selected.windows(2).any(|p| p[0] == p[1]) {
            bail!("explicit unique commit paths are required");
        }
        for path in &selected {
            validate_relative_path(path)?;
        }
        let mut changes = Vec::new();
        for path in selected {
            let change = snapshot
                .changes
                .iter()
                .find(|c| c.path == path)
                .ok_or_else(|| anyhow::anyhow!("selected commit path is not changed: {path}"))?;
            changes.push(change.clone());
        }
        let mut request = self.request(
            &enrollment,
            Operation::Commit {
                expected_head: snapshot.head,
                expected_index_sha256: snapshot.index_sha256,
                message: message.into(),
                changes,
                hook_receipt: None,
            },
            now,
        )?;
        if let Some(policy) = &binding.hook_policy {
            let receipt =
                inspector.run_hook_preflight(&request, &enrollment.registration, policy)?;
            let Operation::Commit { hook_receipt, .. } = &mut request.operation else {
                unreachable!()
            };
            *hook_receipt = Some(receipt);
            validate_request(&request, &enrollment.registration, now)?;
        }
        Ok(request)
    }
    pub fn open(root: &Path) -> Result<Self> {
        let (bytes, _) = crate::file_io::read_optional_handle_bound_file_bounded(
            &root.join("installation.json"),
            "global installation",
            MAX_REQUEST_BYTES as u64,
        )?
        .ok_or_else(|| anyhow::anyhow!("global execution is not installed"))?;
        let manifest: Installation = super::decode_json(&bytes)?;
        manifest.validate()?;
        let protected = ProtectedDirectory::open(root, &manifest.owner_sid)?;
        if protected.read_file("installation.json", MAX_REQUEST_BYTES as u64)? != bytes
            || protected.identity() != manifest.root_identity
        {
            bail!("global installation changed during client attestation");
        }
        if crate::hash::sha256_bytes(&protected.read_file("worker.exe", 128 * 1024 * 1024)?)
            != manifest.worker_sha256
        {
            bail!("global installed worker hash mismatch");
        }
        let current = std::fs::canonicalize(std::env::current_exe()?)?;
        if current == protected.path().join("client.exe")
            && crate::hash::sha256_bytes(&protected.read_file("client.exe", 128 * 1024 * 1024)?)
                != manifest.client_sha256
        {
            bail!("global installed client hash mismatch");
        }
        Ok(Self {
            root: protected,
            installation: manifest,
        })
    }

    pub fn enrollment(&self, workspace: &Path) -> Result<Enrollment> {
        self.lookup_enrollment(workspace)?
            .ok_or_else(|| anyhow::anyhow!("workspace is not enrolled in global execution"))
    }

    fn lookup_enrollment(&self, workspace: &Path) -> Result<Option<Enrollment>> {
        let workspace = std::fs::canonicalize(workspace)?;
        let mut found = None;
        let mut count = 0;
        for entry in std::fs::read_dir(self.root.path())? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(id) = name
                .strip_prefix("project-")
                .and_then(|n| n.strip_suffix(".json"))
            else {
                continue;
            };
            if !is_id(id) {
                bail!("malformed protected project registration filename");
            }
            count += 1;
            if count > 1024 {
                bail!("project registry exceeds client limit");
            }
            let enrollment: Enrollment =
                super::decode_json(&self.root.read_file(&name, MAX_REQUEST_BYTES as u64)?)?;
            enrollment.validate(&self.installation)?;
            if enrollment.registration.worktree_id != id {
                bail!("registration filename and worktree differ");
            }
            if crate::pathfmt::display_path(&enrollment.workspace)
                .replace('/', "\\")
                .to_lowercase()
                == crate::pathfmt::display_path(&workspace)
                    .replace('/', "\\")
                    .to_lowercase()
            {
                if found.is_some() {
                    bail!("workspace has ambiguous global registrations");
                }
                let pin = super::native::SourceDirectory::open(&workspace)?;
                if pin.identity() != enrollment.registration.root_identity {
                    bail!("registered workspace was replaced");
                }
                found = Some(enrollment);
            }
        }
        Ok(found)
    }

    pub fn request(
        &self,
        enrollment: &Enrollment,
        operation: Operation,
        now: i64,
    ) -> Result<Request> {
        enrollment.validate(&self.installation)?;
        let mut random = [0u8; 16];
        getrandom::fill(&mut random)
            .map_err(|e| anyhow::anyhow!("request entropy unavailable: {e}"))?;
        let id = random.iter().map(|b| format!("{b:02x}")).collect();
        let request = Request {
            schema_version: 1,
            request_id: id,
            project_id: enrollment.registration.project_id.clone(),
            worktree_id: enrollment.registration.worktree_id.clone(),
            registration_sha256: enrollment.registration.digest()?,
            source_sha256: if matches!(
                operation,
                Operation::Storage { .. } | Operation::EnrollLinkedWorktree { .. }
            ) {
                "0".repeat(64)
            } else {
                crate::source_fingerprint(&enrollment.workspace)?
            },
            created_at: now,
            expires_at: now
                .checked_add(MAX_LIFETIME_SECONDS)
                .ok_or_else(|| anyhow::anyhow!("request expiry overflow"))?,
            operation,
        };
        validate_request(&request, &enrollment.registration, now)?;
        Ok(request)
    }

    pub fn submit(
        &self,
        request: &Request,
        registration: &Registration,
        now: i64,
    ) -> Result<PathBuf> {
        use std::os::windows::ffi::OsStrExt;
        match std::fs::symlink_metadata(self.root.path().join("kernel-maintenance.json")) {
            Ok(_) => bail!("kernel maintenance is active; submission was not queued"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        validate_request(request, registration, now)?;
        if self
            .root
            .path()
            .join(format!("result-{}.json", request.request_id))
            .try_exists()?
        {
            bail!("request already has a protected result; query it instead of resubmitting");
        }
        let name = format!("project-{}.json", request.worktree_id);
        let enrolled: Enrollment =
            super::decode_json(&self.root.read_file(&name, MAX_REQUEST_BYTES as u64)?)?;
        enrolled.validate(&self.installation)?;
        if enrolled.registration.digest()? != registration.digest()? {
            bail!("submission registration differs from protected enrollment");
        }
        let bytes = serde_json::to_vec(request)?;
        if bytes.len() > MAX_REQUEST_BYTES {
            bail!("encoded request exceeds limit");
        }
        let queue = super::native::SourceDirectory::open(&self.root.path().join("requests"))?;
        let target = queue
            .path()
            .join(format!("{}.request.json", request.request_id));
        if target.try_exists()? {
            bail!("request filename already exists");
        }
        let staging = queue.path().join(format!("{}.upload", request.request_id));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staging)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        let from: Vec<u16> = staging.as_os_str().encode_wide().chain(Some(0)).collect();
        let to: Vec<u16> = target.as_os_str().encode_wide().chain(Some(0)).collect();
        // MoveFileW is no-replace; the worker never observes a partial upload.
        if unsafe { windows_sys::Win32::Storage::FileSystem::MoveFileW(from.as_ptr(), to.as_ptr()) }
            == 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(target)
    }

    pub fn result(&self, request: &Request) -> Result<Option<WorkerResult>> {
        if !is_id(&request.request_id) || !is_id(&request.worktree_id) {
            bail!("invalid result lookup identity");
        }
        let name = format!("result-{}.json", request.request_id);
        if !self.root.path().join(&name).try_exists()? {
            return Ok(None);
        }
        let result: super::worker::QueueResult =
            super::decode_json(&self.root.read_file(&name, MAX_REQUEST_BYTES as u64)?)?;
        if result.schema != "rayman.global-queue-result.v1"
            || result.installation_id != self.installation.installation_id
            || result.executor_sid != self.installation.owner_sid
            || result.request_id != request.request_id
            || result.request_sha256 != crate::hash::sha256_bytes(&serde_json::to_vec(request)?)
        {
            bail!("global result identity or request bytes mismatch");
        }
        if !result.success {
            bail!(
                "global operation failed: {}",
                result.error.as_deref().unwrap_or("missing error")
            );
        }
        let output: WorkerResult = serde_json::from_value(
            result
                .output
                .ok_or_else(|| anyhow::anyhow!("global result missing output"))?,
        )?;
        if output.schema != "rayman.global-worker-result.v1"
            || output.installation_id != self.installation.installation_id
            || output.executor_sid != self.installation.owner_sid
            || output.request_id != request.request_id
            || output.request_sha256 != request.digest()?
            || output.registration_sha256 != request.registration_sha256
        {
            bail!("global worker result binding differs");
        }
        Ok(Some(output))
    }

    pub fn wait(
        &self,
        request: &Request,
        timeout: Duration,
        mut progress: impl FnMut(Duration),
    ) -> Result<WorkerResult> {
        if timeout.is_zero() || timeout > Duration::from_secs(600) {
            bail!("client wait must be bounded to at most 600 seconds");
        }
        let start = Instant::now();
        let mut last = Duration::ZERO;
        loop {
            match self.result(request) {
                Ok(Some(result)) => return Ok(result),
                Ok(None) => {}
                // Atomic publication may expose the final name just before
                // Windows releases its rename/write handle. Poll that same
                // immutable result within the existing deadline; never replay
                // the operation or retry permission/data/identity failures.
                Err(error)
                    if error
                        .downcast_ref::<std::io::Error>()
                        .is_some_and(|e| matches!(e.raw_os_error(), Some(32 | 33))) => {}
                Err(error) => return Err(error),
            }
            let elapsed = start.elapsed();
            if elapsed >= timeout {
                bail!(
                    "global result wait expired; request_id={}; query this request before retrying",
                    request.request_id
                );
            }
            if elapsed.saturating_sub(last) >= Duration::from_secs(15) {
                progress(elapsed);
                last = elapsed;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}
