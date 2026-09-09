use super::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Registration {
    pub schema_version: u32,
    pub project_id: String,
    pub worktree_id: String,
    pub owner_sid: String,
    pub root_identity: String,
    pub git_common_identity: String,
    pub object_id_length: usize,
    pub policy_sha256: String,
    pub capabilities: Capabilities,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capabilities {
    pub local_commit: bool,
    pub added_files: bool,
    pub deleted_files: bool,
    pub formal_state: bool,
    pub install_adapters: BTreeSet<String>,
}

impl Registration {
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != 1
            || !is_id(&self.project_id)
            || !is_id(&self.worktree_id)
            || !is_sha256(&self.root_identity)
            || !is_sha256(&self.git_common_identity)
            || !is_sha256(&self.policy_sha256)
            || !matches!(self.object_id_length, 40 | 64)
            || !self.owner_sid.starts_with("S-1-5-21-")
            || self.owner_sid.len() > 184
            || !self.owner_sid[2..]
                .split('-')
                .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
            || self
                .capabilities
                .install_adapters
                .iter()
                .any(|id| !is_label(id))
            || (!self.capabilities.local_commit
                && (self.capabilities.added_files || self.capabilities.deleted_files))
        {
            bail!("invalid global project registration");
        }
        Ok(())
    }

    pub fn digest(&self) -> Result<String> {
        self.validate()?;
        Ok(crate::hash::sha256_bytes(&serde_json::to_vec(self)?))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub schema_version: u32,
    pub request_id: String,
    pub project_id: String,
    pub worktree_id: String,
    pub registration_sha256: String,
    pub source_sha256: String,
    pub created_at: i64,
    pub expires_at: i64,
    pub operation: Operation,
}

impl Request {
    pub fn digest(&self) -> Result<String> {
        Ok(crate::hash::sha256_bytes(&serde_json::to_vec(self)?))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    Storage {
        action: StorageAction,
    },
    Commit {
        expected_head: String,
        expected_index_sha256: String,
        message: String,
        changes: Vec<Change>,
        hook_receipt: Option<HookReceipt>,
    },
    State {
        task_id: String,
        expected_revision: u64,
        mutation: StateMutation,
    },
    Install {
        adapter_id: String,
        artifact_sha256: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HookReceipt {
    pub policy_sha256: String,
    pub candidate_tree_oid: String,
    pub source_sha256: String,
    pub exit_code: u32,
    pub stdout_sha256: String,
    pub stderr_sha256: String,
    pub environment_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum StateObject {
    Goal { id: String },
    GoalsStore,
    Pending,
    ContextIndex,
    Checkpoints,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum StorageAction {
    CheckpointApply {
        lease_id: String,
        transaction_id: String,
        patch_sha256: String,
        total_bytes: u64,
    },
    Acquire {
        object: StateObject,
    },
    Release {
        lease_id: String,
    },
    Renew {
        lease_id: String,
    },
    Chunk {
        object_sha256: String,
        index: u32,
        total_bytes: u64,
        hex: String,
    },
    Write {
        object: StateObject,
        lease_id: String,
        expected_sha256: Option<String>,
        object_sha256: String,
        total_bytes: u64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum StateMutation {
    Begin { title: String },
    Progress { stage: String, message: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Change {
    pub path: String,
    pub before: Option<FileVersion>,
    pub after: Option<FileVersion>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileVersion {
    pub raw_sha256: String,
    pub git_blob_oid: String,
    pub mode: u32,
}

impl Change {
    pub(super) fn validate(&self, registration: &Registration) -> Result<()> {
        match (&self.before, &self.after) {
            (None, None) => bail!("empty change"),
            (None, Some(_)) if !registration.capabilities.added_files => {
                bail!("added files are not enrolled")
            }
            (Some(_), None) if !registration.capabilities.deleted_files => {
                bail!("deleted files are not enrolled")
            }
            (Some(before), Some(after)) if before == after => {
                bail!("unchanged file in commit request")
            }
            _ => {}
        }
        for version in self.before.iter().chain(self.after.iter()) {
            if !is_sha256(&version.raw_sha256)
                || !is_hex(&version.git_blob_oid, registration.object_id_length)
                || !matches!(version.mode, 0o100644 | 0o100755)
            {
                bail!("invalid ordinary file version");
            }
        }
        Ok(())
    }
}
