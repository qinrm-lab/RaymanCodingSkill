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
mod tests {
    use super::*;
    #[cfg(windows)]
    #[test]
    fn linked_worktree_queue_requires_owner_policy_and_preserves_parent_authority() {
        let base = tempfile::tempdir().unwrap();
        let root = base.path().join("backend");
        let parent = base.path().join("parent");
        let allowed = base.path().join("allowed");
        for path in [&root, &parent, &allowed] {
            std::fs::create_dir(path).unwrap();
        }
        let program = std::path::PathBuf::from("C:/Program Files/Git/mingw64/bin/git.exe");
        let git = |cwd: &Path, args: &[&str]| {
            let output = std::process::Command::new(&program)
                .args(args)
                .current_dir(cwd)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        git(&parent, &["init", "-b", "main"]);
        git(&parent, &["config", "user.name", "Fixture"]);
        git(
            &parent,
            &["config", "user.email", "fixture@example.invalid"],
        );
        std::fs::write(parent.join("file.txt"), b"baseline\n").unwrap();
        git(&parent, &["add", "."]);
        git(
            &parent,
            &["-c", "commit.gpgsign=false", "commit", "-m", "baseline"],
        );
        let linked = allowed.join("工作树 含空格");
        git(
            &parent,
            &[
                "worktree",
                "add",
                "--detach",
                linked.to_str().unwrap(),
                "HEAD",
            ],
        );
        let sid = crate::execution_context::execution_context_probe()
            .principal_sid
            .unwrap();
        super::super::native::protect_fixture_directory(&root, &sid, "").unwrap();
        let pin = ProtectedDirectory::open(&root, &sid).unwrap();
        let binary = root.join("worker.exe");
        std::fs::write(&binary, b"worker fixture").unwrap();
        std::fs::write(root.join("client.exe"), b"client fixture").unwrap();
        std::fs::create_dir(root.join("requests")).unwrap();
        let installation = Installation {
            schema_version: 1,
            installation_id: "a".repeat(32),
            owner_sid: sid,
            root_identity: pin.identity().into(),
            worker_sha256: sha256_bytes(b"worker fixture"),
            client_sha256: sha256_bytes(b"client fixture"),
            source_project_id: "b".repeat(32),
        };
        crate::file_io::write_json(&root.join("installation.json"), &installation).unwrap();
        let mut anchor = enroll(
            &root,
            &parent,
            Some((&program, "Fixture", "fixture@example.invalid")),
            true,
            true,
        )
        .unwrap();
        anchor
            .install_policies
            .insert("fixture-product".into(), "c".repeat(64));
        anchor
            .registration
            .capabilities
            .install_adapters
            .insert("fixture-product".into());
        anchor.registration.policy_sha256 = anchor.policy_digest().unwrap();
        let anchor_path = root.join(format!("project-{}.json", anchor.registration.worktree_id));
        crate::file_io::write_json(&anchor_path, &anchor).unwrap();
        let anchor_bytes = std::fs::read(&anchor_path).unwrap();
        let parent_index = std::fs::read(parent.join(".git/index")).unwrap();
        let client = Client::open(&root).unwrap();
        assert!(
            client
                .prepare_linked_worktree_request(&linked, 1000)
                .is_err()
        );
        authorize_worktrees(&root, &parent, &allowed, true, false).unwrap();
        assert!(
            client
                .prepare_linked_worktree_request(&linked, 1000)
                .is_err()
        );
        authorize_worktrees(&root, &parent, &allowed, true, true).unwrap();
        let worker = Worker::open_with_executable(&root, &binary).unwrap();
        let (request, parent_enrollment) = client
            .prepare_linked_worktree_request(&linked, 1000)
            .unwrap();
        client
            .submit(&request, &parent_enrollment.registration, 1000)
            .unwrap();
        assert_eq!(worker.cycle(1000).unwrap(), 1);
        let result = client.result(&request).unwrap().unwrap();
        assert!(matches!(result.result, RecordedResult::Storage { .. }));
        let child = client.enrollment(&linked).unwrap();
        assert_ne!(
            child.registration.worktree_id,
            anchor.registration.worktree_id
        );
        assert_ne!(
            child.registration.root_identity,
            anchor.registration.root_identity
        );
        assert_eq!(
            child.registration.git_common_identity,
            anchor.registration.git_common_identity
        );
        assert!(child.registration.capabilities.install_adapters.is_empty());
        assert!(child.install_policies.is_empty());
        assert!(!linked.join(".RaymanCodingSkill").exists());
        assert!(!linked.join(".agent-checkpoints").exists());
        let project_path = root.join(format!("project-{}.json", child.registration.worktree_id));
        let project_bytes = std::fs::read(&project_path).unwrap();
        let (retry, _) = client
            .prepare_linked_worktree_request(&linked, 1001)
            .unwrap();
        worker
            .process(&serde_json::to_vec(&retry).unwrap(), 1001)
            .unwrap();
        assert_eq!(std::fs::read(&project_path).unwrap(), project_bytes);
        let member_path = root.join(format!(
            "worktree-member-{}.json",
            child.registration.worktree_id
        ));
        let member_bytes = std::fs::read(&member_path).unwrap();
        let mut interrupted: serde_json::Value = serde_json::from_slice(&member_bytes).unwrap();
        interrupted["registered"] = false.into();
        crate::file_io::write_json(&member_path, &interrupted).unwrap();
        worker
            .process(&serde_json::to_vec(&retry).unwrap(), 1001)
            .unwrap();
        assert_eq!(std::fs::read(&member_path).unwrap(), member_bytes);
        let impostor = allowed.join("impostor");
        std::fs::create_dir(&impostor).unwrap();
        std::fs::copy(linked.join(".git"), impostor.join(".git")).unwrap();
        assert!(
            client
                .prepare_linked_worktree_request(&impostor, 1002)
                .is_err()
        );
        let mut forged = retry.clone();
        if let Operation::EnrollLinkedWorktree {
            workspace,
            root_identity,
            ..
        } = &mut forged.operation
        {
            *workspace = impostor.clone();
            *root_identity = super::super::native::SourceDirectory::open(&impostor)
                .unwrap()
                .identity()
                .into();
        }
        assert!(
            worker
                .process(&serde_json::to_vec(&forged).unwrap(), 1002)
                .is_err()
        );
        let outside = base.path().join("outside");
        git(
            &parent,
            &[
                "worktree",
                "add",
                "--detach",
                outside.to_str().unwrap(),
                "HEAD",
            ],
        );
        assert!(
            client
                .prepare_linked_worktree_request(&outside, 1002)
                .is_err()
        );
        if let Operation::EnrollLinkedWorktree {
            workspace,
            root_identity,
            ..
        } = &mut forged.operation
        {
            *workspace = outside.clone();
            *root_identity = super::super::native::SourceDirectory::open(&outside)
                .unwrap()
                .identity()
                .into();
        }
        assert!(
            worker
                .process(&serde_json::to_vec(&forged).unwrap(), 1002)
                .is_err()
        );
        assert!(
            worker
                .process(&serde_json::to_vec(&retry).unwrap(), 9999)
                .is_err()
        );
        assert_eq!(std::fs::read(&anchor_path).unwrap(), anchor_bytes);
        assert_eq!(
            std::fs::read(parent.join(".git/index")).unwrap(),
            parent_index
        );
    }
    #[cfg(windows)]
    #[test]
    fn maintenance_preserves_queue_and_defers_startup_recovery() {
        let temp = tempfile::tempdir().unwrap();
        let sid = crate::execution_context::execution_context_probe()
            .principal_sid
            .unwrap();
        super::super::native::protect_fixture_directory(temp.path(), &sid, "").unwrap();
        let pin = ProtectedDirectory::open(temp.path(), &sid).unwrap();
        let binary = temp.path().join("worker.exe");
        std::fs::write(&binary, b"worker fixture").unwrap();
        std::fs::write(temp.path().join("client.exe"), b"client fixture").unwrap();
        let installation = Installation {
            schema_version: 1,
            installation_id: "a".repeat(32),
            owner_sid: sid,
            root_identity: pin.identity().into(),
            worker_sha256: sha256_bytes(b"worker fixture"),
            client_sha256: sha256_bytes(b"client fixture"),
            source_project_id: "b".repeat(32),
        };
        file_io::write_json(&temp.path().join("installation.json"), &installation).unwrap();
        let worker = Worker::open_with_executable(temp.path(), &binary).unwrap();
        std::fs::create_dir(temp.path().join("requests")).unwrap();
        let packet = temp
            .path()
            .join("requests")
            .join(format!("{}.request.json", "c".repeat(32)));
        std::fs::write(&packet, b"invalid test packet retained during maintenance").unwrap();
        let marker = temp.path().join("kernel-maintenance.json");
        file_io::write_json(&marker, &serde_json::json!({
            "schema":"rayman.global-kernel-maintenance.v1", "installation_id":installation.installation_id,
            "transaction_id":"d".repeat(32), "worker_sha256":[installation.worker_sha256]
        })).unwrap();
        file_io::write_json(&temp.path().join("heartbeat.json"), &serde_json::json!({
            "schema":"rayman.global-heartbeat.v1", "installation_id":installation.installation_id,
            "executor_sid":installation.owner_sid, "observed_at":chrono::Utc::now().timestamp(), "error":null
        })).unwrap();
        let client = Client::open(temp.path()).unwrap();
        assert_eq!(worker.cycle(1000).unwrap(), 0);
        assert!(
            !worker
                .recovery_checked
                .load(std::sync::atomic::Ordering::Acquire)
        );
        assert!(packet.exists());
        assert!(
            worker
                .process(b"{}", 1000)
                .unwrap_err()
                .to_string()
                .contains("maintenance")
        );
        assert_eq!(client.status().unwrap()["service_healthy"], false);
        let r = registration();
        let q = request(&r);
        assert!(
            client
                .submit(&q, &r, 1001)
                .unwrap_err()
                .to_string()
                .contains("maintenance")
        );

        std::fs::write(&marker, b"malformed marker").unwrap();
        assert!(worker.cycle(1000).is_err());
        assert!(packet.exists());
        std::fs::remove_file(marker).unwrap();
        assert_eq!(worker.cycle(1000).unwrap(), 1);
        assert!(!packet.exists());
        assert!(
            worker
                .recovery_checked
                .load(std::sync::atomic::Ordering::Acquire)
        );
    }
    #[cfg(windows)]
    #[test]
    fn commit_request_archive_preserves_original_bytes_and_refuses_substitution() {
        let temp = tempfile::tempdir().unwrap();
        let sid = crate::execution_context::execution_context_probe()
            .principal_sid
            .unwrap();
        super::super::native::protect_fixture_directory(temp.path(), &sid, "").unwrap();
        let root = ProtectedDirectory::open(temp.path(), &sid).unwrap();
        let registration = super::super::tests::registration();
        let request = super::super::tests::request(&registration);
        let original = serde_json::to_vec_pretty(&request).unwrap();
        retain_commit_request(&root, &request, &original).unwrap();
        let name = format!("commit-request-{}.json", request.request_id);
        assert_eq!(
            root.read_file(&name, MAX_REQUEST_BYTES as u64).unwrap(),
            original
        );
        retain_commit_request(&root, &request, &serde_json::to_vec(&request).unwrap()).unwrap();
        assert_eq!(
            root.read_file(&name, MAX_REQUEST_BYTES as u64).unwrap(),
            original
        );
        let mut changed = request.clone();
        changed.source_sha256 = "f".repeat(64);
        assert!(
            retain_commit_request(&root, &changed, &serde_json::to_vec(&changed).unwrap()).is_err()
        );
        assert!(
            retain_commit_request(&root, &request, &serde_json::to_vec(&changed).unwrap()).is_err()
        );
        assert_eq!(
            root.read_file(&name, MAX_REQUEST_BYTES as u64).unwrap(),
            original
        );
    }
    use crate::file_io;
    use crate::global_execution::tests::{registration, request};

    fn checkpoint_client_roundtrip(worker: &Worker, workspace: &Path) {
        let copies = tempfile::tempdir().unwrap();
        let before = copies.path().join("before.sqlite3");
        let after = copies.path().join("after.sqlite3");
        let stop = std::sync::atomic::AtomicBool::new(false);
        std::thread::scope(|scope| {
            let server = scope.spawn(|| {
                let deadline = std::time::Instant::now();
                while !stop.load(std::sync::atomic::Ordering::Acquire)
                    && deadline.elapsed().as_secs() < 45
                {
                    worker.cycle(chrono::Utc::now().timestamp())?;
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                Ok::<_, anyhow::Error>(())
            });
            let result = (|| -> Result<()> {
                let client = Client::open(worker.root.path())?;
                let enrollment = client.enrollment(workspace)?;
                let lease = client.storage_call(
                    &enrollment,
                    StorageAction::Acquire {
                        object: StateObject::Checkpoints,
                    },
                )?;
                let lease = lease["lease_id"].as_str().unwrap();
                client.copy_checkpoint(&enrollment, &before)?;
                client.copy_checkpoint(&enrollment, &after)?;
                assert!(client.copy_checkpoint(&enrollment, &before).is_err());
                let changed = rusqlite::Connection::open(&after)?;
                let id = super::super::application_state::checkpoint_workspace_id(&enrollment);
                let path = crate::pathfmt::display_path(&enrollment.workspace);
                changed.execute(
                    "INSERT INTO workspaces VALUES(?1,?2,1,NULL,60,'{}','now','now',NULL)",
                    [&id, &path],
                )?;
                let archive = b"binary archive\0\r\n\n".repeat(70000);
                changed.execute("INSERT INTO checkpoints VALUES(?1,'checkpoint','session','now','codex','{}',?2,'archive.zip',?3,?4)",rusqlite::params![&id,b"diff\r\n\n",&archive,sha256_bytes(&archive)])?;
                changed.execute(
                    "UPDATE session_intents SET session_json='updated' WHERE workspace_id=?1",
                    [&id],
                )?;
                drop(changed);
                let tx_id = "8".repeat(32);
                let receipt =
                    client.apply_checkpoint_copies(&enrollment, &before, &after, lease, &tx_id)?;
                assert_eq!(receipt["applied"], true);
                assert_eq!(receipt["replayed"], false);
                let replay =
                    client.apply_checkpoint_copies(&enrollment, &before, &after, lease, &tx_id)?;
                assert_eq!(replay["replayed"], true);
                let copied = copies.path().join("observed.sqlite3");
                client.copy_checkpoint(&enrollment, &copied)?;
                let observed = rusqlite::Connection::open(copied)?;
                let bytes: Vec<u8> = observed.query_row(
                    "SELECT archive_blob FROM checkpoints WHERE workspace_id=?1",
                    [&id],
                    |row| row.get(0),
                )?;
                assert_eq!(bytes, archive);
                assert_eq!(
                    observed.query_row(
                        "SELECT session_json FROM session_intents WHERE workspace_id=?1",
                        [&id],
                        |row| row.get::<_, String>(0)
                    )?,
                    "updated"
                );
                client.storage_call(
                    &enrollment,
                    StorageAction::Release {
                        lease_id: lease.into(),
                    },
                )?;
                Ok(())
            })();
            stop.store(true, std::sync::atomic::Ordering::Release);
            server.join().unwrap().unwrap();
            result.unwrap();
        });
    }

    #[cfg(windows)]
    #[test]
    fn recovery_queue_preserves_failed_result_and_uses_retained_original_request() {
        let root = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let git_program = std::path::PathBuf::from("C:/Program Files/Git/mingw64/bin/git.exe");
        let git = |args: &[&str]| {
            let out = std::process::Command::new(&git_program)
                .args(args)
                .current_dir(workspace.path())
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            out.stdout
        };
        git(&["init", "-b", "main"]);
        git(&["config", "user.name", "Fixture"]);
        git(&["config", "user.email", "fixture@example.invalid"]);
        std::fs::write(workspace.path().join("tracked.txt"), b"old\n").unwrap();
        git(&["add", "."]);
        git(&["-c", "commit.gpgsign=false", "commit", "-m", "initial"]);
        std::fs::write(workspace.path().join("added.txt"), b"new\n").unwrap();
        let sid = crate::execution_context::execution_context_probe()
            .principal_sid
            .unwrap();
        super::super::native::protect_fixture_directory(root.path(), &sid, "").unwrap();
        let pin = ProtectedDirectory::open(root.path(), &sid).unwrap();
        let binary = root.path().join("worker.exe");
        std::fs::write(&binary, b"test worker").unwrap();
        let installation = Installation {
            source_project_id: "9".repeat(32),
            schema_version: 1,
            installation_id: "a".repeat(32),
            owner_sid: sid.clone(),
            root_identity: pin.identity().into(),
            worker_sha256: sha256_bytes(b"test worker"),
            client_sha256: sha256_bytes(b"test client"),
        };
        file_io::write_json(&root.path().join("installation.json"), &installation).unwrap();
        let mut registration = registration();
        registration.owner_sid = sid;
        registration.root_identity = super::super::native::SourceDirectory::open(workspace.path())
            .unwrap()
            .identity()
            .into();
        let git_dir = workspace.path().join(".git");
        registration.git_common_identity = super::super::native::SourceDirectory::open(&git_dir)
            .unwrap()
            .identity()
            .into();
        let binding = GitBinding {
            content_policy: GitContentPolicy::capture(&git_program, workspace.path()).unwrap(),
            hook_policy: None,
            executable: git_program.clone(),
            executable_sha256: crate::hash::sha256_file(&git_program).unwrap(),
            git_directory: git_dir.clone(),
            common_directory: git_dir.clone(),
            branch_ref: "refs/heads/main".into(),
            config_sha256: crate::hash::sha256_file(&git_dir.join("config")).unwrap(),
        };
        let inspector =
            GitInspector::open(workspace.path(), &binding, &registration, &pin).unwrap();
        let snapshot = inspector.capture(&registration).unwrap();
        drop(inspector);
        let mut enrollment = Enrollment {
            schema_version: 1,
            installation_id: installation.installation_id.clone(),
            registration: registration.clone(),
            workspace: workspace.path().to_path_buf(),
            source_policy: SourcePolicy::WorkspaceFingerprintV1,
            git: Some(binding),
            commit_identity: Some(CommitIdentity {
                name: "Fixture".into(),
                email: "fixture@example.invalid".into(),
            }),
            install_policies: Default::default(),
        };
        enrollment
            .registration
            .capabilities
            .install_adapters
            .clear();
        enrollment.registration.policy_sha256 = enrollment.policy_digest().unwrap();
        registration = enrollment.registration.clone();
        file_io::write_json(
            &root
                .path()
                .join(format!("project-{}.json", registration.worktree_id)),
            &enrollment,
        )
        .unwrap();
        let worker = Worker::open_with_executable(root.path(), &binary).unwrap();
        assert!(Worker::open(root.path()).is_err());
        let mut request = request(&registration);
        request.source_sha256 = crate::source_fingerprint(workspace.path()).unwrap();
        request.operation = Operation::Commit {
            expected_head: snapshot.head.clone(),
            expected_index_sha256: snapshot.index_sha256.clone(),
            message: "global exact commit".into(),
            changes: snapshot.changes,
            hook_receipt: None,
        };
        let bytes = serde_json::to_vec(&request).unwrap();

        std::fs::write(root.path().join("client.exe"), b"test client").unwrap();
        std::fs::create_dir(root.path().join("requests")).unwrap();
        let client = Client::open(root.path()).unwrap();
        let foreign_lock = git_dir.join("index.lock");
        std::fs::write(&foreign_lock, b"foreign writer").unwrap();
        client.submit(&request, &registration, 1001).unwrap();
        assert_eq!(worker.cycle(1001).unwrap(), 1);
        let failure_path = root
            .path()
            .join(format!("result-{}.json", request.request_id));
        let failure = std::fs::read(&failure_path).unwrap();
        assert!(
            !serde_json::from_slice::<QueueResult>(&failure)
                .unwrap()
                .success
        );
        assert!(
            !root
                .path()
                .join("requests")
                .join(format!("{}.request.json", request.request_id))
                .exists()
        );
        assert_eq!(
            std::fs::read(
                root.path()
                    .join(format!("commit-request-{}.json", request.request_id))
            )
            .unwrap(),
            bytes
        );
        assert!(client.submit(&request, &registration, 1002).is_err());
        std::fs::remove_file(foreign_lock).unwrap();
        let recovery = client
            .prepare_commit_recovery_request(workspace.path(), &request.request_id, None, 2000)
            .unwrap();
        client.submit(&recovery, &registration, 2000).unwrap();
        assert_eq!(worker.cycle(2000).unwrap(), 1);
        let result = client.result(&recovery).unwrap().unwrap();
        assert!(matches!(
            result.result,
            RecordedResult::EffectSucceeded { .. }
        ));
        assert_eq!(std::fs::read(&failure_path).unwrap(), failure);
        let candidate: CommitCandidate = file_io::read_json(
            &root
                .path()
                .join(format!("candidate-{}/candidate.json", request.request_id)),
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            String::from_utf8(git(&["rev-parse", "HEAD"]))
                .unwrap()
                .trim(),
            candidate.commit
        );
        assert_eq!(
            std::fs::read(workspace.path().join("added.txt")).unwrap(),
            b"new\n"
        );
    }

    #[cfg(windows)]
    #[test]
    fn legacy_queued_recovery_verifies_candidate_without_fabricating_request() {
        let root = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let git_program = std::path::PathBuf::from("C:/Program Files/Git/mingw64/bin/git.exe");
        let git = |args: &[&str]| {
            let out = std::process::Command::new(&git_program)
                .args(args)
                .current_dir(workspace.path())
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            out.stdout
        };
        git(&["init", "-b", "main"]);
        git(&["config", "user.name", "Fixture"]);
        git(&["config", "user.email", "fixture@example.invalid"]);
        std::fs::write(workspace.path().join("tracked.txt"), b"old\n").unwrap();
        git(&["add", "."]);
        git(&["-c", "commit.gpgsign=false", "commit", "-m", "initial"]);
        std::fs::write(workspace.path().join("added.txt"), b"new\n").unwrap();
        let sid = crate::execution_context::execution_context_probe()
            .principal_sid
            .unwrap();
        super::super::native::protect_fixture_directory(root.path(), &sid, "").unwrap();
        let pin = ProtectedDirectory::open(root.path(), &sid).unwrap();
        let binary = root.path().join("worker.exe");
        std::fs::write(&binary, b"test worker").unwrap();
        let installation = Installation {
            source_project_id: "9".repeat(32),
            schema_version: 1,
            installation_id: "a".repeat(32),
            owner_sid: sid.clone(),
            root_identity: pin.identity().into(),
            worker_sha256: sha256_bytes(b"test worker"),
            client_sha256: sha256_bytes(b"test client"),
        };
        file_io::write_json(&root.path().join("installation.json"), &installation).unwrap();
        let mut registration = registration();
        registration.owner_sid = sid;
        registration.root_identity = super::super::native::SourceDirectory::open(workspace.path())
            .unwrap()
            .identity()
            .into();
        let git_dir = workspace.path().join(".git");
        registration.git_common_identity = super::super::native::SourceDirectory::open(&git_dir)
            .unwrap()
            .identity()
            .into();
        let binding = GitBinding {
            content_policy: GitContentPolicy::capture(&git_program, workspace.path()).unwrap(),
            hook_policy: None,
            executable: git_program.clone(),
            executable_sha256: crate::hash::sha256_file(&git_program).unwrap(),
            git_directory: git_dir.clone(),
            common_directory: git_dir.clone(),
            branch_ref: "refs/heads/main".into(),
            config_sha256: crate::hash::sha256_file(&git_dir.join("config")).unwrap(),
        };
        let inspector =
            GitInspector::open(workspace.path(), &binding, &registration, &pin).unwrap();
        let snapshot = inspector.capture(&registration).unwrap();
        drop(inspector);
        let mut enrollment = Enrollment {
            schema_version: 1,
            installation_id: installation.installation_id.clone(),
            registration: registration.clone(),
            workspace: workspace.path().to_path_buf(),
            source_policy: SourcePolicy::WorkspaceFingerprintV1,
            git: Some(binding),
            commit_identity: Some(CommitIdentity {
                name: "Fixture".into(),
                email: "fixture@example.invalid".into(),
            }),
            install_policies: Default::default(),
        };
        enrollment
            .registration
            .capabilities
            .install_adapters
            .clear();
        enrollment.registration.policy_sha256 = enrollment.policy_digest().unwrap();
        registration = enrollment.registration.clone();
        file_io::write_json(
            &root
                .path()
                .join(format!("project-{}.json", registration.worktree_id)),
            &enrollment,
        )
        .unwrap();
        let worker = Worker::open_with_executable(root.path(), &binary).unwrap();
        assert!(Worker::open(root.path()).is_err());
        let mut request = request(&registration);
        request.source_sha256 = crate::source_fingerprint(workspace.path()).unwrap();
        request.operation = Operation::Commit {
            expected_head: snapshot.head.clone(),
            expected_index_sha256: snapshot.index_sha256.clone(),
            message: "global exact commit".into(),
            changes: snapshot.changes,
            hook_receipt: None,
        };

        std::fs::write(root.path().join("client.exe"), b"test client").unwrap();
        std::fs::create_dir(root.path().join("requests")).unwrap();
        let client = Client::open(root.path()).unwrap();
        // Construct a v1-style retained snapshot through real admission and
        // candidate preparation, deliberately without the new request archive.
        let storage = StateStorage::at_existing_root(root.path(), &registration).unwrap();
        storage.accept(&request, &registration, 1001).unwrap();
        let inspector = GitInspector::open_for_worker(
            workspace.path(),
            enrollment.git.as_ref().unwrap(),
            &registration,
            &pin,
        )
        .unwrap();
        let candidate = inspector
            .prepare_commit(
                &request,
                &registration,
                1001,
                "Fixture",
                "fixture@example.invalid",
            )
            .unwrap();
        assert!(
            inspector
                .test_publish_cut(&request, &registration, "ref_seed_created")
                .is_err()
        );
        drop(inspector);
        let directory = root
            .path()
            .join(format!("candidate-{}", request.request_id));
        std::fs::remove_file(directory.join("staging-attempts.json")).unwrap();
        let mut orphans = Vec::new();
        for (parent, prefix) in [
            (
                git_dir.clone(),
                format!(".rayman-index-{}", request.request_id),
            ),
            (
                git_dir.join("refs/heads"),
                format!(".rayman-ref-{}", request.request_id),
            ),
        ] {
            let entry = std::fs::read_dir(&parent)
                .unwrap()
                .map(Result::unwrap)
                .find(|e| e.file_name().to_string_lossy().starts_with(&prefix))
                .unwrap();
            let old_name = parent.join(prefix);
            std::fs::rename(entry.path(), &old_name).unwrap();
            orphans.push((old_name.clone(), std::fs::read(old_name).unwrap()));
        }
        let failure_path = root
            .path()
            .join(format!("result-{}.json", request.request_id));
        file_io::write_json(
            &failure_path,
            &QueueResult {
                schema: "rayman.global-queue-result.v1".into(),
                installation_id: installation.installation_id.clone(),
                request_id: request.request_id.clone(),
                request_sha256: request.digest().unwrap(),
                executor_sid: installation.owner_sid.clone(),
                success: false,
                output: None,
                error: Some(
                    "v1 fixture: metadata preservation failed before journal creation".into(),
                ),
            },
        )
        .unwrap();
        let failure = std::fs::read(&failure_path).unwrap();
        let candidate_path = directory.join("candidate.json");
        let candidate_bytes = std::fs::read(&candidate_path).unwrap();
        let mut altered: serde_json::Value = serde_json::from_slice(&candidate_bytes).unwrap();
        altered["paths"] = serde_json::json!(["tracked.txt"]);
        file_io::write_json(&candidate_path, &altered).unwrap();
        assert!(
            client
                .prepare_commit_recovery_request(workspace.path(), &request.request_id, None, 2000)
                .is_err()
        );
        // A caller cannot bypass the worker's checks by skipping client preview.
        let rejected = client
            .request(
                &enrollment,
                Operation::RecoverCommit {
                    original_request_id: request.request_id.clone(),
                    candidate_sha256: crate::hash::sha256_file(&candidate_path).unwrap(),
                },
                2000,
            )
            .unwrap();
        client.submit(&rejected, &registration, 2000).unwrap();
        assert_eq!(worker.cycle(2000).unwrap(), 1);
        assert!(client.result(&rejected).is_err());
        assert!(!directory.join("legacy-recovery.json").exists());
        assert!(!directory.join("publication.json").exists());
        std::fs::write(&candidate_path, &candidate_bytes).unwrap();
        let foreign = git_dir.join("index.lock");
        std::fs::write(&foreign, b"fixture external writer").unwrap();
        let interrupted = client
            .prepare_commit_recovery_request(workspace.path(), &request.request_id, None, 2001)
            .unwrap();
        assert!(
            !directory.join("legacy-recovery.json").exists(),
            "preview must not persist acceptance"
        );
        assert!(
            !directory.join("publication.json").exists(),
            "preview must not start publication"
        );
        client.submit(&interrupted, &registration, 2001).unwrap();
        assert_eq!(worker.cycle(2001).unwrap(), 1);
        assert!(client.result(&interrupted).is_err());
        assert!(directory.join("legacy-recovery.json").exists());
        assert!(directory.join("publication.json").exists());
        assert_eq!(std::fs::read(&foreign).unwrap(), b"fixture external writer");
        // The fixture's external writer, not the recovery worker, releases it.
        std::fs::remove_file(foreign).unwrap();
        let recovery = client
            .prepare_commit_recovery_request(workspace.path(), &request.request_id, None, 2002)
            .unwrap();
        client.submit(&recovery, &registration, 2002).unwrap();
        assert_eq!(worker.cycle(2002).unwrap(), 1);
        assert!(matches!(
            client.result(&recovery).unwrap().unwrap().result,
            RecordedResult::EffectSucceeded { .. }
        ));
        assert_eq!(std::fs::read(&failure_path).unwrap(), failure);
        assert!(
            !root
                .path()
                .join(format!("commit-request-{}.json", request.request_id))
                .exists(),
            "must not fabricate the missing original request"
        );
        assert_eq!(
            String::from_utf8(git(&["rev-parse", "HEAD"]))
                .unwrap()
                .trim(),
            candidate.commit
        );
        for (path, bytes) in orphans {
            assert_eq!(std::fs::read(path).unwrap(), bytes);
        }
    }

    #[cfg(windows)]
    #[test]
    fn protected_worker_commits_exact_registered_source_and_replays_after_expiry() {
        let root = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let git_program = std::path::PathBuf::from("C:/Program Files/Git/mingw64/bin/git.exe");
        let git = |args: &[&str]| {
            let out = std::process::Command::new(&git_program)
                .args(args)
                .current_dir(workspace.path())
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            out.stdout
        };
        git(&["init", "-b", "main"]);
        git(&["config", "user.name", "Fixture"]);
        git(&["config", "user.email", "fixture@example.invalid"]);
        std::fs::write(workspace.path().join("tracked.txt"), b"old\n").unwrap();
        git(&["add", "."]);
        git(&["-c", "commit.gpgsign=false", "commit", "-m", "initial"]);
        std::fs::write(workspace.path().join("added.txt"), b"new\n").unwrap();
        let sid = crate::execution_context::execution_context_probe()
            .principal_sid
            .unwrap();
        super::super::native::protect_fixture_directory(root.path(), &sid, "").unwrap();
        let pin = ProtectedDirectory::open(root.path(), &sid).unwrap();
        let binary = root.path().join("worker.exe");
        std::fs::write(&binary, b"test worker").unwrap();
        let installation = Installation {
            source_project_id: "9".repeat(32),
            schema_version: 1,
            installation_id: "a".repeat(32),
            owner_sid: sid.clone(),
            root_identity: pin.identity().into(),
            worker_sha256: sha256_bytes(b"test worker"),
            client_sha256: sha256_bytes(b"test client"),
        };
        file_io::write_json(&root.path().join("installation.json"), &installation).unwrap();
        let mut registration = registration();
        registration.owner_sid = sid;
        registration.root_identity = super::super::native::SourceDirectory::open(workspace.path())
            .unwrap()
            .identity()
            .into();
        let git_dir = workspace.path().join(".git");
        registration.git_common_identity = super::super::native::SourceDirectory::open(&git_dir)
            .unwrap()
            .identity()
            .into();
        let binding = GitBinding {
            content_policy: GitContentPolicy::capture(&git_program, workspace.path()).unwrap(),
            hook_policy: None,
            executable: git_program.clone(),
            executable_sha256: crate::hash::sha256_file(&git_program).unwrap(),
            git_directory: git_dir.clone(),
            common_directory: git_dir.clone(),
            branch_ref: "refs/heads/main".into(),
            config_sha256: crate::hash::sha256_file(&git_dir.join("config")).unwrap(),
        };
        let inspector =
            GitInspector::open(workspace.path(), &binding, &registration, &pin).unwrap();
        let snapshot = inspector.capture(&registration).unwrap();
        drop(inspector);
        let mut enrollment = Enrollment {
            schema_version: 1,
            installation_id: installation.installation_id.clone(),
            registration: registration.clone(),
            workspace: workspace.path().to_path_buf(),
            source_policy: SourcePolicy::WorkspaceFingerprintV1,
            git: Some(binding),
            commit_identity: Some(CommitIdentity {
                name: "Fixture".into(),
                email: "fixture@example.invalid".into(),
            }),
            install_policies: Default::default(),
        };
        enrollment
            .registration
            .capabilities
            .install_adapters
            .clear();
        enrollment.registration.policy_sha256 = enrollment.policy_digest().unwrap();
        registration = enrollment.registration.clone();
        file_io::write_json(
            &root
                .path()
                .join(format!("project-{}.json", registration.worktree_id)),
            &enrollment,
        )
        .unwrap();
        let mut worker = Worker::open_with_executable(root.path(), &binary).unwrap();
        assert!(Worker::open(root.path()).is_err());
        let mut request = request(&registration);
        request.source_sha256 = crate::source_fingerprint(workspace.path()).unwrap();
        request.operation = Operation::Commit {
            expected_head: snapshot.head.clone(),
            expected_index_sha256: snapshot.index_sha256.clone(),
            message: "global exact commit".into(),
            changes: snapshot.changes,
            hook_receipt: None,
        };
        let bytes = serde_json::to_vec(&request).unwrap();
        let original_source = worker.installation.source_project_id.clone();
        worker.installation.source_project_id = registration.project_id.clone();
        assert!(
            worker
                .process(&bytes, 1000)
                .unwrap_err()
                .to_string()
                .contains("own installation source")
        );
        worker.installation.source_project_id = original_source;
        let outcome = worker.process(&bytes, 1001).unwrap();
        assert!(matches!(
            outcome.result,
            RecordedResult::EffectSucceeded { .. }
        ));
        let actual = String::from_utf8(git(&["rev-parse", "HEAD"])).unwrap();
        assert_ne!(actual.trim(), snapshot.head);
        assert!(git(&["status", "--porcelain"]).is_empty());
        let replay = worker.process(&bytes, 99999).unwrap();
        assert!(replay.replayed);
        assert_eq!(replay.result, outcome.result);
        assert_eq!(
            String::from_utf8(git(&["rev-parse", "HEAD"])).unwrap(),
            actual
        );
    }

    #[cfg(windows)]
    #[test]
    fn protected_worker_processes_enrolled_state_and_refuses_forged_registration() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let sid = crate::execution_context::execution_context_probe()
            .principal_sid
            .unwrap();
        super::super::native::protect_fixture_directory(temp.path(), &sid, "").unwrap();
        let pin = ProtectedDirectory::open(temp.path(), &sid).unwrap();
        let binary = temp.path().join("worker.exe");
        std::fs::write(&binary, b"worker fixture").unwrap();
        let install = Installation {
            source_project_id: "9".repeat(32),
            schema_version: 1,
            installation_id: "a".repeat(32),
            owner_sid: sid.clone(),
            root_identity: pin.identity().into(),
            worker_sha256: sha256_bytes(b"worker fixture"),
            client_sha256: sha256_bytes(b"test client"),
        };
        file_io::write_json(&temp.path().join("installation.json"), &install).unwrap();
        let mut r = registration();
        r.owner_sid = sid;
        r.capabilities.local_commit = false;
        r.capabilities.added_files = false;
        r.capabilities.deleted_files = false;
        r.root_identity = super::super::native::SourceDirectory::open(workspace.path())
            .unwrap()
            .identity()
            .into();
        let mut enrollment = Enrollment {
            schema_version: 1,
            installation_id: install.installation_id.clone(),
            registration: r.clone(),
            workspace: workspace.path().to_path_buf(),
            source_policy: SourcePolicy::WorkspaceFingerprintV1,
            git: None,
            commit_identity: None,
            install_policies: Default::default(),
        };
        enrollment
            .registration
            .capabilities
            .install_adapters
            .clear();
        enrollment.registration.policy_sha256 = enrollment.policy_digest().unwrap();
        r = enrollment.registration.clone();
        file_io::write_json(
            &temp.path().join(format!("project-{}.json", r.worktree_id)),
            &enrollment,
        )
        .unwrap();
        let worker = Worker::open_with_executable(temp.path(), &binary).unwrap();
        let mut q = request(&r);
        q.source_sha256 = crate::source_fingerprint(workspace.path()).unwrap();
        q.operation = Operation::State {
            task_id: "e".repeat(32),
            expected_revision: 0,
            mutation: StateMutation::Begin {
                title: "actual worker state".into(),
            },
        };
        let first = worker
            .process(&serde_json::to_vec(&q).unwrap(), 1000)
            .unwrap();
        assert!(!first.replayed);
        assert_eq!(
            first.result,
            RecordedResult::StateApplied { task_revision: 1 }
        );
        assert!(
            worker
                .process(&serde_json::to_vec(&q).unwrap(), 1001)
                .unwrap()
                .replayed
        );
        q.registration_sha256 = "f".repeat(64);
        assert!(
            worker
                .process(&serde_json::to_vec(&q).unwrap(), 1001)
                .is_err()
        );
        // Multiple batches must progress past already-completed requests.
        let queue = temp.path().join("requests");
        std::fs::create_dir(&queue).unwrap();
        q.registration_sha256 = r.digest().unwrap();
        for index in 1..=34 {
            q.request_id = format!("{index:032x}");
            q.operation = Operation::State {
                task_id: format!("{index:032x}"),
                expected_revision: 0,
                mutation: StateMutation::Begin {
                    title: format!("task {index}"),
                },
            };
            file_io::write_json(&queue.join(format!("{}.request.json", q.request_id)), &q).unwrap();
        }
        assert_eq!(worker.cycle(1001).unwrap(), 32);
        assert_eq!(worker.cycle(1001).unwrap(), 2);
        assert_eq!(worker.cycle(1001).unwrap(), 0);
        // Invalid entries sorted before valid work do not terminate the worker
        // or consume its 32-request processing allowance.
        let reused = "0".repeat(31) + "1";
        std::fs::write(
            queue.join(format!("{reused}.request.json")),
            b"different bytes",
        )
        .unwrap();
        let unreadable = "0".repeat(32);
        std::fs::create_dir(queue.join(format!("{unreadable}.request.json"))).unwrap();
        let malformed = "d".repeat(32);
        std::fs::write(queue.join(format!("{malformed}.request.json")), b"{broken").unwrap();
        q.request_id = "c".repeat(32);
        q.operation = Operation::State {
            task_id: "c".repeat(32),
            expected_revision: 0,
            mutation: StateMutation::Begin {
                title: "after malformed packets".into(),
            },
        };
        file_io::write_json(&queue.join(format!("{}.request.json", q.request_id)), &q).unwrap();
        assert_eq!(worker.cycle(1001).unwrap(), 2);
        let invalid: QueueResult = super::super::decode_json(
            &worker
                .root
                .read_file(
                    &format!("result-{malformed}.json"),
                    MAX_REQUEST_BYTES as u64,
                )
                .unwrap(),
        )
        .unwrap();
        assert!(!invalid.success);
        let valid: QueueResult = super::super::decode_json(
            &worker
                .root
                .read_file(
                    &format!("result-{}.json", q.request_id),
                    MAX_REQUEST_BYTES as u64,
                )
                .unwrap(),
        )
        .unwrap();
        assert!(valid.success, "{:?}", valid.error);
        assert_eq!(worker.cycle(1001).unwrap(), 0);
        assert!(queue.join(format!("{reused}.request.json")).is_file());
        let health = worker
            .root
            .read_file("queue-health.json", MAX_REQUEST_BYTES as u64)
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&health).unwrap()["problems"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        let client = Client::open(temp.path()).unwrap();
        let selected = client.enrollment(workspace.path()).unwrap();
        let client_request = client
            .request(
                &selected,
                Operation::State {
                    task_id: "0".repeat(32),
                    expected_revision: 0,
                    mutation: StateMutation::Begin {
                        title: "client roundtrip".into(),
                    },
                },
                1001,
            )
            .unwrap();
        client.submit(&client_request, &r, 1001).unwrap();
        assert!(client.result(&client_request).unwrap().is_none());
        assert_eq!(worker.cycle(1001).unwrap(), 1);
        let reply = client
            .wait(&client_request, std::time::Duration::from_secs(1), |_| {})
            .unwrap();
        assert_eq!(
            reply.result,
            RecordedResult::StateApplied { task_revision: 1 }
        );
        // A final result name can be visible before a publishing handle has
        // closed. Force that exact Windows sharing window deterministically.
        {
            use std::os::windows::fs::OpenOptionsExt;
            use windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ;
            let held = std::fs::OpenOptions::new()
                .write(true)
                .share_mode(FILE_SHARE_READ)
                .open(
                    temp.path()
                        .join(format!("result-{}.json", client_request.request_id)),
                )
                .unwrap();
            let error = client.result(&client_request).unwrap_err();
            assert_eq!(
                error
                    .downcast_ref::<std::io::Error>()
                    .unwrap()
                    .raw_os_error(),
                Some(32)
            );
            assert!(
                client
                    .wait(
                        &client_request,
                        std::time::Duration::from_millis(40),
                        |_| {}
                    )
                    .unwrap_err()
                    .to_string()
                    .contains("global result wait expired")
            );
            std::thread::scope(|scope| {
                let waiting = scope.spawn(|| {
                    client.wait(&client_request, std::time::Duration::from_secs(1), |_| {})
                });
                std::thread::sleep(std::time::Duration::from_millis(100));
                drop(held);
                assert_eq!(
                    waiting.join().unwrap().unwrap().result,
                    RecordedResult::StateApplied { task_revision: 1 }
                );
            });
            let result_path = temp
                .path()
                .join(format!("result-{}.json", client_request.request_id));
            let original = std::fs::read(&result_path).unwrap();
            std::fs::write(&result_path, b"{}").unwrap();
            assert!(
                client
                    .wait(
                        &client_request,
                        std::time::Duration::from_millis(40),
                        |_| {}
                    )
                    .unwrap_err()
                    .to_string()
                    .contains("missing field")
            );
            std::fs::write(result_path, original).unwrap();
        }
        assert!(client.submit(&client_request, &r, 1001).is_err());
        let mut storage = request(&r);
        storage.source_sha256 = "0".repeat(64);
        storage.operation = Operation::Storage {
            action: StorageAction::Acquire {
                object: StateObject::GoalsStore,
            },
        };
        let locked = worker
            .process(&serde_json::to_vec(&storage).unwrap(), 1001)
            .unwrap();
        let RecordedResult::Storage { details } = locked.result else {
            panic!("storage result missing")
        };
        let lease = details["lease_id"].as_str().unwrap().to_string();
        let body =
            b"{\"id\":\"goal_0123456789\",\"note\":\"stored data is not validation authority\"}";
        let sha = sha256_bytes(body);
        storage.operation = Operation::Storage {
            action: StorageAction::Chunk {
                object_sha256: sha.clone(),
                index: 0,
                total_bytes: body.len() as u64,
                hex: body.iter().map(|b| format!("{b:02x}")).collect(),
            },
        };
        worker
            .process(&serde_json::to_vec(&storage).unwrap(), 1001)
            .unwrap();
        storage.operation = Operation::Storage {
            action: StorageAction::Write {
                object: StateObject::Goal {
                    id: "goal_0123456789".into(),
                },
                lease_id: lease.clone(),
                expected_sha256: None,
                object_sha256: sha,
                total_bytes: body.len() as u64,
            },
        };
        worker
            .process(&serde_json::to_vec(&storage).unwrap(), 1001)
            .unwrap();
        assert_eq!(
            std::fs::read(
                workspace
                    .path()
                    .join(".RaymanCodingSkill/goals/goal_0123456789.json")
            )
            .unwrap(),
            body
        );
        worker
            .process(&serde_json::to_vec(&storage).unwrap(), 1001)
            .unwrap();
        storage.operation = Operation::Storage {
            action: StorageAction::Release { lease_id: lease },
        };
        worker
            .process(&serde_json::to_vec(&storage).unwrap(), 1001)
            .unwrap();
        let result = temp.path().join(format!("result-{:032x}.json", 1));
        let before = std::fs::read(&result).unwrap();
        let record: QueueResult = serde_json::from_slice(&before).unwrap();
        assert!(record.success);
        let request_path = queue.join(format!("{:032x}.request.json", 1));
        let mut changed = request(&r);
        changed.request_id = format!("{:032x}", 1);
        changed.source_sha256 = "0".repeat(64);
        file_io::write_json(&request_path, &changed).unwrap();
        assert_eq!(worker.cycle(1002).unwrap(), 0);
        assert_eq!(std::fs::read(result).unwrap(), before);
        // Checkpoint persistence has a database-local atomic replay record,
        // independent of the volatile lease and outer queue-result write.
        let db_path = super::super::application_state::checkpoint_path(&worker.root, &enrollment);
        let connection = rusqlite::Connection::open(&db_path).unwrap();
        checkpoint_store::initialize_owned(&connection).unwrap();
        file_io::write_json(
            &worker
                .root
                .path()
                .join(format!("checkpoint-routing-{}.json", r.worktree_id)),
            &serde_json::json!({"active":true,"registration_sha256":r.digest().unwrap()}),
        )
        .unwrap();
        drop(connection);
        storage.operation = Operation::Storage {
            action: StorageAction::Acquire {
                object: StateObject::Checkpoints,
            },
        };
        let lease_reply = worker
            .process(&serde_json::to_vec(&storage).unwrap(), 1003)
            .unwrap();
        let RecordedResult::Storage { details } = lease_reply.result else {
            panic!("missing checkpoint lease")
        };
        let lease = details["lease_id"].as_str().unwrap().to_owned();
        let workspace_id = super::super::application_state::checkpoint_workspace_id(&enrollment);
        let rows = vec![checkpoint_store::RowChange {
            table: "session_intents".into(),
            key: vec![workspace_id.clone()],
            before_sha256: None,
            after: Some(vec![
                checkpoint_store::Cell::Text {
                    value: workspace_id.clone(),
                },
                checkpoint_store::Cell::Text { value: "{}".into() },
                checkpoint_store::Cell::Text {
                    value: "now".into(),
                },
            ]),
        }];
        let patch = serde_json::to_vec(&rows).unwrap();
        let patch_sha256 = sha256_bytes(&patch);
        storage.operation = Operation::Storage {
            action: StorageAction::Chunk {
                object_sha256: patch_sha256.clone(),
                index: 0,
                total_bytes: patch.len() as u64,
                hex: patch.iter().map(|b| format!("{b:02x}")).collect(),
            },
        };
        worker
            .process(&serde_json::to_vec(&storage).unwrap(), 1003)
            .unwrap();
        let transaction_id = "7".repeat(32);
        storage.operation = Operation::Storage {
            action: StorageAction::CheckpointApply {
                lease_id: lease,
                transaction_id: transaction_id.clone(),
                patch_sha256: patch_sha256.clone(),
                total_bytes: patch.len() as u64,
            },
        };
        let written = worker
            .process(&serde_json::to_vec(&storage).unwrap(), 1003)
            .unwrap();
        let RecordedResult::Storage { details } = written.result else {
            panic!("missing checkpoint write")
        };
        assert_eq!(details["applied"], true);
        assert_eq!(details["replayed"], false);
        let mut lost_lease = storage.clone();
        if let Operation::Storage {
            action: StorageAction::CheckpointApply { lease_id, .. },
        } = &mut lost_lease.operation
        {
            *lease_id = "0".repeat(32);
        }
        assert!(
            worker
                .process(&serde_json::to_vec(&lost_lease).unwrap(), 1003)
                .is_err()
        );
        drop(worker);
        let worker = Worker::open_with_executable(temp.path(), &binary).unwrap();
        storage.operation = Operation::Storage {
            action: StorageAction::Acquire {
                object: StateObject::Checkpoints,
            },
        };
        let acquired = worker
            .process(&serde_json::to_vec(&storage).unwrap(), 1004)
            .unwrap();
        let RecordedResult::Storage { details } = acquired.result else {
            panic!("missing new lease")
        };
        storage.operation = Operation::Storage {
            action: StorageAction::CheckpointApply {
                lease_id: details["lease_id"].as_str().unwrap().into(),
                transaction_id,
                patch_sha256,
                total_bytes: patch.len() as u64,
            },
        };
        let replayed = worker
            .process(&serde_json::to_vec(&storage).unwrap(), 1004)
            .unwrap();
        let RecordedResult::Storage { details } = replayed.result else {
            panic!("missing checkpoint replay")
        };
        assert_eq!(details["replayed"], true);
        let connection = rusqlite::Connection::open(&db_path).unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT count(*) FROM session_intents WHERE workspace_id=?1",
                    [&workspace_id],
                    |r| r.get::<_, u32>(0)
                )
                .unwrap(),
            1
        );
        drop(connection);
        checkpoint_client_roundtrip(&worker, workspace.path());
    }
}
