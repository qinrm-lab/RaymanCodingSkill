//! Owner-opted-in enrollment of real linked worktrees. A hook is a trigger,
//! not authority; protected policy and Git back-links are verified anew.
use super::*;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const HOOK_LABEL: &str = "Rayman linked-worktree state setup";

fn merge_hook(document: &mut serde_json::Value, command: &str) -> Result<bool> {
    let root = document
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("hooks document must be an object"))?;
    let hooks = root
        .entry("hooks")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("hooks must be an object"))?;
    let start = hooks
        .entry("SessionStart")
        .or_insert_with(|| serde_json::json!([]))
        .as_array_mut()
        .ok_or_else(|| anyhow::anyhow!("SessionStart hooks must be an array"))?;
    let desired = serde_json::json!({"matcher":"^(startup|resume|clear|compact)$","hooks":[{"type":"command","command":command,"timeout":180,"statusMessage":HOOK_LABEL,"additionalContextLimit":1200}]});
    let mut found = false;
    for entry in start.iter() {
        let owned = entry
            .get("hooks")
            .and_then(|v| v.as_array())
            .is_some_and(|v| v.iter().any(|h| h["statusMessage"] == HOOK_LABEL));
        if owned {
            if found || *entry != desired {
                bail!("existing worktree hook differs; preserve it for explicit repair");
            }
            found = true;
        }
    }
    if !found {
        start.push(desired);
    }
    Ok(!found)
}

pub fn install_worktree_hook(root: &Path, publish: bool) -> Result<serde_json::Value> {
    native::require_unelevated_worker()?;
    let client = Client::open(root)?;
    let status = client.status()?;
    let context = crate::execution_context::execution_context_probe();
    let owner = context
        .principal_sid
        .ok_or_else(|| anyhow::anyhow!("hook installer principal unavailable"))?;
    if status["owner_sid"] != owner {
        bail!("hook installation requires the desktop owner");
    }
    let root_pin = ProtectedDirectory::open(root, &owner)?;
    let installation: Installation =
        decode_json(&root_pin.read_file("installation.json", MAX_REQUEST_BYTES as u64)?)?;
    let executable = std::fs::canonicalize(std::env::current_exe()?)?;
    if executable != root_pin.path().join("client.exe")
        || crate::hash::sha256_file(&executable)? != installation.client_sha256
    {
        bail!("install the hook through the matching installed client after the kernel upgrade");
    }
    let profile = PathBuf::from(
        context
            .token_profile
            .ok_or_else(|| anyhow::anyhow!("desktop profile unavailable"))?,
    );
    let directory = native::SourceDirectory::open(&profile.join(".codex"))?;
    let _lock = if publish {
        Some(crate::state_lock::acquire_state_lock(
            &directory.path().join("worktree-hook-install"),
        )?)
    } else {
        None
    };
    let target = directory.path().join("hooks.json");
    let before = if target.try_exists()? {
        Some(directory.read_file(Path::new("hooks.json"), 1024 * 1024)?)
    } else {
        None
    };
    let mut value = match &before {
        Some(bytes) => decode_json(bytes)?,
        None => serde_json::json!({}),
    };
    let client_path = crate::pathfmt::display_path(&executable);
    let root_path = crate::pathfmt::display_path(root_pin.path());
    if client_path.contains(['"', '\0', '\r', '\n', '%', '!', '`'])
        || root_path.contains(['"', '\0', '\r', '\n', '%', '!', '`'])
    {
        bail!("unsupported shell spelling for fixed hook command");
    }
    let command = format!("\"{client_path}\" worktree-hook --root \"{root_path}\"");
    let changed = merge_hook(&mut value, &command)?;
    if publish && changed {
        let bytes = serde_json::to_vec_pretty(&value)?;
        let nonce = format!(
            "{:x}",
            chrono::Utc::now()
                .timestamp_nanos_opt()
                .ok_or_else(|| anyhow::anyhow!("hook staging clock invalid"))?
        );
        let stage = super::publication::PublicationSlot::create(
            directory.path(),
            &format!(".worktree-hook-{nonce}"),
            &bytes,
        )?;
        if let Some(old) = &before {
            stage.preserve_access_from(&target)?;
            let backup = format!(
                "hooks-before-worktree-{}.json",
                crate::hash::sha256_bytes(old)
            );
            if directory.path().join(&backup).try_exists()? {
                if directory.read_file(Path::new(&backup), 1024 * 1024)? != *old {
                    bail!("existing hook backup differs");
                }
            } else {
                let mut saved = super::publication::PublicationSlot::create(
                    directory.path(),
                    &format!(".worktree-hook-backup-{nonce}"),
                    old,
                )?;
                saved.preserve_access_from(&target)?;
                saved.rename(&backup, false)?;
            }
            if directory.read_file(Path::new("hooks.json"), 1024 * 1024)? != *old {
                bail!("hook configuration changed concurrently");
            }
        } else if target.try_exists()? {
            bail!("hook configuration appeared concurrently");
        }
        publish_hook_and_verify(stage, &directory, before.as_deref(), &bytes)?;
    }
    Ok(
        serde_json::json!({"installed":publish,"preview":!publish,"changed":changed,"path":target,"hook_review_required":changed,"other_handlers_preserved":true}),
    )
}

fn publish_hook_and_verify(
    mut stage: super::publication::PublicationSlot,
    directory: &native::SourceDirectory,
    before: Option<&[u8]>,
    bytes: &[u8],
) -> Result<()> {
    stage.publish_reviewed("hooks.json", before)?;
    // Strict reads deny existing writers too. Release our publication handle
    // before reopening the published file; the directory remains pinned.
    drop(stage);
    if directory.read_file(Path::new("hooks.json"), 1024 * 1024)? != bytes {
        bail!("hook publication readback differs");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(windows)]
    #[test]
    fn hook_publication_strict_readback_releases_its_own_write_handle() {
        for existing in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let old = b"previous hook document";
            if existing {
                std::fs::write(temp.path().join("hooks.json"), old).unwrap();
            }
            let directory = native::SourceDirectory::open(temp.path()).unwrap();
            let bytes = br#"{"hooks":{"SessionStart":[]}}"#;
            let stage = super::super::publication::PublicationSlot::create(
                temp.path(),
                "hook-stage",
                bytes,
            )
            .unwrap();
            publish_hook_and_verify(stage, &directory, existing.then_some(old.as_slice()), bytes)
                .unwrap();
            assert_eq!(
                directory.read_file(Path::new("hooks.json"), 1024).unwrap(),
                bytes
            );
            if existing {
                assert_eq!(
                    std::fs::read(temp.path().join("hook-stage.previous")).unwrap(),
                    old
                );
            }
        }
    }
    #[cfg(windows)]
    #[test]
    fn hook_merge_preserves_foreign_handlers_and_refuses_ambiguous_ownership() {
        let mut document = serde_json::json!({"custom":{"keep":true},"hooks":{"Stop":[{"hooks":[{"command":"existing"}]}],"SessionStart":[{"hooks":[{"command":"other"}]}]}});
        let original = document.clone();
        assert!(merge_hook(&mut document, "fixed command").unwrap());
        assert_eq!(document["hooks"]["Stop"], original["hooks"]["Stop"]);
        assert_eq!(document["custom"], original["custom"]);
        assert_eq!(
            document["hooks"]["SessionStart"][0],
            original["hooks"]["SessionStart"][0]
        );
        assert!(!merge_hook(&mut document, "fixed command").unwrap());
        let before = document.clone();
        assert!(merge_hook(&mut document, "different command").is_err());
        assert_eq!(document, before);
        document["hooks"]["SessionStart"]
            .as_array_mut()
            .unwrap()
            .push(before["hooks"]["SessionStart"][1].clone());
        assert!(merge_hook(&mut document, "fixed command").is_err());
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorktreePolicy {
    pub schema_version: u32,
    pub installation_id: String,
    pub anchor_worktree_id: String,
    pub anchor_registration_sha256: String,
    pub allowed_root: PathBuf,
    pub allowed_root_identity: String,
    pub formal_state: bool,
    pub rayman: Option<WorktreeProduct>,
    pub checkpoint: Option<WorktreeProduct>,
    pub powershell: Option<WorktreeProductFile>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorktreeProductFile {
    pub path: PathBuf,
    pub parent_identity: String,
    pub sha256: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorktreeProduct {
    pub program: PathBuf,
    pub program_sha256: String,
    pub source_skill: PathBuf,
    pub files: Vec<WorktreeProductFile>,
}

fn product_file(path: &Path, parent_identity: Option<&str>) -> Result<WorktreeProductFile> {
    let parent = native::SourceDirectory::open(
        path.parent()
            .ok_or_else(|| anyhow::anyhow!("product parent missing"))?,
    )?;
    if parent_identity.is_some_and(|id| id != parent.identity()) {
        bail!("registered product directory changed");
    }
    let leaf = Path::new(
        path.file_name()
            .ok_or_else(|| anyhow::anyhow!("product leaf missing"))?,
    );
    let _pin = parent.pin_file(leaf)?;
    let bytes = parent.read_file(leaf, 128 * 1024 * 1024)?;
    Ok(WorktreeProductFile {
        path: parent.path().join(leaf),
        parent_identity: parent.identity().into(),
        sha256: crate::hash::sha256_bytes(&bytes),
    })
}

fn product(
    root: &ProtectedDirectory,
    installation: &Installation,
    kind: &str,
) -> Result<(WorktreeProduct, WorktreeProductFile)> {
    let suffix = format!("-{kind}.json");
    let mut found = None;
    for entry in std::fs::read_dir(root.path())? {
        let name = entry?.file_name().to_string_lossy().into_owned();
        let Some(id) = name
            .strip_prefix("install-adapter-")
            .and_then(|s| s.strip_suffix(&suffix))
        else {
            continue;
        };
        if !is_id(id) || found.is_some() {
            bail!("ambiguous product adapter for worktree bootstrap");
        }
        let adapter: InstallAdapter =
            decode_json(&root.read_file(&name, MAX_REQUEST_BYTES as u64)?)?;
        let enrollment: Enrollment =
            decode_json(&root.read_file(&format!("project-{id}.json"), MAX_REQUEST_BYTES as u64)?)?;
        enrollment.validate(installation)?;
        adapter.validate(&enrollment)?;
        if enrollment.install_policies.get(kind) != Some(&adapter.digest()?) {
            bail!("product adapter is not owner-attested");
        }
        let (program_role, skill_role) = if kind == "rayman" {
            ("01_cli", "10_skill")
        } else {
            ("save_06", "save_00")
        };
        let program = &adapter
            .targets
            .get(program_role)
            .ok_or_else(|| anyhow::anyhow!("product CLI role missing"))?
            .destination;
        let skill = &adapter
            .targets
            .get(skill_role)
            .ok_or_else(|| anyhow::anyhow!("product skill role missing"))?
            .destination;
        let mut files = Vec::new();
        for (role, target) in &adapter.targets {
            if kind == "rayman"
                && !matches!(
                    role.as_str(),
                    "01_cli" | "10_skill" | "11_agent_contract" | "12_workflow_contract"
                )
            {
                continue;
            }
            files.push(product_file(
                &target.destination,
                Some(&target.parent_identity),
            )?);
        }
        let powershell = product_file(&adapter.powershell, None)?;
        if powershell.sha256 != adapter.powershell_sha256 {
            bail!("registered PowerShell changed");
        }
        let program_path = std::fs::canonicalize(program)?;
        let program_sha256 = files
            .iter()
            .find(|file| file.path == program_path)
            .ok_or_else(|| anyhow::anyhow!("program is outside product roles"))?
            .sha256
            .clone();
        found = Some((
            WorktreeProduct {
                program: program.clone(),
                program_sha256,
                source_skill: skill
                    .parent()
                    .ok_or_else(|| anyhow::anyhow!("skill directory missing"))?
                    .into(),
                files,
            },
            powershell,
        ));
    }
    found.ok_or_else(|| {
        anyhow::anyhow!("registered {kind} product is required for worktree state initialization")
    })
}

fn has_parent_route(
    anchor: &Enrollment,
    installation: &Installation,
    relative: &str,
) -> Result<bool> {
    let source = native::SourceDirectory::open(&anchor.workspace)?;
    if !source.path().join(relative).try_exists()? {
        return Ok(false);
    }
    let marker: serde_json::Value = decode_json(&source.read_file(Path::new(relative), 65536)?)?;
    if marker["schema"] != "rayman.global-state-routing.v1"
        || marker["installation_id"] != installation.installation_id
        || marker["worktree_id"] != anchor.registration.worktree_id
    {
        bail!("parent application route does not match enrollment");
    }
    Ok(true)
}

pub(super) fn pin_products(policy: &WorktreePolicy) -> Result<Vec<native::SourceFile>> {
    let mut pins = Vec::new();
    for product in policy.rayman.iter().chain(policy.checkpoint.iter()) {
        if product.files.is_empty()
            || product.files.len() > 64
            || !product.program.is_absolute()
            || !product.source_skill.is_absolute()
        {
            bail!("invalid owner product binding");
        }
        let _program_parent = native::SourceDirectory::open(
            product
                .program
                .parent()
                .ok_or_else(|| anyhow::anyhow!("program parent missing"))?,
        )?;
        let _skill_parent = native::SourceDirectory::open(&product.source_skill)?;
        for path in [&product.program, &product.source_skill.join("SKILL.md")] {
            let canonical = std::fs::canonicalize(path)?;
            if !product.files.iter().any(|file| file.path == canonical) {
                bail!("bootstrap program or skill is outside its pinned role set");
            }
        }
        if !product.files.iter().any(|file| {
            std::fs::canonicalize(&product.program).ok().as_ref() == Some(&file.path)
                && file.sha256 == product.program_sha256
        }) {
            bail!("product program hash binding differs");
        }
    }
    for file in policy
        .rayman
        .iter()
        .chain(policy.checkpoint.iter())
        .flat_map(|p| p.files.iter())
        .chain(policy.powershell.iter())
    {
        let parent = native::SourceDirectory::open(
            file.path
                .parent()
                .ok_or_else(|| anyhow::anyhow!("product parent missing"))?,
        )?;
        if parent.identity() != file.parent_identity {
            bail!("bootstrap product parent changed");
        }
        let leaf = Path::new(
            file.path
                .file_name()
                .ok_or_else(|| anyhow::anyhow!("product name missing"))?,
        );
        let pin = parent.pin_file(leaf)?;
        if crate::hash::sha256_bytes(&parent.read_file(leaf, 128 * 1024 * 1024)?) != file.sha256 {
            bail!("bootstrap product bytes changed; owner policy refresh required");
        }
        pins.push(pin);
    }
    Ok(pins)
}

pub(super) fn verify_member(
    root: &ProtectedDirectory,
    child: &Enrollment,
    policy_sha256: &str,
) -> Result<()> {
    let bytes = root.read_file(
        &format!("worktree-member-{}.json", child.registration.worktree_id),
        MAX_REQUEST_BYTES as u64,
    )?;
    let member: WorktreeMember = decode_json(&bytes)?;
    if member.schema_version != 1
        || !member.registered
        || member.registration_sha256 != child.registration.digest()?
        || member.policy_sha256 != policy_sha256
    {
        bail!("worktree membership requires explicit recovery");
    }
    let source = native::SourceDirectory::open(&child.workspace)?;
    let git = native::SourceDirectory::open(
        &child
            .git
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("member Git binding missing"))?
            .git_directory,
    )?;
    if super::relocation::rebound_enrollment(root, source.path())?.is_some() {
        return Ok(());
    }
    let expected =
        crate::hash::sha256_bytes(format!("{}:{}", source.identity(), git.identity()).as_bytes());
    if child.registration.worktree_id != expected[..32] {
        bail!("registered linked-worktree Git directory was replaced");
    }
    Ok(())
}

pub(super) fn verify_existing_member(root: &ProtectedDirectory, child: &Enrollment) -> Result<()> {
    let name = format!("worktree-member-{}.json", child.registration.worktree_id);
    if root.path().join(&name).try_exists()? {
        let member: WorktreeMember =
            decode_json(&root.read_file(&name, MAX_REQUEST_BYTES as u64)?)?;
        verify_member(root, child, &member.policy_sha256)?;
    }
    Ok(())
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorktreeMember {
    schema_version: u32,
    policy_sha256: String,
    registration_sha256: String,
    registered: bool,
}

pub(super) fn checked_policy(
    root: &ProtectedDirectory,
    installation: &Installation,
    anchor: &Enrollment,
) -> Result<(WorktreePolicy, String)> {
    let name = format!("worktree-policy-{}.json", anchor.registration.worktree_id);
    let bytes = root.read_file(&name, MAX_REQUEST_BYTES as u64)?;
    let policy: WorktreePolicy = decode_json(&bytes)?;
    if policy.schema_version != 1
        || policy.installation_id != installation.installation_id
        || policy.anchor_worktree_id != anchor.registration.worktree_id
        || policy.anchor_registration_sha256 != anchor.registration.digest()?
        || !policy.allowed_root.is_absolute()
        || !is_sha256(&policy.allowed_root_identity)
        || (policy.formal_state && !anchor.registration.capabilities.formal_state)
        || (!policy.formal_state && (policy.rayman.is_some() || policy.checkpoint.is_some()))
        || ((policy.rayman.is_some() || policy.checkpoint.is_some()) != policy.powershell.is_some())
    {
        bail!("linked-worktree policy does not match its owner-approved anchor");
    }
    let allowed = native::SourceDirectory::open(&policy.allowed_root)?;
    if allowed.identity() != policy.allowed_root_identity {
        bail!("approved linked-worktree root was replaced");
    }
    Ok((policy, crate::hash::sha256_bytes(&bytes)))
}

pub fn authorize_worktrees(
    root: &Path,
    workspace: &Path,
    allowed_root: &Path,
    formal_state: bool,
    publish: bool,
) -> Result<WorktreePolicy> {
    native::require_unelevated_worker()?;
    let sid = crate::execution_context::execution_context_probe()
        .principal_sid
        .ok_or_else(|| anyhow::anyhow!("owner SID unavailable"))?;
    let protected = ProtectedDirectory::open(root, &sid)?;
    let installation: Installation =
        decode_json(&protected.read_file("installation.json", MAX_REQUEST_BYTES as u64)?)?;
    installation.validate()?;
    if installation.owner_sid != sid || installation.root_identity != protected.identity() {
        bail!("only the installed desktop owner may authorize linked worktrees");
    }
    let anchor = Client::open(root)?.enrollment(workspace)?;
    anchor.validate(&installation)?;
    if anchor.git.is_none()
        || anchor.commit_identity.is_none()
        || !anchor.registration.capabilities.local_commit
        || !anchor.registration.capabilities.added_files
        || !anchor.registration.capabilities.deleted_files
        || (formal_state && !anchor.registration.capabilities.formal_state)
    {
        bail!("anchor does not provide the requested linked-worktree capabilities");
    }
    let allowed = native::SourceDirectory::open(allowed_root)?;
    if allowed.path().parent().is_none() || allowed.path().starts_with(protected.path()) {
        bail!("a volume root cannot be an automatic worktree root");
    }
    let mut rayman = None;
    let mut checkpoint = None;
    let mut powershell: Option<WorktreeProductFile> = None;
    if formal_state {
        for (kind, marker) in [
            ("rayman", ".RaymanCodingSkill/global-state.json"),
            ("save-work-status", ".agent-checkpoints/global-state.json"),
        ] {
            if has_parent_route(&anchor, &installation, marker)? {
                let (binding, shell) = product(&protected, &installation, kind)?;
                if powershell
                    .as_ref()
                    .is_some_and(|old| old.sha256 != shell.sha256 || old.path != shell.path)
                {
                    bail!("product PowerShell identities differ");
                }
                powershell = Some(shell);
                if kind == "rayman" {
                    rayman = Some(binding);
                } else {
                    checkpoint = Some(binding);
                }
            }
        }
    }
    let policy = WorktreePolicy {
        schema_version: 1,
        installation_id: installation.installation_id,
        anchor_worktree_id: anchor.registration.worktree_id.clone(),
        anchor_registration_sha256: anchor.registration.digest()?,
        allowed_root: allowed.path().into(),
        allowed_root_identity: allowed.identity().into(),
        formal_state,
        rayman,
        checkpoint,
        powershell,
    };
    if publish {
        let _lock = crate::state_lock::acquire_state_lock(&protected.path().join("registry"))?;
        let name = format!("worktree-policy-{}.json", anchor.registration.worktree_id);
        let path = protected.path().join(&name);
        if path.try_exists()? {
            let old: WorktreePolicy =
                decode_json(&protected.read_file(&name, MAX_REQUEST_BYTES as u64)?)?;
            if serde_json::to_vec(&old)? != serde_json::to_vec(&policy)? {
                bail!("worktree delegation already exists with different authority");
            }
        } else {
            crate::file_io::write_json(&path, &policy)?;
        }
    }
    Ok(policy)
}

pub(super) fn linked_common_identity(
    protected: &ProtectedDirectory,
    workspace: &Path,
) -> Result<(PathBuf, String, String)> {
    let root = native::SourceDirectory::open(workspace)?;
    let marker = root.path().join(".git");
    let bytes = root.read_file(Path::new(".git"), 4096)?;
    let value = std::str::from_utf8(&bytes)?
        .trim()
        .strip_prefix("gitdir: ")
        .ok_or_else(|| anyhow::anyhow!("linked worktree Git marker is invalid"))?;
    let git = native::SourceDirectory::open(&root.path().join(value))?;
    let common = git.read_file(Path::new("commondir"), 4096)?;
    let common =
        native::SourceDirectory::open(&git.path().join(std::str::from_utf8(&common)?.trim()))?;
    if git.path().parent() != Some(common.path().join("worktrees").as_path()) {
        bail!("Git directory is not a linked worktree of its common repository");
    }
    let backlink = git.read_file(Path::new("gitdir"), 4096)?;
    if std::fs::canonicalize(std::str::from_utf8(&backlink)?.trim())?
        != std::fs::canonicalize(marker)?
    {
        bail!("linked-worktree backlink does not match the target");
    }
    Ok((
        root.path().into(),
        root.identity().into(),
        super::relocation::logical_common_identity(protected, &common)?,
    ))
}

pub(super) fn enrollment_candidate(
    root: &ProtectedDirectory,
    installation: &Installation,
    anchor: &Enrollment,
    workspace: &Path,
    root_identity: &str,
    policy_sha256: &str,
) -> Result<Enrollment> {
    let (policy, digest) = checked_policy(root, installation, anchor)?;
    if digest != policy_sha256 {
        bail!("linked-worktree policy changed");
    }
    let allowed = native::SourceDirectory::open(&policy.allowed_root)?;
    let target = native::SourceDirectory::open(workspace)?;
    if allowed.identity() != policy.allowed_root_identity
        || target.identity() != root_identity
        || target.path() == allowed.path()
        || !target.path().starts_with(allowed.path())
    {
        bail!("worktree is outside its approved root or its identity changed");
    }
    let (_, observed_root, observed_common) = linked_common_identity(root, workspace)?;
    if observed_root != root_identity || observed_common != anchor.registration.git_common_identity
    {
        bail!("worktree is not linked to the owner-approved repository");
    }
    let marker = target.path().join(".git");
    let metadata = std::fs::symlink_metadata(&marker)?;
    if !metadata.is_file() || crate::file_io::is_link_or_reparse(&metadata) {
        bail!("automatic enrollment accepts only ordinary linked-worktree Git files");
    }
    let git = anchor
        .git
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("anchor Git capability missing"))?;
    if crate::hash::sha256_file(&git.executable)? != git.executable_sha256
        || crate::hash::sha256_file(&git.common_directory.join("config"))? != git.config_sha256
    {
        bail!("anchor Git executable or configuration changed");
    }
    let identity = anchor
        .commit_identity
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("anchor commit identity missing"))?;
    let candidate = enroll(
        root.path(),
        target.path(),
        Some((&git.executable, &identity.name, &identity.email)),
        policy.formal_state,
        false,
    )?;
    let binding = candidate
        .git
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("linked Git binding missing"))?;
    if candidate.registration.git_common_identity != anchor.registration.git_common_identity
        || candidate.registration.project_id != anchor.registration.project_id
        || candidate.registration.worktree_id == anchor.registration.worktree_id
        || binding.common_directory != git.common_directory
        || binding.config_sha256 != git.config_sha256
        || !candidate.install_policies.is_empty()
        || !candidate
            .registration
            .capabilities
            .install_adapters
            .is_empty()
    {
        bail!("linked worktree differs from the approved repository or inherits install authority");
    }
    Ok(candidate)
}

pub(super) fn register_linked(
    root: &ProtectedDirectory,
    installation: &Installation,
    anchor: &Enrollment,
    workspace: &Path,
    root_identity: &str,
    policy_sha256: &str,
) -> Result<Enrollment> {
    if !is_sha256(root_identity) || !is_sha256(policy_sha256) {
        bail!("invalid worktree enrollment binding");
    }
    let _lock = crate::state_lock::acquire_state_lock(
        &root
            .path()
            .join(format!("worktree-onboarding-{root_identity}")),
    )?;
    let directory = native::SourceDirectory::open(root.path())?;
    let _policy_pin = directory.pin_file(Path::new(&format!(
        "worktree-policy-{}.json",
        anchor.registration.worktree_id
    )))?;
    let candidate = enrollment_candidate(
        root,
        installation,
        anchor,
        workspace,
        root_identity,
        policy_sha256,
    )?;
    let project_path = root.path().join(format!(
        "project-{}.json",
        candidate.registration.worktree_id
    ));
    let member_path = root.path().join(format!(
        "worktree-member-{}.json",
        candidate.registration.worktree_id
    ));
    let mut member = WorktreeMember {
        schema_version: 1,
        policy_sha256: policy_sha256.into(),
        registration_sha256: candidate.registration.digest()?,
        registered: false,
    };
    if member_path.try_exists()? {
        let previous: WorktreeMember = decode_json(&directory.read_file(
            member_path.strip_prefix(root.path())?,
            MAX_REQUEST_BYTES as u64,
        )?)?;
        if previous.schema_version != 1
            || previous.policy_sha256 != member.policy_sha256
            || previous.registration_sha256 != member.registration_sha256
        {
            bail!("existing worktree membership differs; explicit recovery required");
        }
        member = previous;
    } else {
        if project_path.try_exists()? {
            bail!("existing enrollment cannot be adopted implicitly");
        }
        crate::file_io::write_json(&member_path, &member)?;
    }
    let git = candidate.git.as_ref().unwrap();
    let identity = candidate.commit_identity.as_ref().unwrap();
    let published = enroll(
        root.path(),
        workspace,
        Some((&git.executable, &identity.name, &identity.email)),
        candidate.registration.capabilities.formal_state,
        true,
    )?;
    if published.registration.digest()? != member.registration_sha256 {
        bail!("worktree changed during enrollment");
    }
    member.registered = true;
    crate::file_io::write_json(&member_path, &member)?;
    Ok(published)
}
