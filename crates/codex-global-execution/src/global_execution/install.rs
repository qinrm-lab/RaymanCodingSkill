//! Fixed file publication through the compiled-in official installer kernel.
//! The owner registers destinations. Requests provide only a bundle digest;
//! neither a command nor an installation destination comes from a request.
use super::*;
use crate::hash::sha256_bytes;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

const INSTALLER: &str = include_str!("../../../../scripts/install-rayman.ps1");
const DISPATCH: &str = "\nif ($env:RAYMAN_UPDATE_WORKER -eq '1') {";
const ENTRY: &str = include_str!("install-worker.ps1");
const MAX_FILES: usize = 64;
const MAX_PAYLOAD: u64 = 256 * 1024 * 1024;
const GLOBAL_SKILL: &str = include_str!("../../../../global-skill/SKILL.md");

/// Publish only the compiled global entrypoint; project input cannot select
/// the skill text or destination. Existing different content is preserved.
pub fn publish_global_skill(root: &Path) -> Result<serde_json::Value> {
    publish_global_skill_checked(root, None)
}

pub fn publish_global_skill_checked(
    root: &Path,
    expected_sha256: Option<&str>,
) -> Result<serde_json::Value> {
    super::native::require_unelevated_worker()?;
    if expected_sha256.is_some_and(|hash| !is_sha256(hash)) {
        bail!("invalid expected global skill digest");
    }
    let client = Client::open(root)?;
    let status = client.status()?;
    let context = crate::execution_context::execution_context_probe();
    let sid = context
        .principal_sid
        .ok_or_else(|| anyhow::anyhow!("skill publisher identity unavailable"))?;
    if status["owner_sid"] != sid {
        bail!("global skill publication requires the installation owner");
    }
    let profile = PathBuf::from(
        context
            .token_profile
            .ok_or_else(|| anyhow::anyhow!("owner profile unavailable"))?,
    );
    publish_skill_in_profile_checked(&profile, expected_sha256)
}

#[cfg(test)]
fn publish_skill_in_profile(profile: &Path) -> Result<serde_json::Value> {
    publish_skill_in_profile_checked(profile, None)
}

fn publish_skill_in_profile_checked(
    profile: &Path,
    expected_sha256: Option<&str>,
) -> Result<serde_json::Value> {
    let parent = super::native::SourceDirectory::open(&profile.join(".codex/skills"))?;
    let destination = parent.path().join("codex-global-execution");
    match std::fs::create_dir(&destination) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e.into()),
    }
    let directory = super::native::SourceDirectory::open(&destination)?;
    let expected = GLOBAL_SKILL.as_bytes();
    let previous = if directory.path().join("SKILL.md").try_exists()? {
        let bytes = directory.read_file(Path::new("SKILL.md"), 65536)?;
        if bytes != expected && expected_sha256 != Some(sha256_bytes(&bytes).as_str()) {
            bail!("existing global skill differs; preserve it for explicit upgrade");
        }
        if bytes == expected {
            return Ok(
                serde_json::json!({"published":true,"already_current":true,"path":directory.path().join("SKILL.md"),"sha256":sha256_bytes(expected)}),
            );
        }
        Some(bytes)
    } else {
        None
    };
    let mut nonce = [0u8; 16];
    getrandom::fill(&mut nonce)
        .map_err(|e| anyhow::anyhow!("skill publication entropy unavailable: {e}"))?;
    let seed = format!(
        ".skill-{}",
        nonce.iter().map(|b| format!("{b:02x}")).collect::<String>()
    );
    let mut slot = super::publication::PublicationSlot::create(directory.path(), &seed, expected)?;
    if let Some(before) = &previous {
        slot.preserve_access_from(&directory.path().join("SKILL.md"))?;
        if directory.read_file(Path::new("SKILL.md"), 65536)? != *before {
            bail!("global skill changed concurrently");
        }
    }
    slot.publish_reviewed("SKILL.md", previous.as_deref())?;
    Ok(
        serde_json::json!({"published":true,"already_current":false,"path":directory.path().join("SKILL.md"),"sha256":sha256_bytes(expected)}),
    )
}

pub fn decode_install_sources(bytes: &[u8]) -> Result<BTreeMap<String, String>> {
    decode_json(bytes)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct InstallTarget {
    pub destination: PathBuf,
    pub parent_identity: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct InstallAdapter {
    pub schema_version: u32,
    pub adapter_id: String,
    pub worktree_id: String,
    pub owner_sid: String,
    pub targets: BTreeMap<String, InstallTarget>,
    pub powershell: PathBuf,
    pub powershell_sha256: String,
    pub kernel_sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct InstallBundle {
    pub schema_version: u32,
    pub adapter_id: String,
    pub version: String,
    pub source_sha256: String,
    pub files: Vec<InstallBundleFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct InstallBundleFile {
    pub role: String,
    pub source: String,
    pub sha256: String,
    pub size: u64,
    pub expected_current_sha256: Option<String>,
}

impl InstallBundle {
    pub fn digest(&self) -> Result<String> {
        Ok(sha256_bytes(&serde_json::to_vec(self)?))
    }
    pub(super) fn validate(&self, adapter: &InstallAdapter) -> Result<()> {
        version(&self.version)?;
        if self.schema_version != 1
            || self.adapter_id != adapter.adapter_id
            || !is_sha256(&self.source_sha256)
            || self.files.is_empty()
            || self.files.len() > MAX_FILES
            || self.files.len() != adapter.targets.len()
        {
            bail!("install bundle does not match its fixed adapter");
        }
        let mut previous = None;
        let mut total = 0u64;
        let mut destinations = BTreeSet::new();
        for file in &self.files {
            validate_relative_path(&file.source)?;
            if !adapter.targets.contains_key(&file.role)
                || !is_label(&file.role)
                || previous.is_some_and(|p: &str| p >= file.role.as_str())
                || !is_sha256(&file.sha256)
                || file.size == 0
                || file.size > MAX_PAYLOAD
                || file
                    .expected_current_sha256
                    .as_ref()
                    .is_some_and(|h| !is_sha256(h))
            {
                bail!("invalid, duplicate or unordered install file role");
            }
            previous = Some(file.role.as_str());
            if !destinations.insert(
                crate::pathfmt::display_path(&adapter.targets[&file.role].resolve(&self.version)?)
                    .to_lowercase(),
            ) {
                bail!("versioned install targets collide");
            }
            total = total
                .checked_add(file.size)
                .ok_or_else(|| anyhow::anyhow!("install size overflow"))?;
        }
        if total > MAX_PAYLOAD {
            bail!("install package exceeds its memory bound");
        }
        Ok(())
    }
}

fn version(text: &str) -> Result<[u32; 3]> {
    let parts: Vec<_> = text.split('.').collect();
    if parts.len() != 3
        || parts.iter().any(|p| {
            p.is_empty()
                || p.len() > 9
                || (p.len() > 1 && p.starts_with('0'))
                || !p.bytes().all(|b| b.is_ascii_digit())
        })
    {
        bail!("invalid install version");
    }
    Ok([parts[0].parse()?, parts[1].parse()?, parts[2].parse()?])
}

impl InstallTarget {
    pub(super) fn resolve(&self, release: &str) -> Result<PathBuf> {
        version(release)?;
        let leaf = self
            .destination
            .file_name()
            .and_then(|p| p.to_str())
            .ok_or_else(|| anyhow::anyhow!("invalid install leaf"))?;
        let leaf = leaf.replace("{version}", release);
        validate_relative_path(&leaf)?;
        Ok(self.destination.with_file_name(leaf))
    }
}

fn kernel() -> Result<String> {
    let normalized = INSTALLER.replace("\r\n", "\n");
    let (definitions, _) = normalized
        .split_once(DISPATCH)
        .ok_or_else(|| anyhow::anyhow!("official installer library boundary changed"))?;
    Ok(format!("{definitions}\n{ENTRY}"))
}

impl InstallAdapter {
    pub(super) fn digest(&self) -> Result<String> {
        Ok(sha256_bytes(&serde_json::to_vec(self)?))
    }
    pub(super) fn validate(&self, enrollment: &Enrollment) -> Result<()> {
        if self.schema_version != 1
            || !is_label(&self.adapter_id)
            || self.worktree_id != enrollment.registration.worktree_id
            || self.owner_sid != enrollment.registration.owner_sid
            || self.targets.is_empty()
            || self.targets.len() > MAX_FILES
            || !self.powershell.is_absolute()
            || !is_sha256(&self.powershell_sha256)
            || self.kernel_sha256 != sha256_bytes(kernel()?.as_bytes())
        {
            bail!("install adapter identity or compiled kernel differs");
        }
        let mut destinations = BTreeSet::new();
        for (role, target) in &self.targets {
            if !is_label(role)
                || !target.destination.is_absolute()
                || !is_sha256(&target.parent_identity)
                || !destinations
                    .insert(crate::pathfmt::display_path(&target.destination).to_lowercase())
            {
                bail!("install targets are invalid or aliased");
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct PlanFile {
    role: String,
    source: PathBuf,
    destination: PathBuf,
    new_sha256: String,
    expected_current_sha256: Option<String>,
    expect_absent: bool,
    allow_existing_new: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Plan {
    transaction_id: String,
    version: String,
    request_sha256: String,
    registration_sha256: String,
    adapter_sha256: String,
    journal_path: PathBuf,
    files: Vec<PlanFile>,
}

pub(super) struct PreparedInstall {
    plan: Plan,
    adapter: InstallAdapter,
    _parents: Vec<super::native::SourceDirectory>,
    _program: super::native::SourceFile,
    _lock: crate::state_lock::StateLock,
}

fn adapter_name(worktree: &str, id: &str) -> String {
    format!("install-adapter-{worktree}-{id}.json")
}

pub(super) fn pending_request(root: &ProtectedDirectory) -> Result<Option<Request>> {
    if !root.path().join("install-active.json").try_exists()? {
        return Ok(None);
    }
    let request: Request =
        decode_json(&root.read_file("install-active.json", MAX_REQUEST_BYTES as u64)?)?;
    if !is_id(&request.request_id)
        || !is_id(&request.worktree_id)
        || !matches!(request.operation, Operation::Install { .. })
    {
        bail!("invalid active installation intent");
    }
    Ok(Some(request))
}

pub(super) fn clear_active(root: &ProtectedDirectory, request: &Request) -> Result<()> {
    let Some(active) = pending_request(root)? else {
        return Ok(());
    };
    if active.digest()? != request.digest()? {
        bail!("active installation changed");
    }
    let bytes = root.read_file("install-active.json", MAX_REQUEST_BYTES as u64)?;
    let parent = super::native::SourceDirectory::open(root.path())?;
    if !parent.consume_packet("install-active.json", &bytes)? {
        bail!("active installation marker changed during retirement");
    }
    Ok(())
}

pub(super) fn cancel_unadmitted(root: &ProtectedDirectory, request: &Request) -> Result<()> {
    if root
        .path()
        .join(format!("install-journal-{}.json", request.request_id))
        .try_exists()?
    {
        bail!("installation journal exists without ledger admission; preserve for operator review");
    }
    clear_active(root, request)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AdapterSpec {
    adapter_id: String,
    targets: BTreeMap<String, PathBuf>,
}

/// Owner-only control-plane operation. A queued request cannot call this.
pub fn register_install_adapter(
    root: &Path,
    workspace: &Path,
    specification: &Path,
) -> Result<InstallAdapter> {
    let context = crate::execution_context::execution_context_probe();
    let sid = context
        .principal_sid
        .ok_or_else(|| anyhow::anyhow!("adapter owner unavailable"))?;
    let client = Client::open(root)?;
    let mut enrollment = client.enrollment(workspace)?;
    if sid != enrollment.registration.owner_sid {
        bail!("adapter registration requires the enrolled desktop owner");
    }
    let protected = ProtectedDirectory::open(root, &sid)?;
    let _lock = crate::state_lock::acquire_state_lock(&protected.path().join("registry"))?;
    let profile = PathBuf::from(
        context
            .token_profile
            .ok_or_else(|| anyhow::anyhow!("owner profile unavailable"))?,
    );
    let spec_parent = super::native::SourceDirectory::open(
        specification
            .parent()
            .ok_or_else(|| anyhow::anyhow!("specification parent missing"))?,
    )?;
    let spec: AdapterSpec = decode_json(
        &spec_parent.read_file(
            Path::new(
                specification
                    .file_name()
                    .ok_or_else(|| anyhow::anyhow!("specification leaf missing"))?,
            ),
            MAX_REQUEST_BYTES as u64,
        )?,
    )?;
    if !is_label(&spec.adapter_id) || spec.targets.is_empty() || spec.targets.len() > MAX_FILES {
        bail!("invalid install target specification");
    }
    let mut targets = BTreeMap::new();
    for (role, path) in spec.targets {
        if !path.is_absolute() || !is_label(&role) {
            bail!("invalid install role or target path");
        }
        let parent = super::native::SourceDirectory::open(
            path.parent()
                .ok_or_else(|| anyhow::anyhow!("install parent missing"))?,
        )?;
        let leaf = path
            .file_name()
            .and_then(|p| p.to_str())
            .ok_or_else(|| anyhow::anyhow!("install target leaf missing"))?;
        validate_relative_path(&leaf.replace("{version}", "1.0.0"))?;
        let destination = parent.path().join(leaf);
        let key = crate::pathfmt::display_path(&destination)
            .replace('/', "\\")
            .to_lowercase();
        let permitted = [profile.join("AppData/Local"), profile.join(".codex/skills")]
            .iter()
            .any(|p| {
                key.starts_with(
                    &(crate::pathfmt::display_path(p)
                        .replace('/', "\\")
                        .trim_end_matches('\\')
                        .to_lowercase()
                        + "\\"),
                )
            });
        if !permitted
            || destination.starts_with(protected.path())
            || protected.path().starts_with(&destination)
        {
            bail!("install target is outside the owner application/skill scope");
        }
        targets.insert(
            role,
            InstallTarget {
                destination,
                parent_identity: parent.identity().into(),
            },
        );
    }
    let powershell = PathBuf::from("C:/Program Files/PowerShell/7/pwsh.exe");
    let parent = super::native::SourceDirectory::open(powershell.parent().unwrap())?;
    let bytes = parent.read_file(Path::new("pwsh.exe"), 128 * 1024 * 1024)?;
    let adapter = InstallAdapter {
        schema_version: 1,
        adapter_id: spec.adapter_id,
        worktree_id: enrollment.registration.worktree_id.clone(),
        owner_sid: sid,
        targets,
        powershell: parent.path().join("pwsh.exe"),
        powershell_sha256: sha256_bytes(&bytes),
        kernel_sha256: sha256_bytes(kernel()?.as_bytes()),
    };
    adapter.validate(&enrollment)?;
    let digest = adapter.digest()?;
    if let Some(old) = enrollment.install_policies.get(&adapter.adapter_id) {
        if *old == digest {
            return Ok(adapter);
        }
        bail!("existing install adapter differs; explicit policy migration is required");
    }
    if protected
        .path()
        .join(format!(
            "ledger-{}.json",
            enrollment.registration.worktree_id
        ))
        .try_exists()?
    {
        bail!(
            "register install adapters before first project operation; existing ledger needs policy migration"
        );
    }
    let name = adapter_name(&enrollment.registration.worktree_id, &adapter.adapter_id);
    if protected.path().join(&name).try_exists()? {
        let old: InstallAdapter =
            decode_json(&protected.read_file(&name, MAX_REQUEST_BYTES as u64)?)?;
        if old != adapter {
            bail!("retained adapter differs from proposed policy");
        }
    } else {
        crate::file_io::write_json(&protected.path().join(&name), &adapter)?;
    }
    enrollment
        .install_policies
        .insert(adapter.adapter_id.clone(), digest);
    enrollment
        .registration
        .capabilities
        .install_adapters
        .insert(adapter.adapter_id.clone());
    enrollment.registration.policy_sha256 = enrollment.policy_digest()?;
    crate::file_io::write_json(
        &protected.path().join(format!(
            "project-{}.json",
            enrollment.registration.worktree_id
        )),
        &enrollment,
    )?;
    Ok(adapter)
}

pub(super) fn prepare(
    root: &ProtectedDirectory,
    enrollment: &Enrollment,
    request: &Request,
    recovery: bool,
) -> Result<PreparedInstall> {
    let Operation::Install {
        adapter_id,
        artifact_sha256,
    } = &request.operation
    else {
        bail!("not an install request");
    };
    let adapter: InstallAdapter = decode_json(&root.read_file(
        &adapter_name(&request.worktree_id, adapter_id),
        MAX_REQUEST_BYTES as u64,
    )?)?;
    adapter.validate(enrollment)?;
    if enrollment.install_policies.get(adapter_id) != Some(&adapter.digest()?) {
        bail!("install adapter is not sealed into registration");
    }
    let lock = crate::state_lock::acquire_state_lock(&root.path().join("all-installations"))?;
    if let Some(active) = pending_request(root)?
        && active.digest()? != request.digest()?
    {
        bail!("another installation needs recovery before a new publication");
    }
    let mut parents = Vec::new();
    for target in adapter.targets.values() {
        let parent = super::native::SourceDirectory::open(
            target
                .destination
                .parent()
                .ok_or_else(|| anyhow::anyhow!("target has no parent"))?,
        )?;
        if parent.identity() != target.parent_identity {
            bail!("registered install parent was replaced");
        }
        if target.destination.starts_with(root.path())
            || root.path().starts_with(&target.destination)
        {
            bail!("worker cannot replace its own installation");
        }
        parents.push(parent);
    }
    let program_parent = super::native::SourceDirectory::open(
        adapter
            .powershell
            .parent()
            .ok_or_else(|| anyhow::anyhow!("PowerShell parent missing"))?,
    )?;
    let program = program_parent.pin_file(Path::new(
        adapter
            .powershell
            .file_name()
            .ok_or_else(|| anyhow::anyhow!("PowerShell name missing"))?,
    ))?;
    let program_bytes = crate::file_io::read_bytes_from_handle(
        &program.file,
        program.file.metadata()?.len(),
        &program.path,
        "fixed PowerShell runtime",
    )?;
    if sha256_bytes(&program_bytes) != adapter.powershell_sha256 {
        bail!("registered PowerShell runtime changed");
    }
    let plan_name = format!("install-plan-{}.json", request.request_id);
    let plan = if recovery {
        decode_json::<Plan>(&root.read_file(&plan_name, MAX_REQUEST_BYTES as u64)?)?
    } else {
        if root.path().join(&plan_name).try_exists()? {
            bail!("install plan already exists without admitted intent");
        }
        let source = super::native::SourceDirectory::open(&enrollment.workspace)?;
        let manifest = source.read_file(
            Path::new(&format!(
                ".RaymanCodingSkill/tmp/install-packages/{artifact_sha256}.json"
            )),
            MAX_REQUEST_BYTES as u64,
        )?;
        let bundle: InstallBundle = decode_json(&manifest)?;
        bundle.validate(&adapter)?;
        if bundle.digest()? != *artifact_sha256 || bundle.source_sha256 != request.source_sha256 {
            bail!("install bundle identity differs");
        }
        let mut files = Vec::new();
        for (index, file) in bundle.files.iter().enumerate() {
            let bytes = source.read_file(Path::new(&file.source), file.size)?;
            if bytes.len() as u64 != file.size || sha256_bytes(&bytes) != file.sha256 {
                bail!("install payload bytes differ");
            }
            let target = &adapter.targets[&file.role];
            let parent =
                super::native::SourceDirectory::open(target.destination.parent().unwrap())?;
            let destination = target.resolve(&bundle.version)?;
            let current = if destination.try_exists()? {
                Some(sha256_bytes(&parent.read_file(
                    Path::new(destination.file_name().unwrap()),
                    MAX_PAYLOAD,
                )?))
            } else {
                None
            };
            if current != file.expected_current_sha256 {
                bail!("install destination changed before staging");
            }
            let name = format!("install-payload-{}-{index:04x}", request.request_id);
            let staged = root.path().join(name);
            use std::io::Write;
            let mut output = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&staged)?;
            output.write_all(&bytes)?;
            output.sync_all()?;
            files.push(PlanFile {
                role: file.role.clone(),
                source: staged,
                destination,
                new_sha256: file.sha256.clone(),
                expected_current_sha256: current.clone(),
                expect_absent: current.is_none(),
                allow_existing_new: false,
            });
        }
        Plan {
            transaction_id: request.request_id.clone(),
            version: bundle.version,
            request_sha256: request.digest()?,
            registration_sha256: enrollment.registration.digest()?,
            adapter_sha256: adapter.digest()?,
            journal_path: root
                .path()
                .join(format!("install-journal-{}.json", request.request_id)),
            files,
        }
    };
    if plan.transaction_id != request.request_id
        || plan.request_sha256 != request.digest()?
        || plan.registration_sha256 != enrollment.registration.digest()?
        || plan.adapter_sha256 != adapter.digest()?
        || plan.files.len() != adapter.targets.len()
        || plan.journal_path
            != root
                .path()
                .join(format!("install-journal-{}.json", request.request_id))
    {
        bail!("install recovery plan binding differs");
    }
    for (index, file) in plan.files.iter().enumerate() {
        if adapter
            .targets
            .get(&file.role)
            .map(|t| t.resolve(&plan.version))
            .transpose()?
            != Some(file.destination.clone())
            || file.source
                != root.path().join(format!(
                    "install-payload-{}-{index:04x}",
                    request.request_id
                ))
            || sha256_bytes(&root.read_file(
                file.source.file_name().unwrap().to_str().unwrap(),
                MAX_PAYLOAD,
            )?) != file.new_sha256
        {
            bail!("install recovery file binding differs");
        }
    }
    if !recovery {
        crate::file_io::write_json(&root.path().join(plan_name), &plan)?;
        crate::file_io::write_json(&root.path().join("install-active.json"), request)?;
    }
    Ok(PreparedInstall {
        plan,
        adapter,
        _parents: parents,
        _program: program,
        _lock: lock,
    })
}

impl PreparedInstall {
    pub(super) fn execute(
        &self,
        root: &ProtectedDirectory,
        recovery: bool,
    ) -> Result<serde_json::Value> {
        self.run(root, recovery, &kernel()?)
    }
    fn run(
        &self,
        root: &ProtectedDirectory,
        recovery: bool,
        script: &str,
    ) -> Result<serde_json::Value> {
        let plan = serde_json::to_string(&self.plan)?;
        if plan.encode_utf16().count() > 24000 {
            bail!("install plan exceeds the fixed environment bound");
        }
        let mut env = BTreeMap::new();
        for name in [
            "SystemRoot",
            "WINDIR",
            "TEMP",
            "TMP",
            "USERPROFILE",
            "LOCALAPPDATA",
            "PATH",
        ] {
            if let Ok(value) = std::env::var(name) {
                env.insert(name.into(), value);
            }
        }
        env.insert("RAYMAN_UPDATE_WORKER".into(), "1".into());
        env.insert("RAYMAN_GLOBAL_INSTALL_PLAN".into(), plan);
        env.insert(
            "RAYMAN_GLOBAL_INSTALL_RECOVERY".into(),
            if recovery { "1" } else { "0" }.into(),
        );
        env.insert("POWERSHELL_TELEMETRY_OPTOUT".into(), "1".into());
        // A blank line terminates the compound statement in -Command - mode.
        let wrapped = format!(
            "[Console]::OutputEncoding=[Text.UTF8Encoding]::new($false)\ntry {{ & {{\n{script}\n}}; exit 0 }} catch {{ [Console]::Error.WriteLine($_.Exception.Message); exit 1 }}\n\n"
        );
        let output = super::process::run_single_process(
            &self.adapter.powershell,
            &[
                "-NoProfile".into(),
                "-NonInteractive".into(),
                "-Command".into(),
                "-".into(),
            ],
            root.path(),
            &env,
            wrapped.as_bytes(),
            60000,
            1024 * 1024,
        )?;
        if output.exit_code != 0 {
            bail!(
                "fixed installation kernel failed (exit {}); retained plan requires recovery: {}",
                output.exit_code,
                String::from_utf8_lossy(&output.stderr)
                    .chars()
                    .take(2048)
                    .collect::<String>()
            );
        }
        let result: serde_json::Value = serde_json::from_slice(&output.stdout)?;
        if result["schema"] != "rayman.global-install-outcome.v1"
            || result["transaction_id"] != self.plan.transaction_id
            || !matches!(
                result["status"].as_str(),
                Some("files_published" | "rolled_back" | "not_started")
            )
        {
            bail!("installation kernel outcome is invalid");
        }
        if result["status"] == "rolled_back" {
            for file in &self.plan.files {
                // The upstream updater may retain a newly introduced versioned
                // worker. Generic destinations are not necessarily inert. A
                // byte match alone cannot prove creation ownership after a
                // crash, so preserve the file and the active intent for review.
                if file.expect_absent && file.destination.try_exists()? {
                    bail!(
                        "initially absent installation destination remains; owner recovery required: {}",
                        file.destination.display()
                    );
                }
            }
        }
        Ok(result)
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    struct Fixture {
        root: ProtectedDirectory,
        enrollment: Enrollment,
        request: Request,
        bundle: InstallBundle,
        target: tempfile::TempDir,
        _workspace: tempfile::TempDir,
        _root_dir: tempfile::TempDir,
    }
    fn fixture() -> Fixture {
        let root_dir = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        let sid = crate::execution_context::execution_context_probe()
            .principal_sid
            .unwrap();
        super::super::native::protect_fixture_directory(root_dir.path(), &sid, "").unwrap();
        let root = ProtectedDirectory::open(root_dir.path(), &sid).unwrap();
        let source = super::super::native::SourceDirectory::open(workspace.path()).unwrap();
        let mut registration = super::super::tests::registration();
        registration.owner_sid = sid.clone();
        registration.root_identity = source.identity().into();
        registration.capabilities.install_adapters = BTreeSet::from(["fixture".into()]);
        let parent = super::super::native::SourceDirectory::open(target.path()).unwrap();
        let mut targets = BTreeMap::new();
        for role in ["app", "resource"] {
            let destination = parent.path().join(format!("{role}.txt"));
            std::fs::write(&destination, b"old").unwrap();
            std::fs::write(
                workspace.path().join(format!("{role}.txt")),
                format!("new-{role}"),
            )
            .unwrap();
            targets.insert(
                role.into(),
                InstallTarget {
                    destination,
                    parent_identity: parent.identity().into(),
                },
            );
        }
        let powershell = std::fs::canonicalize("C:/Program Files/PowerShell/7/pwsh.exe").unwrap();
        let adapter = InstallAdapter {
            schema_version: 1,
            adapter_id: "fixture".into(),
            worktree_id: registration.worktree_id.clone(),
            owner_sid: sid,
            targets,
            powershell_sha256: crate::hash::sha256_file(&powershell).unwrap(),
            powershell,
            kernel_sha256: sha256_bytes(kernel().unwrap().as_bytes()),
        };
        let mut enrollment = Enrollment {
            schema_version: 1,
            installation_id: "1".repeat(32),
            registration,
            workspace: source.path().into(),
            source_policy: SourcePolicy::WorkspaceFingerprintV1,
            git: None,
            commit_identity: None,
            install_policies: BTreeMap::from([("fixture".into(), adapter.digest().unwrap())]),
        };
        enrollment.registration.capabilities.local_commit = false;
        enrollment.registration.capabilities.added_files = false;
        enrollment.registration.capabilities.deleted_files = false;
        enrollment.registration.policy_sha256 = enrollment.policy_digest().unwrap();
        crate::file_io::write_json(
            &root.path().join(adapter_name(
                &enrollment.registration.worktree_id,
                "fixture",
            )),
            &adapter,
        )
        .unwrap();
        let sha = crate::source_fingerprint(workspace.path()).unwrap();
        let bundle = InstallBundle {
            schema_version: 1,
            adapter_id: "fixture".into(),
            version: "1.0.0".into(),
            source_sha256: sha.clone(),
            files: ["app", "resource"]
                .into_iter()
                .map(|role| InstallBundleFile {
                    role: role.into(),
                    source: format!("{role}.txt"),
                    sha256: sha256_bytes(format!("new-{role}").as_bytes()),
                    size: format!("new-{role}").len() as u64,
                    expected_current_sha256: Some(sha256_bytes(b"old")),
                })
                .collect(),
        };
        let package = workspace
            .path()
            .join(".RaymanCodingSkill/tmp/install-packages");
        std::fs::create_dir_all(&package).unwrap();
        crate::file_io::write_json(
            &package.join(format!("{}.json", bundle.digest().unwrap())),
            &bundle,
        )
        .unwrap();
        let mut request = super::super::tests::request(&enrollment.registration);
        request.source_sha256 = sha;
        request.operation = Operation::Install {
            adapter_id: "fixture".into(),
            artifact_sha256: bundle.digest().unwrap(),
        };
        Fixture {
            _root_dir: root_dir,
            _workspace: workspace,
            target,
            root,
            enrollment,
            request,
            bundle,
        }
    }
    #[cfg(windows)]
    #[test]
    fn installation_bundle_rejects_unregistered_paths_versions_and_duplicate_roles() {
        let f = fixture();
        let adapter: InstallAdapter = decode_json(
            &f.root
                .read_file(
                    &adapter_name(&f.request.worktree_id, "fixture"),
                    MAX_REQUEST_BYTES as u64,
                )
                .unwrap(),
        )
        .unwrap();
        f.bundle.validate(&adapter).unwrap();
        for path in ["../credentials", "C:/outside", ".git/config", "a:stream"] {
            let mut changed = f.bundle.clone();
            changed.files[0].source = path.into();
            assert!(changed.validate(&adapter).is_err());
        }
        let mut changed = f.bundle.clone();
        changed.files[1].role = changed.files[0].role.clone();
        assert!(changed.validate(&adapter).is_err());
        for release in ["../1", "01.0.0", "1.0", "1.0.0/evil"] {
            assert!(version(release).is_err());
        }
        let json = serde_json::to_string_pretty(&f.bundle).unwrap();
        assert!(decode_install_sources(br#"{"app":"a.exe","app":"b.exe"}"#).is_err());
        assert!(decode_install_sources(br#"{"app":"a.exe","App":"b.exe"}"#).is_err());
        let crlf: InstallBundle = decode_json(json.replace('\n', "\r\n").as_bytes()).unwrap();
        assert_eq!(f.bundle.digest().unwrap(), crlf.digest().unwrap());
    }

    #[cfg(windows)]
    #[test]
    fn global_skill_publication_is_idempotent_and_preserves_different_content() {
        let profile = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(profile.path().join(".codex/skills")).unwrap();
        assert_eq!(
            publish_skill_in_profile(profile.path()).unwrap()["already_current"],
            false
        );
        assert_eq!(
            publish_skill_in_profile(profile.path()).unwrap()["already_current"],
            true
        );
        let target = profile
            .path()
            .join(".codex/skills/codex-global-execution/SKILL.md");
        std::fs::write(&target, b"user maintained entry").unwrap();
        assert!(publish_skill_in_profile(profile.path()).is_err());
        assert_eq!(std::fs::read(target).unwrap(), b"user maintained entry");
    }
    #[cfg(windows)]
    #[test]
    fn global_skill_upgrade_requires_exact_reviewed_previous_bytes() {
        let profile = tempfile::tempdir().unwrap();
        let directory = profile.path().join(".codex/skills/codex-global-execution");
        std::fs::create_dir_all(&directory).unwrap();
        let target = directory.join("SKILL.md");
        let previous = b"reviewed previous skill\r\n";
        std::fs::write(&target, previous).unwrap();
        assert!(publish_skill_in_profile_checked(profile.path(), Some(&"0".repeat(64))).is_err());
        assert_eq!(std::fs::read(&target).unwrap(), previous);
        publish_skill_in_profile_checked(profile.path(), Some(&sha256_bytes(previous))).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), GLOBAL_SKILL.as_bytes());
        assert_eq!(
            publish_skill_in_profile_checked(profile.path(), Some(&sha256_bytes(previous)))
                .unwrap()["already_current"],
            true
        );
    }
    #[cfg(windows)]
    #[test]
    fn installation_kernel_publishes_exact_files_and_replays_committed_journal() {
        let f = fixture();
        let prepared = prepare(&f.root, &f.enrollment, &f.request, false).unwrap();
        let outcome = prepared.execute(&f.root, false).unwrap();
        assert_eq!(outcome["status"], "files_published");
        assert_eq!(outcome["project_code_executed"], false);
        drop(prepared);
        for role in ["app", "resource"] {
            let target = super::super::native::SourceDirectory::open(f.target.path()).unwrap();
            assert_eq!(
                target
                    .read_file(Path::new(&format!("{role}.txt")), 1024)
                    .unwrap(),
                format!("new-{role}").as_bytes()
            );
            assert_eq!(
                std::fs::read(f.target.path().join(format!("{role}.txt"))).unwrap(),
                format!("new-{role}").as_bytes()
            );
        }
        let recovered = prepare(&f.root, &f.enrollment, &f.request, true).unwrap();
        assert_eq!(
            recovered.execute(&f.root, true).unwrap()["status"],
            "files_published"
        );
    }
    #[cfg(windows)]
    #[test]
    fn installation_kernel_rolls_back_failure_and_refuses_conflicting_destination() {
        let f = fixture();
        let prepared = prepare(&f.root, &f.enrollment, &f.request, false).unwrap();
        let injected = kernel().unwrap().replace(
            "# GLOBAL_INSTALL_FAILURE_INJECTION_POINT (test builds only)",
            "if ($index -eq 0) { throw 'injected install failure' }",
        );
        assert!(prepared.run(&f.root, false, &injected).is_err());
        for role in ["app", "resource"] {
            assert_eq!(
                std::fs::read(f.target.path().join(format!("{role}.txt"))).unwrap(),
                b"old"
            );
        }
        assert_eq!(
            prepared.execute(&f.root, true).unwrap()["status"],
            "rolled_back"
        );
        drop(prepared);
        clear_active(&f.root, &f.request).unwrap();
        let mut second = f.request.clone();
        second.request_id = "e".repeat(32);
        std::fs::write(f.target.path().join("app.txt"), b"concurrent").unwrap();
        assert!(prepare(&f.root, &f.enrollment, &second, false).is_err());
        assert_eq!(
            std::fs::read(f.target.path().join("app.txt")).unwrap(),
            b"concurrent"
        );
    }

    #[cfg(windows)]
    #[test]
    fn installation_crash_recovery_never_claims_rollback_with_new_file_remaining() {
        let mut f = fixture();
        let target = f.target.path().join("app.txt");
        std::fs::remove_file(&target).unwrap();
        f.bundle.files[0].expected_current_sha256 = None;
        let digest = f.bundle.digest().unwrap();
        crate::file_io::write_json(
            &f.enrollment.workspace.join(format!(
                ".RaymanCodingSkill/tmp/install-packages/{digest}.json"
            )),
            &f.bundle,
        )
        .unwrap();
        f.request.operation = Operation::Install {
            adapter_id: "fixture".into(),
            artifact_sha256: digest,
        };
        let prepared = prepare(&f.root, &f.enrollment, &f.request, false).unwrap();
        prepared.execute(&f.root, false).unwrap();
        // Model a crash after file publication but before the commit marker.
        let path = &prepared.plan.journal_path;
        let mut journal: serde_json::Value =
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        journal["phase"] = "blocked".into();
        journal["committed"] = false.into();
        crate::file_io::write_json(path, &journal).unwrap();
        let error = prepared.execute(&f.root, true).unwrap_err().to_string();
        assert!(error.contains("initially absent installation destination remains"));
        assert_eq!(std::fs::read(target).unwrap(), b"new-app");
        assert_eq!(
            std::fs::read(f.target.path().join("resource.txt")).unwrap(),
            b"old"
        );
        assert!(pending_request(&f.root).unwrap().is_some());
    }

    fn open_worker(f: &Fixture) -> Worker {
        std::fs::write(f.root.path().join("worker.exe"), b"worker-fixture").unwrap();
        std::fs::write(f.root.path().join("client.exe"), b"client-fixture").unwrap();
        std::fs::create_dir_all(f.root.path().join("requests")).unwrap();
        let installation = Installation {
            schema_version: 1,
            installation_id: f.enrollment.installation_id.clone(),
            owner_sid: f.enrollment.registration.owner_sid.clone(),
            root_identity: f.root.identity().into(),
            worker_sha256: sha256_bytes(b"worker-fixture"),
            client_sha256: sha256_bytes(b"client-fixture"),
            source_project_id: "0".repeat(32),
        };
        crate::file_io::write_json(&f.root.path().join("installation.json"), &installation)
            .unwrap();
        crate::file_io::write_json(
            &f.root
                .path()
                .join(format!("project-{}.json", f.request.worktree_id)),
            &f.enrollment,
        )
        .unwrap();
        Worker::open_install_fixture(f.root.path(), &f.root.path().join("worker.exe")).unwrap()
    }
    #[cfg(windows)]
    #[test]
    fn installation_worker_dispatch_and_restart_reconcile_the_original_intent() {
        let f = fixture();
        let worker = open_worker(&f);
        let bytes = serde_json::to_vec(&f.request).unwrap();
        let result = worker.process(&bytes, 1001).unwrap();
        assert!(matches!(
            result.result,
            RecordedResult::EffectSucceeded { .. }
        ));
        assert!(pending_request(&f.root).unwrap().is_none());
        assert!(worker.process(&bytes, 2000).unwrap().replayed);
        drop(worker);
        let fresh = fixture();
        let prepared = prepare(&fresh.root, &fresh.enrollment, &fresh.request, false).unwrap();
        let store =
            StateStorage::at_existing_root(fresh.root.path(), &fresh.enrollment.registration)
                .unwrap();
        store
            .accept(&fresh.request, &fresh.enrollment.registration, 1001)
            .unwrap();
        prepared.execute(&fresh.root, false).unwrap();
        drop(prepared); // Simulate process loss before the result/active marker.
        let worker = open_worker(&fresh);
        assert_eq!(worker.cycle(2000).unwrap(), 0);
        assert!(pending_request(&fresh.root).unwrap().is_none());
        assert!(matches!(
            store
                .recorded_result(&fresh.request, &fresh.enrollment.registration)
                .unwrap(),
            Some(RecordedResult::EffectSucceeded { .. })
        ));
    }
    #[cfg(windows)]
    #[test]
    fn installation_pending_intent_blocks_other_publications_without_ledger_admission() {
        let f = fixture();
        let prepared = prepare(&f.root, &f.enrollment, &f.request, false).unwrap();
        drop(prepared);
        let mut other = f.request.clone();
        other.request_id = "e".repeat(32);
        assert!(prepare(&f.root, &f.enrollment, &other, false).is_err());
        assert!(clear_active(&f.root, &other).is_err());
        let worker = open_worker(&f);
        assert_eq!(worker.cycle(2000).unwrap(), 0);
        assert!(pending_request(&f.root).unwrap().is_none());
        assert_eq!(
            std::fs::read(f.target.path().join("app.txt")).unwrap(),
            b"old"
        );
    }
}
