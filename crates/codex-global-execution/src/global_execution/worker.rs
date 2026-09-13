//! Windows worker core. Only an authenticated installation can reach storage.
//! The initial state handler cannot dispatch an arbitrary program or script.
use super::*;
use crate::hash::sha256_bytes;
use serde::{Deserialize, Serialize};
use std::path::Path;

pub struct Worker {
    root: ProtectedDirectory,
    installation: Installation,
    application_state: std::sync::Mutex<super::application_state::ApplicationState>,
    activity: std::sync::Mutex<WorkerActivity>,
    recovery_checked: std::sync::atomic::AtomicBool,
}

#[derive(Clone, Default, Serialize)]
struct WorkerActivity {
    request_id: Option<String>,
    started_at: Option<i64>,
    stage: Option<String>,
    completed_requests: u64,
    last_error: Option<String>,
    service_error: Option<String>,
}

fn bounded_error(error: &anyhow::Error) -> String {
    let text = format!("{error:#}");
    let mut chars = text.chars();
    let mut bounded: String = chars.by_ref().take(1024).collect();
    if chars.next().is_some() {
        bounded.push_str(" [truncated]");
    }
    bounded
}

fn retain_commit_request(root: &ProtectedDirectory, request: &Request, bytes: &[u8]) -> Result<()> {
    if !matches!(
        request.operation,
        Operation::Commit { .. } | Operation::RecoverCommit { .. }
    ) || decode_request(bytes)?.digest()? != request.digest()?
        || !is_id(&request.request_id)
    {
        bail!("commit request archive binding differs");
    }
    let name = format!("commit-request-{}.json", request.request_id);
    let path = root.path().join(&name);
    let _lock = crate::state_lock::acquire_state_lock(&path)?;
    if path.try_exists()? {
        let old = root.read_file(&name, MAX_REQUEST_BYTES as u64)?;
        if decode_request(&old)?.digest()? != request.digest()? {
            bail!("retained commit request differs; original evidence preserved");
        }
        return Ok(());
    }
    let mut nonce = [0u8; 16];
    getrandom::fill(&mut nonce).map_err(|e| anyhow::anyhow!("archive entropy unavailable: {e}"))?;
    let nonce: String = nonce.iter().map(|b| format!("{b:02x}")).collect();
    let seed = format!(".commit-request-{}-{nonce}", request.request_id);
    let mut slot = super::publication::PublicationSlot::create(root.path(), &seed, bytes)?;
    slot.rename(&name, false)?;
    drop(slot);
    if root.read_file(&name, MAX_REQUEST_BYTES as u64)? != bytes {
        bail!("commit request archive readback differs");
    }
    Ok(())
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct QueueResult {
    pub(super) schema: String,
    pub(super) installation_id: String,
    pub(super) request_id: String,
    pub(super) request_sha256: String,
    pub(super) executor_sid: String,
    pub(super) success: bool,
    pub(super) output: Option<serde_json::Value>,
    pub(super) error: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerResult {
    pub schema: String,
    pub installation_id: String,
    pub request_id: String,
    pub request_sha256: String,
    pub registration_sha256: String,
    pub executor_sid: String,
    pub replayed: bool,
    pub result: RecordedResult,
}

impl Worker {
    fn maintenance_requested(&self) -> Result<bool> {
        let path = self.root.path().join("kernel-maintenance.json");
        match std::fs::symlink_metadata(path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(e.into()),
            Ok(_) => {}
        }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Marker {
            schema: String,
            installation_id: String,
            transaction_id: String,
            worker_sha256: Vec<String>,
        }
        let marker: Marker = super::decode_json(
            &self
                .root
                .read_file("kernel-maintenance.json", MAX_REQUEST_BYTES as u64)?,
        )?;
        if marker.schema != "rayman.global-kernel-maintenance.v1"
            || marker.installation_id != self.installation.installation_id
            || !is_id(&marker.transaction_id)
            || marker.worker_sha256.is_empty()
            || marker.worker_sha256.len() > 2
            || marker.worker_sha256.iter().any(|hash| !is_sha256(hash))
            || !marker
                .worker_sha256
                .contains(&self.installation.worker_sha256)
        {
            bail!("kernel maintenance marker does not match this installation");
        }
        Ok(true)
    }
    fn recover_legacy_commit(
        &self,
        request: &Request,
        enrollment: &Enrollment,
        storage: &StateStorage,
        bytes: &[u8],
    ) -> Result<WorkerResult> {
        let Operation::RecoverCommit {
            original_request_id,
            candidate_sha256,
        } = &request.operation
        else {
            bail!("not a recovery request");
        };
        let original = storage
            .recorded_request_by_id(original_request_id, &enrollment.registration)?
            .ok_or_else(|| anyhow::anyhow!("legacy recovery has no admitted original request"))?;
        if !matches!(
            original.result,
            RecordedResult::RecoveryRequired | RecordedResult::EffectSucceeded { .. }
        ) {
            bail!("legacy recovery refuses a terminal failure");
        }
        let directory = super::native::SourceDirectory::open(
            &self
                .root
                .path()
                .join(format!("candidate-{original_request_id}")),
        )?;
        let raw = directory.read_file(Path::new("candidate.json"), MAX_REQUEST_BYTES as u64)?;
        if sha256_bytes(&raw) != *candidate_sha256 {
            bail!("legacy candidate differs from review");
        }
        let candidate: CommitCandidate = super::decode_json(&raw)?;
        if candidate.request_sha256 != original.request_sha256
            || candidate.registration_sha256 != enrollment.registration.digest()?
        {
            bail!("legacy candidate admission differs");
        }
        retain_commit_request(&self.root, request, bytes)?;
        let result = if original.result == RecordedResult::RecoveryRequired {
            let binding = enrollment
                .git
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("Git capability missing"))?;
            let identity = enrollment
                .commit_identity
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("commit identity missing"))?;
            let git = GitInspector::open_for_worker(
                &enrollment.workspace,
                binding,
                &enrollment.registration,
                &self.root,
            )?;
            let committed = git.recover_legacy_prepared(
                super::git::LegacyRecovery {
                    original_id: original_request_id,
                    original_digest: &original.request_sha256,
                    candidate_sha256,
                    source_sha256: &request.source_sha256,
                    identity,
                },
                &enrollment.registration,
            )?;
            let outcome = RecordedResult::EffectSucceeded {
                outcome_sha256: sha256_bytes(&serde_json::to_vec(&committed)?),
            };
            storage.record_effect_result(
                original_request_id,
                &original.request_sha256,
                outcome.clone(),
            )?;
            outcome
        } else {
            original.result
        };
        Ok(WorkerResult {
            schema: "rayman.global-worker-result.v1".into(),
            installation_id: self.installation.installation_id.clone(),
            request_id: request.request_id.clone(),
            request_sha256: request.digest()?,
            registration_sha256: enrollment.registration.digest()?,
            executor_sid: self.installation.owner_sid.clone(),
            replayed: true,
            result,
        })
    }
    fn stage(&self, name: &str) -> Result<()> {
        self.activity
            .lock()
            .map_err(|_| anyhow::anyhow!("worker activity lock poisoned"))?
            .stage = Some(name.into());
        Ok(())
    }

    pub fn open(root: &Path) -> Result<Self> {
        let executable = std::env::current_exe()?;
        Self::open_with_executable(root, &executable)
    }

    fn open_with_executable(root: &Path, executable: &Path) -> Result<Self> {
        let sid = crate::execution_context::execution_context_probe()
            .principal_sid
            .ok_or_else(|| anyhow::anyhow!("worker principal is unavailable"))?;
        let protected = ProtectedDirectory::open(root, &sid)?;
        let install: Installation = super::decode_json(
            &protected.read_file("installation.json", MAX_REQUEST_BYTES as u64)?,
        )?;
        install.validate()?;
        if install.owner_sid != sid || install.root_identity != protected.identity() {
            bail!("worker installation principal or root identity changed");
        }
        let expected = protected.path().join("worker.exe");
        if std::fs::canonicalize(executable)? != expected {
            bail!("worker must execute from its protected installation path");
        }
        let bytes = protected.read_file("worker.exe", 128 * 1024 * 1024)?;
        if sha256_bytes(&bytes) != install.worker_sha256 {
            bail!("installed worker hash changed");
        }
        Ok(Self {
            root: protected,
            installation: install,
            activity: std::sync::Mutex::new(WorkerActivity::default()),
            recovery_checked: std::sync::atomic::AtomicBool::new(false),
            application_state: std::sync::Mutex::new(
                super::application_state::ApplicationState::default(),
            ),
        })
    }

    pub fn process(&self, bytes: &[u8], now: i64) -> Result<WorkerResult> {
        if self.maintenance_requested()? {
            bail!("kernel maintenance is active; request was not admitted");
        }
        let request = decode_request(bytes)?;
        if !is_id(&request.worktree_id) {
            bail!("invalid worktree identity");
        }
        let name = format!("project-{}.json", request.worktree_id);
        let registry = super::native::SourceDirectory::open(self.root.path())?;
        let _enrollment_pin = registry.pin_file(Path::new(&name))?;
        let enrollment: Enrollment =
            super::decode_json(&self.root.read_file(&name, MAX_REQUEST_BYTES as u64)?)?;
        enrollment.validate(&self.installation)?;
        validate_request_structure(&request, &enrollment.registration)?;
        super::worktrees::verify_existing_member(&self.root, &enrollment)?;
        if let Operation::EnrollLinkedWorktree {
            workspace,
            root_identity,
            policy_sha256,
        } = &request.operation
        {
            validate_request(&request, &enrollment.registration, now)?;
            self.stage("worktree_enrollment")?;
            let linked = super::worktrees::register_linked(
                &self.root,
                &self.installation,
                &enrollment,
                workspace,
                root_identity,
                policy_sha256,
            )?;
            return Ok(WorkerResult {
                schema: "rayman.global-worker-result.v1".into(),
                installation_id: self.installation.installation_id.clone(),
                request_id: request.request_id.clone(),
                request_sha256: request.digest()?,
                registration_sha256: enrollment.registration.digest()?,
                executor_sid: self.installation.owner_sid.clone(),
                replayed: false,
                result: RecordedResult::Storage {
                    details: serde_json::json!({"worktree_id":linked.registration.worktree_id,"registered":true,"state_initialized":false,"installation_capabilities_inherited":false}),
                },
            });
        }
        if matches!(
            request.operation,
            Operation::Commit { .. } | Operation::RecoverCommit { .. }
        ) && enrollment.registration.project_id == self.installation.source_project_id
        {
            bail!(
                "global worker cannot commit its own installation source repository or linked worktrees"
            );
        }
        if let Operation::Storage { action } = &request.operation {
            self.stage("state_storage")?;
            validate_request(&request, &enrollment.registration, now)?;
            let details = self
                .application_state
                .lock()
                .map_err(|_| anyhow::anyhow!("application state lock poisoned"))?
                .process(&self.root, &enrollment, action, now)?;
            return Ok(WorkerResult {
                schema: "rayman.global-worker-result.v1".into(),
                installation_id: self.installation.installation_id.clone(),
                request_id: request.request_id.clone(),
                request_sha256: request.digest()?,
                registration_sha256: enrollment.registration.digest()?,
                executor_sid: self.installation.owner_sid.clone(),
                replayed: false,
                result: RecordedResult::Storage { details },
            });
        }
        let storage = StateStorage::at_existing_root(self.root.path(), &enrollment.registration)?;
        if let Operation::RecoverCommit {
            original_request_id,
            candidate_sha256,
        } = &request.operation
        {
            validate_request(&request, &enrollment.registration, now)?;
            if crate::source_fingerprint(&enrollment.workspace)? != request.source_sha256 {
                bail!("recovery source changed before admission");
            }
            let archived_name = format!("commit-request-{original_request_id}.json");
            let archive_present =
                match std::fs::symlink_metadata(self.root.path().join(&archived_name)) {
                    Ok(_) => true,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
                    Err(e) => return Err(e.into()),
                };
            if !archive_present {
                return self.recover_legacy_commit(&request, &enrollment, &storage, bytes);
            }
            let original_bytes = self.root.read_file(&archived_name, MAX_REQUEST_BYTES as u64)
                .map_err(|e| anyhow::anyhow!("original commit request archive unavailable; legacy recovery needs separate verified handling: {e:#}"))?;
            let original = decode_request(&original_bytes)?;
            if original.request_id != *original_request_id
                || !matches!(original.operation, Operation::Commit { .. })
            {
                bail!("recovery archive is not the original commit request");
            }
            let recorded = storage
                .recorded_result(&original, &enrollment.registration)?
                .ok_or_else(|| {
                    anyhow::anyhow!("commit recovery has no admitted original intent")
                })?;
            if !matches!(
                recorded,
                RecordedResult::RecoveryRequired | RecordedResult::EffectSucceeded { .. }
            ) {
                bail!("terminal failed commit cannot be replayed");
            }
            let candidate_dir = super::native::SourceDirectory::open(
                &self
                    .root
                    .path()
                    .join(format!("candidate-{original_request_id}")),
            )?;
            let candidate_bytes =
                candidate_dir.read_file(Path::new("candidate.json"), 64 * 1024 * 1024)?;
            if sha256_bytes(&candidate_bytes) != *candidate_sha256 {
                bail!("recovery candidate differs from the reviewed bytes");
            }
            let candidate: CommitCandidate = serde_json::from_slice(&candidate_bytes)?;
            if candidate.request_sha256 != original.digest()?
                || candidate.registration_sha256 != enrollment.registration.digest()?
            {
                bail!("recovery candidate differs from original admission");
            }
            retain_commit_request(&self.root, &request, bytes)?;
            // This invokes only the admitted Commit branch. The original queue
            // result remains immutable; this recovery has its own result ID.
            let recovered = self.process(&original_bytes, now)?;
            return Ok(WorkerResult {
                schema: "rayman.global-worker-result.v1".into(),
                installation_id: self.installation.installation_id.clone(),
                request_id: request.request_id.clone(),
                request_sha256: request.digest()?,
                registration_sha256: enrollment.registration.digest()?,
                executor_sid: self.installation.owner_sid.clone(),
                replayed: true,
                result: recovered.result,
            });
        }
        let recorded = storage.recorded_result(&request, &enrollment.registration)?;
        let reply = |result, replayed| -> Result<WorkerResult> {
            Ok(WorkerResult {
                schema: "rayman.global-worker-result.v1".into(),
                installation_id: self.installation.installation_id.clone(),
                request_id: request.request_id.clone(),
                request_sha256: request.digest()?,
                registration_sha256: enrollment.registration.digest()?,
                executor_sid: self.installation.owner_sid.clone(),
                replayed,
                result,
            })
        };
        if let Some(result) = recorded.as_ref()
            && *result != RecordedResult::RecoveryRequired
        {
            if matches!(request.operation, Operation::Install { .. })
                && super::install::pending_request(&self.root)?
                    .is_some_and(|active| active.request_id == request.request_id)
            {
                super::install::clear_active(&self.root, &request)?;
            }
            return reply(result.clone(), true);
        }
        if recorded.is_none() {
            validate_request(&request, &enrollment.registration, now)?;
        }
        if matches!(request.operation, Operation::Install { .. }) {
            let recovery = recorded.is_some();
            if !recovery
                && crate::source_fingerprint(&enrollment.workspace)? != request.source_sha256
            {
                bail!("install source changed before admission");
            }
            self.stage("prepare_installation")?;
            let prepared = super::install::prepare(&self.root, &enrollment, &request, recovery)?;
            if !recovery
                && crate::source_fingerprint(&enrollment.workspace)? != request.source_sha256
            {
                super::install::cancel_unadmitted(&self.root, &request)?;
                bail!("install source changed during staging");
            }
            let transition = storage.accept(&request, &enrollment.registration, now)?;
            self.stage(if recovery {
                "recover_installation"
            } else {
                "publish_installation"
            })?;
            let outcome = match prepared.execute(&self.root, recovery) {
                Ok(outcome) => outcome,
                Err(error) => {
                    self.stage("recover_installation")?;
                    prepared.execute(&self.root, true).map_err(|recovery_error| anyhow::anyhow!("installation failed: {error:#}; recovery unresolved: {recovery_error:#}"))?
                }
            };
            let result = if outcome["status"] == "files_published" {
                RecordedResult::EffectSucceeded {
                    outcome_sha256: sha256_bytes(&serde_json::to_vec(&outcome)?),
                }
            } else {
                RecordedResult::EffectFailed {
                    error_code: "installation_rolled_back".into(),
                }
            };
            crate::file_io::write_json(
                &self
                    .root
                    .path()
                    .join(format!("install-outcome-{}.json", request.request_id)),
                &outcome,
            )?;
            storage.record_effect_result(
                &request.request_id,
                &request.digest()?,
                result.clone(),
            )?;
            super::install::clear_active(&self.root, &request)?;
            return reply(result, transition.replayed);
        }
        let source = super::native::SourceDirectory::open(&enrollment.workspace)?;
        super::relocation::check_root(&self.root, &enrollment.registration, &source)?;
        self.stage("hash_source")?;
        let before = crate::source_fingerprint(source.path())?;
        if before != request.source_sha256 {
            bail!("enrolled workspace source changed before state transaction");
        }
        let commit_runtime = if let Operation::Commit { hook_receipt, .. } = &request.operation {
            let binding = enrollment
                .git
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("Git capability is not enrolled"))?;
            match (&binding.hook_policy, hook_receipt) {
                (None, None) => {}
                (Some(policy), Some(receipt))
                    if receipt.policy_sha256 == policy.digest()?
                        && receipt.source_sha256 == request.source_sha256
                        && receipt.exit_code == 0 => {}
                _ => bail!("sandbox hook receipt is missing or differs before admission"),
            }
            let identity = enrollment
                .commit_identity
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("commit identity is not enrolled"))?;
            Some((
                GitInspector::open_for_worker(
                    &enrollment.workspace,
                    binding,
                    &enrollment.registration,
                    &self.root,
                )?,
                identity,
            ))
        } else {
            None
        };
        if matches!(request.operation, Operation::Commit { .. }) {
            self.stage("retain_commit_request")?;
            retain_commit_request(&self.root, &request, bytes)?;
        }
        self.stage("record_intent")?;
        let transition = storage.accept(&request, &enrollment.registration, now)?;
        let mut result = transition.result;
        if matches!(request.operation, Operation::Commit { .. })
            && result == RecordedResult::RecoveryRequired
        {
            let (git, identity) = commit_runtime
                .ok_or_else(|| anyhow::anyhow!("commit runtime preflight missing"))?;
            if transition.execute_effect {
                self.stage("prepare_commit")?;
                git.prepare_commit(
                    &request,
                    &enrollment.registration,
                    now,
                    &identity.name,
                    &identity.email,
                )?;
            }
            // A replay can only recover the already prepared candidate; it
            // cannot silently generate a replacement from changed source.
            self.stage("publish_commit")?;
            let commit = if transition.replayed {
                git.recover_admitted_commit(&request, &enrollment.registration)?
            } else {
                git.publish_commit(&request, &enrollment.registration, now)?
            };
            result = RecordedResult::EffectSucceeded {
                outcome_sha256: sha256_bytes(&serde_json::to_vec(&commit)?),
            };
            self.stage("record_result")?;
            storage.record_effect_result(
                &request.request_id,
                &request.digest()?,
                result.clone(),
            )?;
        }
        // State progress is non-authoritative even when source was stable.
        // Goal-validation receipts are never synthesized by this handler.
        reply(result, transition.replayed)
    }

    /// Process a bounded batch. The queue is a fixed child of the attested
    /// root. Results are immutable and owned by the protected worker.
    pub fn cycle(&self, now: i64) -> Result<usize> {
        self.cycle_with_clock(|| now)
    }

    #[cfg(test)]
    pub(super) fn open_install_fixture(root: &Path, executable: &Path) -> Result<Self> {
        Self::open_with_executable(root, executable)
    }

    fn cycle_with_clock(&self, clock: impl Fn() -> i64) -> Result<usize> {
        if self.maintenance_requested()? {
            self.stage("maintenance")?;
            return Ok(0);
        }
        {
            let mut activity = self
                .activity
                .lock()
                .map_err(|_| anyhow::anyhow!("worker activity lock poisoned"))?;
            if activity.stage.as_deref() == Some("maintenance") {
                activity.stage = None;
            }
        }
        // One recovery attempt per service start, never an unbounded retry loop.
        if !self
            .recovery_checked
            .swap(true, std::sync::atomic::Ordering::AcqRel)
            && let Some(request) = super::install::pending_request(&self.root)?
        {
            let name = format!("project-{}.json", request.worktree_id);
            let enrollment: Enrollment =
                decode_json(&self.root.read_file(&name, MAX_REQUEST_BYTES as u64)?)?;
            enrollment.validate(&self.installation)?;
            let storage =
                StateStorage::at_existing_root(self.root.path(), &enrollment.registration)?;
            if storage
                .recorded_result(&request, &enrollment.registration)?
                .is_none()
            {
                super::install::cancel_unadmitted(&self.root, &request)?;
            } else {
                self.process(&serde_json::to_vec(&request)?, clock())?;
            }
        }
        let queue = super::native::SourceDirectory::open(&self.root.path().join("requests"))?;
        let mut names = Vec::new();
        let mut entry_count = 0;
        for entry in std::fs::read_dir(queue.path())?.take(4097) {
            entry_count += 1;
            let entry = entry?;
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if let Some(id) = name.strip_suffix(".request.json")
                && is_id(id)
            {
                names.push((id.to_owned(), name));
            }
        }
        if entry_count > 4096 {
            bail!("global queue capacity exceeded");
        }
        names.sort();
        let mut count = 0;
        let mut problems = Vec::new();
        for (id, name) in names {
            if count >= 32 {
                break;
            }
            let bytes = match queue.read_file(Path::new(&name), MAX_REQUEST_BYTES as u64) {
                Ok(bytes) => bytes,
                Err(error) => {
                    if problems.len() < 32 {
                        problems.push(
                            serde_json::json!({"request_id":id,"error":bounded_error(&error)}),
                        );
                    }
                    continue;
                }
            };
            let digest = match decode_request(&bytes) {
                Ok(request) => request.digest()?,
                Err(_) => sha256_bytes(&bytes),
            };
            let result_name = format!("result-{id}.json");
            let result_path = self.root.path().join(&result_name);
            if result_path.try_exists()? {
                let previous: QueueResult = super::decode_json(
                    &self
                        .root
                        .read_file(&result_name, MAX_REQUEST_BYTES as u64)?,
                )?;
                if previous.installation_id != self.installation.installation_id
                    || previous.request_id != id
                    || previous.request_sha256 != digest
                {
                    if problems.len() < 32 {
                        problems.push(serde_json::json!({"request_id":id,"error":"request ID reused with different queued bytes; original result preserved"}));
                    }
                } else if let Err(error) = queue.consume_packet(&name, &bytes)
                    && problems.len() < 32
                {
                    problems.push(serde_json::json!({"request_id":id,"error":format!("completed packet cleanup deferred: {}",bounded_error(&error))}));
                }
                continue;
            }
            {
                let mut activity = self
                    .activity
                    .lock()
                    .map_err(|_| anyhow::anyhow!("worker activity lock poisoned"))?;
                activity.request_id = Some(id.clone());
                activity.started_at = Some(clock());
                activity.stage = Some("validate_request".into());
            }
            let result = (|| {
                let request = decode_request(&bytes)?;
                if request.request_id != id {
                    bail!("request filename and body identity differ");
                }
                self.process(&bytes, clock())
            })();
            {
                let mut activity = self
                    .activity
                    .lock()
                    .map_err(|_| anyhow::anyhow!("worker activity lock poisoned"))?;
                activity.request_id = None;
                activity.started_at = None;
                activity.stage = None;
                activity.completed_requests = activity.completed_requests.saturating_add(1);
                activity.last_error = result.as_ref().err().map(bounded_error);
            }
            let (output, error) = match result {
                Ok(r) => (Some(serde_json::to_value(r)?), None),
                Err(e) => (None, Some(bounded_error(&e))),
            };
            let record = QueueResult {
                schema: "rayman.global-queue-result.v1".into(),
                installation_id: self.installation.installation_id.clone(),
                request_id: id,
                request_sha256: digest,
                executor_sid: self.installation.owner_sid.clone(),
                success: error.is_none(),
                output,
                error,
            };
            crate::file_io::write_json(&result_path, &record)?;
            // Durable results support later queries; consumed request packets
            // need not remain in the hot polling directory.
            if let Err(error) = queue.consume_packet(&name, &bytes)
                && problems.len() < 32
            {
                problems.push(serde_json::json!({"request_id":record.request_id,"error":format!("packet cleanup deferred: {}",bounded_error(&error))}));
            }
            count += 1;
        }
        let diagnostic =
            serde_json::json!({"schema":"rayman.global-queue-health.v1", "problems":problems});
        let diagnostic_path = self.root.path().join("queue-health.json");
        let encoded = serde_json::to_string_pretty(&diagnostic)?;
        // Stable invalid packets must not cause continuous disk writes.
        let same = diagnostic_path.try_exists()?
            && self
                .root
                .read_file("queue-health.json", MAX_REQUEST_BYTES as u64)?
                == encoded.as_bytes();
        if !same {
            crate::file_io::write_atomic(&diagnostic_path, &encoded)?;
        }
        Ok(count)
    }
}

pub fn serve(root: &Path, once: bool) -> Result<()> {
    super::native::require_unelevated_worker()?;
    let worker = Worker::open(root)?;
    let _lock = crate::state_lock::acquire_state_lock(&worker.root.path().join("worker"))?;
    let (stop, receiver) = std::sync::mpsc::channel();
    let worker = &worker;
    std::thread::scope(|scope| {
        let heartbeat=scope.spawn(move || -> Result<()> {
            loop {
                let activity=worker.activity.lock().map_err(|_|anyhow::anyhow!("worker activity lock poisoned"))?.clone();
                let record=serde_json::json!({"schema":"rayman.global-heartbeat.v1","installation_id":worker.installation.installation_id,"executor_sid":worker.installation.owner_sid,"observed_at":chrono::Utc::now().timestamp(),"pid":std::process::id(),"activity":activity,"error":activity.service_error,"maintenance":worker.maintenance_requested().unwrap_or(true)});
                crate::file_io::write_json(&worker.root.path().join("heartbeat.json"),&record)?;
                match receiver.recv_timeout(std::time::Duration::from_secs(2)) {
                    Ok(())|Err(std::sync::mpsc::RecvTimeoutError::Disconnected)=>return Ok(()),
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout)=>{},
                }
            }
        });
        let processing = (|| -> Result<()> {
            loop {
                if heartbeat.is_finished() {
                    bail!("global heartbeat writer stopped; service is not healthy");
                }
                let cycle = worker.cycle_with_clock(|| chrono::Utc::now().timestamp());
                let processed = match cycle {
                    Ok(count) => {
                        worker
                            .activity
                            .lock()
                            .map_err(|_| anyhow::anyhow!("worker activity lock poisoned"))?
                            .service_error = None;
                        count
                    }
                    Err(error) => {
                        worker
                            .activity
                            .lock()
                            .map_err(|_| anyhow::anyhow!("worker activity lock poisoned"))?
                            .service_error = Some(bounded_error(&error));
                        if once {
                            return Err(error);
                        }
                        std::thread::sleep(std::time::Duration::from_secs(1));
                        continue;
                    }
                };
                if once {
                    return Ok(());
                }
                std::thread::sleep(std::time::Duration::from_millis(if processed == 0 {
                    50
                } else {
                    5
                }));
            }
        })();
        let _ = stop.send(());
        heartbeat
            .join()
            .map_err(|_| anyhow::anyhow!("worker heartbeat thread panicked"))??;
        processing
    })
}

#[cfg(test)]
mod tests;
