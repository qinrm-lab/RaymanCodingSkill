//! Durable state transactions below an already protected worker-owned root.
//! This component does not attest installation ACLs or confer a capability.
//! A caller must hold that attested root stable for the complete transaction.
use super::*;
use crate::{file_io, hash::sha256_bytes, state_lock, state_paths};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};

const MAX_LEDGER_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    schema_version: u32,
    registration_sha256: String,
    ledger_sha256: String,
    ledger: Ledger,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    schema_version: u32,
    before_sha256: Option<String>,
    after_sha256: String,
    after: Envelope,
}

/// Storage layout is selected by an attested worker, never by a request.
/// Pure protocol clients cannot construct a desktop-user process from this API.
pub struct StateStorage {
    ledger: PathBuf,
    journal: PathBuf,
    registration_sha256: String,
}

impl StateStorage {
    pub fn at_existing_root(root: &Path, registration: &Registration) -> Result<Self> {
        state_paths::ensure_real_directory(root)?;
        let root = fs::canonicalize(root)?;
        Ok(Self {
            ledger: root.join(format!("ledger-{}.json", registration.worktree_id)),
            journal: root.join(format!("transaction-{}.json", registration.worktree_id)),
            registration_sha256: registration.digest()?,
        })
    }

    pub fn accept(
        &self,
        request: &Request,
        registration: &Registration,
        now: i64,
    ) -> Result<Transition> {
        if registration.digest()? != self.registration_sha256 {
            bail!("storage registration changed");
        }
        // Validate before touching state or recovering a prior operation.
        validate_request_structure(request, registration)?;
        let _lock = state_lock::acquire_state_lock(&self.ledger)?;
        self.recover_locked()?;
        let before = self.read_current()?;
        let current = before
            .as_ref()
            .map(|(_, e)| e.ledger.clone())
            .unwrap_or_default();
        let transition = current.accept(request, registration, now)?;
        if !transition.replayed {
            self.publish(before.as_ref().map(|(h, _)| h.clone()), &transition.next)?;
        }
        Ok(transition)
    }

    pub fn recover(&self) -> Result<()> {
        let _lock = state_lock::acquire_state_lock(&self.ledger)?;
        self.recover_locked()
    }

    pub fn recorded_result(
        &self,
        request: &Request,
        registration: &Registration,
    ) -> Result<Option<RecordedResult>> {
        validate_request_structure(request, registration)?;
        if registration.digest()? != self.registration_sha256 {
            bail!("storage registration changed");
        }
        let _lock = state_lock::acquire_state_lock(&self.ledger)?;
        let Some((_, current)) = self.read_current()? else {
            return Ok(None);
        };
        let Some(record) = current.ledger.requests.get(&request.request_id) else {
            return Ok(None);
        };
        if record.request_sha256 != request.digest()?
            || record.registration_sha256 != registration.digest()?
            || record.project_id != request.project_id
            || record.worktree_id != request.worktree_id
        {
            bail!("request replay content differs");
        }
        Ok(Some(record.result.clone()))
    }

    pub fn record_effect_result(
        &self,
        id: &str,
        request_sha256: &str,
        outcome: RecordedResult,
    ) -> Result<()> {
        let _lock = state_lock::acquire_state_lock(&self.ledger)?;
        self.recover_locked()?;
        let (before_hash, current) = self
            .read_current()?
            .ok_or_else(|| anyhow::anyhow!("effect has no durable ledger"))?;
        let next = current
            .ledger
            .record_effect_result(id, request_sha256, outcome)?;
        if next.revision != current.ledger.revision {
            self.publish(Some(before_hash), &next)?;
        }
        Ok(())
    }

    fn read_current(&self) -> Result<Option<(String, Envelope)>> {
        let Some((bytes, _)) = file_io::read_optional_handle_bound_file_bounded(
            &self.ledger,
            "global ledger",
            MAX_LEDGER_BYTES,
        )?
        else {
            return Ok(None);
        };
        let envelope: Envelope = serde_json::from_slice(&bytes)?;
        self.validate_envelope(&envelope)?;
        Ok(Some((sha256_bytes(&bytes), envelope)))
    }

    fn validate_envelope(&self, envelope: &Envelope) -> Result<()> {
        if envelope.schema_version != 1
            || envelope.registration_sha256 != self.registration_sha256
            || envelope.ledger_sha256 != sha256_bytes(&serde_json::to_vec(&envelope.ledger)?)
        {
            bail!("global ledger identity or integrity mismatch");
        }
        Ok(())
    }

    fn publish(&self, before_sha256: Option<String>, ledger: &Ledger) -> Result<()> {
        let after = Envelope {
            schema_version: 1,
            registration_sha256: self.registration_sha256.clone(),
            ledger_sha256: sha256_bytes(&serde_json::to_vec(ledger)?),
            ledger: ledger.clone(),
        };
        let after_bytes = serde_json::to_vec(&after)?;
        if after_bytes.len() as u64 > MAX_LEDGER_BYTES {
            bail!("global ledger exceeds size limit");
        }
        let journal = Journal {
            schema_version: 1,
            before_sha256,
            after_sha256: sha256_bytes(&after_bytes),
            after,
        };
        // Publishing the intent precedes effects. Recovery completes this exact
        // ledger transition, then the executor handles any recovery-required effect.
        file_io::write_json(&self.journal, &journal)?;
        self.recover_locked()
    }

    fn recover_locked(&self) -> Result<()> {
        let Some((bytes, _)) = file_io::read_optional_handle_bound_file_bounded(
            &self.journal,
            "global state transaction",
            MAX_LEDGER_BYTES * 2,
        )?
        else {
            return Ok(());
        };
        let journal: Journal = serde_json::from_slice(&bytes)?;
        self.validate_envelope(&journal.after)?;
        let after = serde_json::to_vec(&journal.after)?;
        if journal.schema_version != 1
            || journal.after_sha256 != sha256_bytes(&after)
            || journal
                .before_sha256
                .as_ref()
                .is_some_and(|h| !is_sha256(h))
        {
            bail!("global state transaction integrity mismatch");
        }
        let current = self.read_current()?;
        let current_hash = current.as_ref().map(|(hash, _)| hash.as_str());
        if current_hash != Some(journal.after_sha256.as_str()) {
            if current_hash != journal.before_sha256.as_deref() {
                bail!("global state recovery conflicts with current data; evidence retained");
            }
            file_io::write_atomic(&self.ledger, std::str::from_utf8(&after)?)?;
        }
        let observed = self
            .read_current()?
            .ok_or_else(|| anyhow::anyhow!("ledger disappeared after publication"))?;
        if observed.0 != journal.after_sha256 {
            bail!("global ledger readback failed");
        }
        // Remove only this terminal journal, never a lock or a state directory.
        let reread = file_io::read_handle_bound_file(&self.journal, "terminal global transaction")?;
        if reread.0 != bytes {
            bail!("global transaction changed during recovery");
        }
        fs::remove_file(&self.journal)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::global_execution::tests::{registration, request};

    #[test]
    fn durable_state_survives_reopen_and_does_not_repeat_an_effect() {
        let temp = tempfile::tempdir().unwrap();
        let r = registration();
        let q = request(&r);
        let store = StateStorage::at_existing_root(temp.path(), &r).unwrap();
        assert!(store.accept(&q, &r, 1000).unwrap().execute_effect);
        drop(store);
        let store = StateStorage::at_existing_root(temp.path(), &r).unwrap();
        let repeated = store.accept(&q, &r, 1001).unwrap();
        assert!(repeated.replayed);
        assert!(!repeated.execute_effect);
        assert_eq!(repeated.result, RecordedResult::RecoveryRequired);
        assert!(!store.journal.exists());
    }

    #[test]
    fn journal_recovers_exact_state_and_retains_conflicting_data() {
        let temp = tempfile::tempdir().unwrap();
        let r = registration();
        let q = request(&r);
        let store = StateStorage::at_existing_root(temp.path(), &r).unwrap();
        let next = Ledger::default().accept(&q, &r, 1000).unwrap().next;
        let after = Envelope {
            schema_version: 1,
            registration_sha256: r.digest().unwrap(),
            ledger_sha256: sha256_bytes(&serde_json::to_vec(&next).unwrap()),
            ledger: next,
        };
        let journal = Journal {
            schema_version: 1,
            before_sha256: None,
            after_sha256: sha256_bytes(&serde_json::to_vec(&after).unwrap()),
            after,
        };
        file_io::write_json(&store.journal, &journal).unwrap();
        store.recover().unwrap();
        assert!(store.accept(&q, &r, 1001).unwrap().replayed);
        let preserved = fs::read(&store.ledger).unwrap();
        let mut conflict = journal.clone();
        conflict.before_sha256 = Some("0".repeat(64));
        conflict.after.ledger.revision += 1;
        conflict.after.ledger_sha256 =
            sha256_bytes(&serde_json::to_vec(&conflict.after.ledger).unwrap());
        conflict.after_sha256 = sha256_bytes(&serde_json::to_vec(&conflict.after).unwrap());
        file_io::write_json(&store.journal, &conflict).unwrap();
        assert!(store.recover().is_err());
        assert_eq!(fs::read(&store.ledger).unwrap(), preserved);
        assert!(store.journal.exists());
    }
}
