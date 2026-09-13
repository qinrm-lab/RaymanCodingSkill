//! Shared protocol for an explicitly registered desktop-user execution lane.
//! Validation is pure: it neither enrolls a project nor executes a request.

mod model;
pub use model::*;
mod ledger;
pub use ledger::*;
mod preflight;
pub use preflight::*;
mod storage;
pub use storage::*;
pub mod checkpoint_store;
mod enrollment;
pub use enrollment::*;
#[cfg(windows)]
mod relocation;
#[cfg(windows)]
mod worktrees;
#[cfg(windows)]
pub use relocation::rebind_workspace;
#[cfg(windows)]
pub use worktrees::{WorktreePolicy, authorize_worktrees, install_worktree_hook};
#[cfg(windows)]
mod install;
#[cfg(windows)]
pub use install::{
    InstallAdapter, InstallBundle, InstallBundleFile, InstallTarget, decode_install_sources,
    publish_global_skill, publish_global_skill_checked, register_install_adapter,
};
#[cfg(windows)]
mod worker;
#[cfg(windows)]
pub use worker::*;
#[cfg(windows)]
mod git;
mod git_policy;
pub use git_policy::{GitContentPolicy, GitHookPolicy};
#[cfg(windows)]
pub(crate) mod native;
#[cfg(windows)]
mod process;
#[cfg(windows)]
pub use git::*;
#[cfg(windows)]
mod client;
#[cfg(windows)]
mod publication;
#[cfg(windows)]
pub use client::*;
#[cfg(windows)]
mod application_state;
#[cfg(windows)]
pub use native::ProtectedDirectory;

use anyhow::{Result, bail};
use serde::Deserialize;
use std::collections::BTreeSet;

pub const MAX_REQUEST_BYTES: usize = 256 * 1024;
pub const MAX_LIFETIME_SECONDS: i64 = 120;
pub const MAX_CLOCK_SKEW_SECONDS: i64 = 30;

/// Decode with duplicate-key rejection before typed deserialization. Serde's
/// generic JSON value otherwise silently retains the last duplicate key.
pub fn decode_request(bytes: &[u8]) -> Result<Request> {
    decode_json(bytes)
}

pub fn decode_registration(bytes: &[u8]) -> Result<Registration> {
    let registration: Registration = decode_json(bytes)?;
    registration.validate()?;
    Ok(registration)
}

pub(crate) fn decode_json<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    if bytes.is_empty() || bytes.len() > MAX_REQUEST_BYTES {
        bail!("global request size is outside the permitted range");
    }
    if bytes.starts_with(&[0xef, 0xbb, 0xbf]) {
        bail!("global request must be UTF-8 without BOM");
    }
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let _ = UniqueJson::deserialize(&mut deserializer)?;
    deserializer.end()?;
    Ok(serde_json::from_slice(bytes)?)
}

/// Validate only against a registration already authenticated by the worker.
/// `now` is the worker's clock, never a timestamp supplied by the client.
pub fn validate_request(request: &Request, registration: &Registration, now: i64) -> Result<()> {
    validate_request_structure(request, registration)?;
    if request.created_at > now.saturating_add(MAX_CLOCK_SKEW_SECONDS) || request.expires_at <= now
    {
        bail!("global request is expired or future-dated");
    }
    Ok(())
}

pub(crate) fn validate_request_structure(
    request: &Request,
    registration: &Registration,
) -> Result<()> {
    registration.validate()?;
    if request.schema_version != 1 || !is_id(&request.request_id) {
        bail!("unsupported global request schema or invalid request ID");
    }
    if request.project_id != registration.project_id
        || request.worktree_id != registration.worktree_id
        || request.registration_sha256 != registration.digest()?
    {
        bail!("global request does not match the enrolled project/worktree/policy");
    }
    let lifetime = request
        .expires_at
        .checked_sub(request.created_at)
        .ok_or_else(|| anyhow::anyhow!("invalid request clock range"))?;
    if !(1..=MAX_LIFETIME_SECONDS).contains(&lifetime) {
        bail!("global request has an invalid lifetime");
    }
    if !is_sha256(&request.source_sha256) {
        bail!("invalid source digest");
    }
    match &request.operation {
        Operation::EnrollLinkedWorktree {
            workspace,
            root_identity,
            policy_sha256,
        } => {
            if !registration.capabilities.local_commit
                || request.source_sha256 != "0".repeat(64)
                || !workspace.is_absolute()
                || !is_sha256(root_identity)
                || !is_sha256(policy_sha256)
            {
                bail!("invalid linked-worktree enrollment request");
            }
        }
        Operation::Storage { action } => {
            if !registration.capabilities.formal_state || request.source_sha256 != "0".repeat(64) {
                bail!("storage traffic requires the enrolled non-authoritative storage scope");
            }
            let object_valid = |object: &StateObject| -> bool {
                match object {
                    StateObject::Goal { id } => {
                        id.strip_prefix("goal_").is_some_and(|v| is_hex(v, 10))
                    }
                    _ => true,
                }
            };
            match action {
                StorageAction::CheckpointApply {
                    lease_id,
                    transaction_id,
                    patch_sha256,
                    total_bytes,
                } => {
                    if !is_id(lease_id)
                        || !is_id(transaction_id)
                        || !is_sha256(patch_sha256)
                        || *total_bytes == 0
                        || *total_bytes > 64 * 1024 * 1024
                    {
                        bail!("invalid checkpoint transaction envelope");
                    }
                }
                StorageAction::Acquire { object } => {
                    if !object_valid(object) {
                        bail!("invalid application state object");
                    }
                }
                StorageAction::Release { lease_id } | StorageAction::Renew { lease_id } => {
                    if !is_id(lease_id) {
                        bail!("invalid state lease");
                    }
                }
                StorageAction::Chunk {
                    object_sha256,
                    index,
                    total_bytes,
                    hex,
                } => {
                    if !is_sha256(object_sha256)
                        || *total_bytes == 0
                        || *total_bytes > 512 * 1024 * 1024
                        || *index >= 8192
                        || hex.is_empty()
                        || hex.len() > 128 * 1024
                        || hex.len() % 2 != 0
                        || !is_hex(hex, hex.len())
                    {
                        bail!("invalid state transfer chunk");
                    }
                }
                StorageAction::Write {
                    object,
                    lease_id,
                    expected_sha256,
                    object_sha256,
                    total_bytes,
                } => {
                    if !object_valid(object)
                        || matches!(object, StateObject::GoalsStore | StateObject::Checkpoints)
                        || !is_id(lease_id)
                        || !is_sha256(object_sha256)
                        || expected_sha256.as_ref().is_some_and(|h| !is_sha256(h))
                        || *total_bytes == 0
                        || *total_bytes > 64 * 1024 * 1024
                    {
                        bail!("invalid application state publication");
                    }
                }
            }
        }
        Operation::RecoverCommit {
            original_request_id,
            candidate_sha256,
        } => {
            if !registration.capabilities.local_commit
                || !is_id(original_request_id)
                || original_request_id == &request.request_id
                || !is_sha256(candidate_sha256)
            {
                bail!("invalid enrolled commit recovery request");
            }
        }
        Operation::Commit {
            expected_head,
            expected_index_sha256,
            message,
            changes,
            hook_receipt,
        } => {
            if !registration.capabilities.local_commit {
                bail!("local commit capability is not enrolled");
            }
            if !is_hex(expected_head, registration.object_id_length)
                || !is_sha256(expected_index_sha256)
                || message.trim() != message
                || message.is_empty()
                || message.chars().count() > 200
                || message.chars().any(char::is_control)
                || changes.is_empty()
                || changes.len() > 1024
            {
                bail!("invalid exact commit request");
            }
            if hook_receipt.as_ref().is_some_and(|receipt| {
                !is_sha256(&receipt.policy_sha256)
                    || !is_hex(&receipt.candidate_tree_oid, registration.object_id_length)
                    || !is_sha256(&receipt.source_sha256)
                    || !is_sha256(&receipt.stdout_sha256)
                    || !is_sha256(&receipt.stderr_sha256)
                    || !is_sha256(&receipt.environment_sha256)
                    || receipt.exit_code != 0
            }) {
                bail!("invalid sandbox hook receipt");
            }
            let mut previous: Option<&str> = None;
            let mut folded = BTreeSet::new();
            for change in changes {
                validate_relative_path(&change.path)?;
                if previous.is_some_and(|path| path >= change.path.as_str())
                    || !folded.insert(change.path.to_lowercase())
                {
                    bail!("commit paths must be sorted, unique and case-disjoint");
                }
                previous = Some(&change.path);
                change.validate(registration)?;
            }
        }
        Operation::State {
            task_id,
            expected_revision,
            mutation,
        } => {
            if !registration.capabilities.formal_state
                || !is_id(task_id)
                || *expected_revision == u64::MAX
            {
                bail!("invalid or unenrolled formal state operation");
            }
            match mutation {
                StateMutation::Progress { stage, message } => {
                    if !is_label(stage)
                        || message.is_empty()
                        || message.len() > 4096
                        || message.contains('\0')
                    {
                        bail!("invalid progress update");
                    }
                }
                StateMutation::Begin { title } => {
                    if title.trim().is_empty()
                        || title.len() > 1024
                        || title.chars().any(char::is_control)
                        || *expected_revision != 0
                    {
                        bail!("invalid task initialization");
                    }
                }
            }
        }
        Operation::Install {
            adapter_id,
            artifact_sha256,
        } => {
            if !is_label(adapter_id)
                || !is_sha256(artifact_sha256)
                || !registration
                    .capabilities
                    .install_adapters
                    .contains(adapter_id)
            {
                bail!("installation adapter is not enrolled or artifact identity is invalid");
            }
        }
    }
    Ok(())
}

pub(crate) fn is_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
pub(crate) fn is_sha256(value: &str) -> bool {
    is_hex(value, 64)
}
pub(crate) fn is_id(value: &str) -> bool {
    is_hex(value, 32)
}
pub(crate) fn is_label(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}

/// Protocol paths are slash-separated and cannot name Windows devices, ADS,
/// traversal, control files or ambiguous trailing dots/spaces. Filesystem
/// handlers must additionally reject reparse points and hold strong identities.
pub fn validate_relative_path(path: &str) -> Result<()> {
    if path.is_empty()
        || path.len() > 4096
        || path.chars().any(char::is_control)
        || path.contains(['\\', ':', '*', '?', '"', '<', '>', '|'])
    {
        bail!("unsafe global operation path");
    }
    for component in path.split('/') {
        let lower = component.to_lowercase();
        let stem = lower.split('.').next().unwrap_or_default();
        let device_number = stem
            .strip_prefix("com")
            .or_else(|| stem.strip_prefix("lpt"));
        if component.is_empty()
            || matches!(component, "." | "..")
            || component.ends_with(['.', ' '])
            || matches!(lower.as_str(), ".git" | ".agent-checkpoints")
            || (lower == ".raymancodingskill" && path != ".RaymanCodingSkill/quality.json")
            || matches!(stem, "con" | "prn" | "aux" | "nul" | "conin$" | "conout$")
            || device_number.is_some_and(|n| {
                matches!(
                    n,
                    "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
                )
            })
        {
            bail!("unsafe global operation path component");
        }
    }
    Ok(())
}

struct UniqueJson;
impl<'de> Deserialize<'de> for UniqueJson {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = UniqueJson;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("JSON without duplicate or case-colliding keys")
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> std::result::Result<UniqueJson, A::Error> {
                let mut seen = BTreeSet::new();
                while let Some(key) = map.next_key::<String>()? {
                    if !seen.insert(key.to_lowercase()) {
                        return Err(serde::de::Error::custom(
                            "duplicate or case-colliding JSON key",
                        ));
                    }
                    let _ = map.next_value::<UniqueJson>()?;
                }
                Ok(UniqueJson)
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> std::result::Result<UniqueJson, A::Error> {
                while seq.next_element::<UniqueJson>()?.is_some() {}
                Ok(UniqueJson)
            }
            fn visit_bool<E: serde::de::Error>(
                self,
                _: bool,
            ) -> std::result::Result<UniqueJson, E> {
                Ok(UniqueJson)
            }
            fn visit_i64<E: serde::de::Error>(self, _: i64) -> std::result::Result<UniqueJson, E> {
                Ok(UniqueJson)
            }
            fn visit_u64<E: serde::de::Error>(self, _: u64) -> std::result::Result<UniqueJson, E> {
                Ok(UniqueJson)
            }
            fn visit_f64<E: serde::de::Error>(self, _: f64) -> std::result::Result<UniqueJson, E> {
                Ok(UniqueJson)
            }
            fn visit_str<E: serde::de::Error>(self, _: &str) -> std::result::Result<UniqueJson, E> {
                Ok(UniqueJson)
            }
            fn visit_unit<E: serde::de::Error>(self) -> std::result::Result<UniqueJson, E> {
                Ok(UniqueJson)
            }
        }
        d.deserialize_any(Visitor)
    }
}

#[cfg(test)]
mod tests;
