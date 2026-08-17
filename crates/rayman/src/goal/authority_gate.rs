use super::*;

/// The immutable logical identity of a repository-owned PowerShell gate.
///
/// These keys are deliberately stricter than a filesystem path.  A receipt may
/// name only this spelling; aliases, absolute paths and traversal never get to
/// borrow the trust attached to a reviewed gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum GateKind {
    CheckRepo,
    AuditRepository,
    VerifyReleaseContract,
    ReleaseCloseout,
    InstallRayman,
    CheckAgentInstructions,
}

impl GateKind {
    fn from_exact_key(key: &str) -> Option<Self> {
        match key {
            "scripts/check-repo.ps1" => Some(Self::CheckRepo),
            "scripts/audit-repository.ps1" => Some(Self::AuditRepository),
            "scripts/verify-release-contract.ps1" => Some(Self::VerifyReleaseContract),
            "scripts/release-closeout.ps1" => Some(Self::ReleaseCloseout),
            "scripts/install-rayman.ps1" => Some(Self::InstallRayman),
            "scripts/check-agent-instructions.ps1" => Some(Self::CheckAgentInstructions),
            _ => None,
        }
    }

    pub(super) fn name(self) -> &'static str {
        match self {
            Self::CheckRepo => "check-repo.ps1",
            Self::AuditRepository => "audit-repository.ps1",
            Self::VerifyReleaseContract => "verify-release-contract.ps1",
            Self::ReleaseCloseout => "release-closeout.ps1",
            Self::InstallRayman => "install-rayman.ps1",
            Self::CheckAgentInstructions => "check-agent-instructions.ps1",
        }
    }

    fn key(self) -> &'static str {
        match self {
            Self::CheckRepo => "scripts/check-repo.ps1",
            Self::AuditRepository => "scripts/audit-repository.ps1",
            Self::VerifyReleaseContract => "scripts/verify-release-contract.ps1",
            Self::ReleaseCloseout => "scripts/release-closeout.ps1",
            Self::InstallRayman => "scripts/install-rayman.ps1",
            Self::CheckAgentInstructions => "scripts/check-agent-instructions.ps1",
        }
    }

    fn is_reserved_basename(name: &str) -> bool {
        [
            Self::CheckRepo,
            Self::AuditRepository,
            Self::VerifyReleaseContract,
            Self::ReleaseCloseout,
            Self::InstallRayman,
            Self::CheckAgentInstructions,
        ]
        .into_iter()
        .any(|kind| kind.name().eq_ignore_ascii_case(name))
    }

    fn workspace_wide(self) -> bool {
        matches!(
            self,
            Self::CheckRepo | Self::AuditRepository | Self::VerifyReleaseContract
        )
    }
}

/// A repository gate basename is reserved even outside its reviewed logical
/// path.  A same-named local script must not inherit the broad ordinary-script
/// relevance rule: it can be recorded as generic evidence, but cannot claim to
/// validate Rust/Cargo changes or a typed release obligation.
pub(super) fn powershell_script_has_reserved_gate_basename(
    command: &ParsedValidationCommand,
) -> bool {
    powershell_script(command)
        .and_then(|script| Path::new(script).file_name())
        .and_then(|name| name.to_str())
        .is_some_and(GateKind::is_reserved_basename)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CapturedGateIdentity {
    pub kind: GateKind,
    pub entrypoint: String,
}

#[derive(Debug, Clone)]
pub struct LiveWorkspaceScript {
    pub canonical_path: PathBuf,
    pub logical_key: String,
    launch_argument: String,
}

impl LiveWorkspaceScript {
    /// PowerShell rejects Windows verbatim paths during its authorization
    /// check. This argument is safe to spawn only because resolution proved
    /// that canonicalizing it returns the exact identity above.
    pub fn launch_argument(&self) -> &str {
        &self.launch_argument
    }
}

fn powershell_launch_argument(canonical_path: &Path) -> Result<String> {
    powershell_launch_argument_with(canonical_path, |path| path.canonicalize())
}

fn powershell_launch_argument_with<F>(canonical_path: &Path, canonicalize: F) -> Result<String>
where
    F: FnOnce(&Path) -> std::io::Result<PathBuf>,
{
    let launch_argument = crate::pathfmt::display_path(canonical_path);
    let launch_path = Path::new(&launch_argument);
    if !launch_path.is_absolute() {
        bail!("PowerShell validation launch path must be absolute");
    }
    let launch_identity = canonicalize(launch_path)
        .context("PowerShell validation launch path could not be canonicalized")?;
    if launch_identity != canonical_path {
        bail!("PowerShell validation launch path changed the canonical script identity");
    }
    Ok(launch_argument)
}

fn strict_authority_key(raw: &str, require_ps1: bool) -> Result<String> {
    if raw.is_empty()
        || raw.trim() != raw
        || raw.ends_with('.')
        || raw.ends_with(' ')
        || raw.contains(':')
        || raw.contains('\0')
        || raw.starts_with('/')
        || raw.starts_with('\\')
        || raw.starts_with("//")
        || raw.starts_with("\\\\?\\")
        || raw.starts_with("\\\\.\\")
        || raw.len() >= 2 && raw.as_bytes()[1] == b':'
    {
        bail!("PowerShell authority script must use one exact repository logical key");
    }
    if raw.contains('\\') || raw.contains("//") {
        bail!("PowerShell authority script key must use canonical '/' separators");
    }
    let components = raw.split('/').collect::<Vec<_>>();
    if components.is_empty()
        || components.iter().any(|component| {
            component.is_empty()
                || matches!(*component, "." | "..")
                || component.ends_with('.')
                || component.ends_with(' ')
        })
    {
        bail!("PowerShell authority script key is not canonical");
    }
    if require_ps1 && !raw.to_ascii_lowercase().ends_with(".ps1") {
        bail!("PowerShell authority script key must name a .ps1 file");
    }
    Ok(raw.to_string())
}

fn strict_gate_kind(command: &ParsedValidationCommand) -> Result<Option<GateKind>> {
    let Some(script) = powershell_script(command) else {
        return Ok(None);
    };
    let key = strict_authority_key(script, true)?;
    Ok(GateKind::from_exact_key(&key))
}

fn captured_workspace_relative_powershell_key(raw: &str) -> Result<String> {
    if raw.is_empty()
        || raw.trim() != raw
        || raw.contains('\0')
        || raw.contains(':')
        || raw.starts_with('/')
        || raw.starts_with('\\')
    {
        bail!("captured PowerShell validation script must use an ordinary workspace path");
    }
    if raw.contains('\\') {
        bail!("captured PowerShell validation script must use canonical '/' separators");
    }
    let key = raw.to_string();
    if key.split('/').any(|component| {
        component.is_empty()
            || matches!(component, "." | "..")
            || component.ends_with('.')
            || component.ends_with(' ')
    }) || !key.to_ascii_lowercase().ends_with(".ps1")
    {
        bail!("captured PowerShell validation script must name an ordinary workspace .ps1 file");
    }
    Ok(key)
}

fn captured_workspace_absolute_powershell_key(root: &Path, raw: &str) -> Result<String> {
    let _ = (root, raw);
    bail!("captured PowerShell validation script must use a workspace-relative path")
}

fn case_probe_key(key: &str) -> String {
    let mut probe = key.as_bytes().to_vec();
    if let Some(byte) = probe.iter_mut().find(|byte| byte.is_ascii_alphabetic()) {
        *byte = if byte.is_ascii_lowercase() {
            byte.to_ascii_uppercase()
        } else {
            byte.to_ascii_lowercase()
        };
    }
    String::from_utf8(probe).expect("ASCII case conversion preserves UTF-8")
}

/// Capture-only identity for an ordinary workspace-owned PowerShell validation
/// script. It performs only lexical path mapping and immutable capture lookups;
/// it never opens or canonicalizes a live path. This deliberately differs from
/// strict repository-gate identity, so aliases cannot borrow authority merely
/// because they map to the same captured file.
pub(super) fn captured_workspace_powershell_key_with_context(
    decision: &GoalDecisionContext<'_>,
    command: &ParsedValidationCommand,
) -> Result<Option<String>> {
    let Some(script) = powershell_script(command) else {
        return Ok(None);
    };
    let requested = Path::new(script);
    let key = if requested.is_absolute() {
        captured_workspace_absolute_powershell_key(decision.root(), script)?
    } else {
        captured_workspace_relative_powershell_key(script)?
    };
    let Some(captured) = decision.captured_workspace_file(&key)? else {
        return Ok(None);
    };
    let captured_key = captured_workspace_relative_powershell_key(&captured.key)?;

    // `captured_workspace_file` deliberately gives an exact key priority. For
    // an ordinary validation path we additionally need a unique Windows
    // logical identity: probe a different ASCII spelling so a case-colliding
    // capture cannot silently select one of two files. The probe itself stays
    // inside the immutable capture API and never reopens the workspace.
    if cfg!(windows) {
        let probe = case_probe_key(&captured_key);
        match decision.captured_workspace_file(&probe)? {
            Some(replayed) if replayed.key == captured.key => {}
            Some(replayed) => bail!(
                "captured PowerShell validation script is ambiguous under Windows case rules: {} vs {}",
                captured.key,
                replayed.key
            ),
            None => bail!(
                "captured PowerShell validation script cannot prove a unique Windows logical key: {}",
                captured.key
            ),
        }
    }
    Ok(Some(captured_key))
}

/// Capture-only gate identity. It reads no path after the caller captured the
/// workspace bytes, and therefore cannot be fooled by a transient on-disk gate.
pub(super) fn trusted_gate_script_identity_with_context(
    decision: &GoalDecisionContext<'_>,
    command: &ParsedValidationCommand,
) -> Result<Option<CapturedGateIdentity>> {
    let Some(kind) = strict_gate_kind(command)? else {
        return Ok(None);
    };
    let entrypoint = kind.key();
    let Some(captured) = decision.captured_workspace_file(entrypoint)? else {
        return Ok(None);
    };
    if captured.key != entrypoint {
        bail!(
            "trusted PowerShell gate must use the exact captured key {entrypoint}, not {}",
            captured.key
        );
    }
    Ok(Some(CapturedGateIdentity {
        kind,
        entrypoint: entrypoint.to_string(),
    }))
}

/// Resolve a PowerShell file just before execution.  This type intentionally
/// contains no capture bytes: it is only safe to hand to the spawn path.
pub fn resolve_live_powershell_script(
    root: &Path,
    command: &ParsedValidationCommand,
) -> Result<Option<LiveWorkspaceScript>> {
    let Some(script) = powershell_script(command) else {
        return Ok(None);
    };
    let requested = Path::new(script);
    let path = if requested.is_absolute() {
        requested.to_path_buf()
    } else {
        if requested.as_os_str().is_empty()
            || requested
                .components()
                .any(|component| !matches!(component, std::path::Component::Normal(_)))
        {
            bail!("PowerShell validation script must use a normal workspace-relative path");
        }
        root.join(requested)
    };
    crate::context::ensure_source_file(root, &path)?;
    let canonical_root = root.canonicalize()?;
    let canonical_path = path.canonicalize()?;
    let relative = canonical_path.strip_prefix(&canonical_root).map_err(|_| {
        anyhow::anyhow!("PowerShell validation script must remain inside the workspace")
    })?;
    if relative.as_os_str().is_empty()
        || relative
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
        || !canonical_path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("ps1"))
    {
        bail!("PowerShell validation script must be an ordinary workspace .ps1 file");
    }
    let logical_key = relative.to_string_lossy().replace('\\', "/");
    let launch_argument = powershell_launch_argument(&canonical_path)?;
    Ok(Some(LiveWorkspaceScript {
        canonical_path,
        logical_key,
        launch_argument,
    }))
}

/// Resolve a recognized repository gate script by identity, not by file name.
pub(super) fn trusted_gate_script_identity(
    root: &Path,
    command: &ParsedValidationCommand,
) -> Option<(&'static str, PathBuf)> {
    let kind = strict_gate_kind(command).ok().flatten()?;
    let script = resolve_live_powershell_script(root, command)
        .ok()
        .flatten()?;
    if script.logical_key != kind.key() {
        return None;
    }
    Some((kind.name(), script.canonical_path))
}

pub(super) fn trusted_gate_script(
    root: &Path,
    command: &ParsedValidationCommand,
) -> Option<&'static str> {
    trusted_gate_script_identity(root, command).map(|(name, _)| name)
}

pub(super) fn trusted_gate_script_with_context(
    decision: &GoalDecisionContext<'_>,
    command: &ParsedValidationCommand,
) -> Result<Option<&'static str>> {
    Ok(trusted_gate_script_identity_with_context(decision, command)?.map(|gate| gate.kind.name()))
}

/// Workspace-wide project gates whose contract covers the whole repository.
pub(super) fn trusted_workspace_gate_script(
    root: &Path,
    command: &ParsedValidationCommand,
) -> bool {
    trusted_gate_script(root, command).is_some_and(|name| {
        matches!(
            name,
            "check-repo.ps1" | "audit-repository.ps1" | "verify-release-contract.ps1"
        )
    })
}

pub(super) fn trusted_workspace_gate_script_with_context(
    decision: &GoalDecisionContext<'_>,
    command: &ParsedValidationCommand,
) -> Result<bool> {
    Ok(
        trusted_gate_script_identity_with_context(decision, command)?
            .is_some_and(|gate| gate.kind.workspace_wide()),
    )
}

const AUTHORITY_GATE_BINDING_POLICY_V1: &str = "powershell_repository_gate_closure_v1";

fn dependency_key(parent: &str, literal: &str) -> Result<String> {
    if literal.is_empty()
        || literal != literal.trim()
        || literal.starts_with('/')
        || literal.starts_with('\\')
        || literal.starts_with("//")
        || literal.contains(':')
        || literal.contains('\0')
        || literal.contains('\\')
    {
        bail!("PowerShell gate dependency must be a canonical repository-relative key: {literal}");
    }
    let mut parts = parent.split('/').collect::<Vec<_>>();
    if parts.pop().is_none() {
        bail!("PowerShell gate entrypoint has no parent: {parent}");
    }
    let literal = literal.strip_prefix("scripts/").unwrap_or(literal);
    let literal_parts = literal.split('/').collect::<Vec<_>>();
    for component in &literal_parts {
        if component.is_empty()
            || matches!(*component, "." | "..")
            || component.ends_with('.')
            || component.ends_with(' ')
        {
            bail!("PowerShell gate dependency key is not canonical: {literal}");
        }
        parts.push(component);
    }
    let key = parts.join("/");
    strict_authority_key(&key, true)
}

fn dependency_literals(source: &[u8]) -> Result<Vec<String>> {
    let source = std::str::from_utf8(source)
        .context("PowerShell gate source must be valid UTF-8 for dependency closure")?;
    let mut result = Vec::new();
    for line in source.lines() {
        if line.trim_start().starts_with('#') {
            continue;
        }
        result.extend(
            powershell_quoted_literals(line)
                .into_iter()
                .filter(|literal| literal.to_ascii_lowercase().ends_with(".ps1")),
        );
    }
    Ok(result)
}

/// Return literal repository-local PowerShell dependencies from the immutable
/// decision capture. In current-goal independence mode, an absent literal is
/// retained as an empty dependency slot: baseline/current comparison can then
/// distinguish a fixture that stayed absent from a helper that was added or
/// deleted. Receipt-era binding still omits literals absent from that baseline.
fn trusted_workspace_gate_dependency_keys_with_context(
    decision: &GoalDecisionContext<'_>,
    command: &ParsedValidationCommand,
    receipt_baseline: Option<&WorkspaceBaseline>,
) -> Result<Option<BTreeSet<String>>> {
    let Some(gate) = trusted_gate_script_identity_with_context(decision, command)? else {
        return Ok(None);
    };
    if !gate.kind.workspace_wide() {
        return Ok(None);
    }
    let mut pending = vec![gate.entrypoint];
    let mut dependencies = BTreeSet::new();
    while let Some(key) = pending.pop() {
        if !dependencies.insert(key.clone()) {
            continue;
        }
        let captured = decision.captured_workspace_file(&key)?.ok_or_else(|| {
            anyhow::anyhow!("PowerShell gate dependency is absent from capture: {key}")
        })?;
        if captured.key != key {
            bail!("PowerShell gate dependency key is ambiguous: {key}");
        }
        for literal in dependency_literals(captured.bytes)? {
            let helper = dependency_key(&key, &literal)?;
            match decision.captured_workspace_file(&helper)? {
                Some(file) if file.key == helper => {
                    if receipt_baseline.is_some_and(|baseline| {
                        baseline_hash_for_path(baseline, &helper)
                            .ok()
                            .flatten()
                            .is_none()
                    }) {
                        continue;
                    }
                    pending.push(helper);
                }
                Some(file) => bail!(
                    "PowerShell gate dependency has ambiguous captured identity: {helper} -> {}",
                    file.key
                ),
                None if receipt_baseline.is_some_and(|baseline| {
                    baseline_hash_for_path(baseline, &helper)
                        .ok()
                        .flatten()
                        .is_none()
                }) => {}
                None if receipt_baseline.is_none() => {
                    dependencies.insert(helper);
                }
                None => bail!("PowerShell gate dependency is absent from capture: {helper}"),
            }
        }
    }
    Ok(Some(dependencies))
}

/// Live closure used only by the write/execute path. It shares the same
/// logical-key grammar as capture, then validates every path component.
fn trusted_workspace_gate_dependency_paths(
    root: &Path,
    command: &ParsedValidationCommand,
    receipt_baseline: Option<&WorkspaceBaseline>,
) -> Result<Option<BTreeSet<String>>> {
    let Some(kind) = strict_gate_kind(command)? else {
        return Ok(None);
    };
    if !kind.workspace_wide() {
        return Ok(None);
    }
    let mut pending = vec![kind.key().to_string()];
    let mut dependencies = BTreeSet::new();
    while let Some(key) = pending.pop() {
        if !dependencies.insert(key.clone()) {
            continue;
        }
        let path = root.join(&key);
        crate::context::ensure_source_file(root, &path)?;
        let source = fs::read(&path)?;
        for literal in dependency_literals(&source)? {
            let helper = dependency_key(&key, &literal)?;
            let path = root.join(&helper);
            let missing_at_goal_baseline = receipt_baseline.is_some_and(|baseline| {
                baseline_hash_for_path(baseline, &helper)
                    .ok()
                    .flatten()
                    .is_none()
            });
            match crate::context::ensure_source_file(root, &path) {
                Ok(()) => {
                    if missing_at_goal_baseline {
                        continue;
                    }
                    pending.push(helper);
                }
                Err(error) if error_is_not_found(&error) && missing_at_goal_baseline => {}
                Err(error) if error_is_not_found(&error) && receipt_baseline.is_none() => {
                    // Retain the missing literal as an empty dependency slot.
                    // The caller compares `None` with the goal baseline, so a
                    // fixture absent on both sides is harmless while deletion
                    // of a baseline helper remains a self-validation conflict.
                    dependencies.insert(helper);
                }
                Err(error) => return Err(error),
            }
        }
    }
    Ok(Some(dependencies))
}

fn error_is_not_found(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|io_error| io_error.kind() == std::io::ErrorKind::NotFound)
    })
}

fn authority_gate_binding_sha256(
    policy: &str,
    entrypoint: &str,
    dependency_sha256: &BTreeMap<String, String>,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"rayman.authority-gate-binding.v1");
    hasher.update([0]);
    hasher.update(policy.as_bytes());
    hasher.update([0]);
    hasher.update(entrypoint.as_bytes());
    hasher.update([0]);
    for (path, hash) in dependency_sha256 {
        hasher.update(path.as_bytes());
        hasher.update([0]);
        hasher.update(hash.as_bytes());
        hasher.update([0]);
    }
    format!("{:x}", hasher.finalize())
}

pub(super) fn authority_gate_binding_for_goal_with_context(
    goal: &Goal,
    decision: &GoalDecisionContext<'_>,
    command: &str,
) -> Result<Option<AuthorityGateBinding>> {
    let parsed = parse_validation_command(command)?;
    let baseline = goal.baseline.as_ref().ok_or_else(|| {
        anyhow::anyhow!(
            "goal {} has no start baseline, so authority-gate history cannot be bound",
            goal.id
        )
    })?;
    let Some(dependencies) =
        trusted_workspace_gate_dependency_keys_with_context(decision, &parsed, Some(baseline))?
    else {
        return Ok(None);
    };
    let entrypoint = trusted_gate_script_identity_with_context(decision, &parsed)?
        .ok_or_else(|| anyhow::anyhow!("trusted PowerShell gate identity disappeared"))?
        .entrypoint;
    let mut dependency_sha256 = BTreeMap::new();
    for key in dependencies {
        let hash = baseline_hash_for_path(baseline, &key)?
            .cloned()
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "authority gate dependency {key} was absent from goal {} baseline",
                    goal.id
                )
            })?;
        let captured = decision.captured_workspace_file(&key)?.ok_or_else(|| {
            anyhow::anyhow!("authority gate dependency is missing from capture: {key}")
        })?;
        if captured.key != key || crate::hash::sha256_bytes(captured.bytes) != hash {
            bail!(
                "authority gate dependency {key} differs from goal {} baseline",
                goal.id
            );
        }
        dependency_sha256.insert(key, hash);
    }
    if !dependency_sha256.contains_key(&entrypoint) {
        bail!("authority gate binding does not contain its entrypoint: {entrypoint}");
    }
    let binding_sha256 = authority_gate_binding_sha256(
        AUTHORITY_GATE_BINDING_POLICY_V1,
        &entrypoint,
        &dependency_sha256,
    );
    Ok(Some(AuthorityGateBinding {
        policy: AUTHORITY_GATE_BINDING_POLICY_V1.into(),
        entrypoint,
        dependency_sha256,
        binding_sha256,
    }))
}

pub(super) fn authority_gate_binding_for_goal(
    goal: &Goal,
    root: &Path,
    command: &str,
) -> Result<Option<AuthorityGateBinding>> {
    let parsed = parse_validation_command(command)?;
    let baseline = goal.baseline.as_ref().ok_or_else(|| {
        anyhow::anyhow!(
            "goal {} has no start baseline, so authority-gate history cannot be bound",
            goal.id
        )
    })?;
    let Some(dependencies) =
        trusted_workspace_gate_dependency_paths(root, &parsed, Some(baseline))?
    else {
        return Ok(None);
    };
    let entrypoint = strict_gate_kind(&parsed)?
        .ok_or_else(|| anyhow::anyhow!("trusted PowerShell gate identity disappeared"))?
        .key()
        .to_string();
    let mut dependency_sha256 = BTreeMap::new();
    for key in dependencies {
        let hash = baseline_hash_for_path(baseline, &key)?
            .cloned()
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "authority gate dependency {key} was absent from goal {} baseline",
                    goal.id
                )
            })?;
        let path = root.join(&key);
        crate::context::ensure_source_file(root, &path)?;
        if crate::hash::sha256_file(&path)? != hash {
            bail!(
                "authority gate dependency {key} differs from goal {} baseline",
                goal.id
            );
        }
        dependency_sha256.insert(key, hash);
    }
    if !dependency_sha256.contains_key(&entrypoint) {
        bail!("authority gate binding does not contain its entrypoint: {entrypoint}");
    }
    let binding_sha256 = authority_gate_binding_sha256(
        AUTHORITY_GATE_BINDING_POLICY_V1,
        &entrypoint,
        &dependency_sha256,
    );
    Ok(Some(AuthorityGateBinding {
        policy: AUTHORITY_GATE_BINDING_POLICY_V1.into(),
        entrypoint,
        dependency_sha256,
        binding_sha256,
    }))
}

pub(super) fn authority_gate_binding_error(
    authority: &Goal,
    command: &str,
    binding: Option<&AuthorityGateBinding>,
) -> Option<String> {
    let powershell_gate = parse_validation_command(command)
        .ok()
        .is_some_and(|parsed| powershell_script(&parsed).is_some());
    let Some(binding) = binding else {
        return powershell_gate.then(|| {
            "PowerShell replacement authority 缺少 receipt-era dependency binding".into()
        });
    };
    if !powershell_gate {
        return Some("non-PowerShell authority 不得携带 PowerShell gate binding".into());
    }
    let entrypoint_matches_command = parse_validation_command(command)
        .ok()
        .and_then(|parsed| strict_gate_kind(&parsed).ok().flatten())
        .is_some_and(|kind| binding.entrypoint == kind.key());
    if binding.policy != AUTHORITY_GATE_BINDING_POLICY_V1
        || GateKind::from_exact_key(&binding.entrypoint).is_none()
        || !entrypoint_matches_command
        || binding.dependency_sha256.is_empty()
        || !binding.dependency_sha256.contains_key(&binding.entrypoint)
        || binding.dependency_sha256.iter().any(|(path, hash)| {
            GateKind::from_exact_key(path).is_none() && !ordinary_dependency_key(path)
                || !is_sha256(hash)
        })
        || binding.binding_sha256
            != authority_gate_binding_sha256(
                &binding.policy,
                &binding.entrypoint,
                &binding.dependency_sha256,
            )
    {
        return Some("PowerShell replacement authority dependency binding 无效".into());
    }
    let Some(baseline) = authority.baseline.as_ref() else {
        return Some("PowerShell authority goal 缺少 receipt-era baseline".into());
    };
    if binding.dependency_sha256.iter().any(|(path, hash)| {
        baseline_hash_for_path(baseline, path)
            .ok()
            .flatten()
            .map(String::as_str)
            != Some(hash.as_str())
    }) {
        return Some("PowerShell authority binding 与 authority goal baseline 不一致".into());
    }
    None
}

fn ordinary_dependency_key(key: &str) -> bool {
    strict_authority_key(key, true).is_ok()
}

fn powershell_quoted_literals(line: &str) -> Vec<String> {
    let mut literals = Vec::new();
    let mut characters = line.chars().peekable();
    while let Some(character) = characters.next() {
        if !matches!(character, '\'' | '"') {
            continue;
        }
        let quote = character;
        let mut literal = String::new();
        while let Some(character) = characters.next() {
            if character == quote {
                if quote == '\'' && characters.peek() == Some(&'\'') {
                    literal.push('\'');
                    characters.next();
                    continue;
                }
                break;
            }
            if quote == '"' && character == '`' {
                if let Some(escaped) = characters.next() {
                    literal.push(escaped);
                }
                continue;
            }
            literal.push(character);
        }
        literals.push(literal);
    }
    literals
}

fn baseline_hash_for_path<'a>(
    baseline: &'a WorkspaceBaseline,
    path: &str,
) -> Result<Option<&'a String>> {
    if let Some(hash) = baseline.files.get(path) {
        return Ok(Some(hash));
    }
    if !cfg!(windows) {
        return Ok(None);
    }
    let mut matches = baseline.files.iter().filter(|(candidate, _)| {
        candidate
            .replace('\\', "/")
            .eq_ignore_ascii_case(&path.replace('\\', "/"))
    });
    let Some((_, hash)) = matches.next() else {
        return Ok(None);
    };
    if matches.next().is_some() {
        bail!("baseline path is ambiguous under Windows case rules: {path}");
    }
    Ok(Some(hash))
}

fn authority_command_goal_delta_conflicts_with_context(
    goal: &Goal,
    decision: &GoalDecisionContext<'_>,
    command: &str,
) -> Result<Vec<String>> {
    let parsed = parse_validation_command(command)?;
    let Some(dependencies) =
        trusted_workspace_gate_dependency_keys_with_context(decision, &parsed, None)?
    else {
        return Ok(Vec::new());
    };
    let baseline = goal.baseline.as_ref().ok_or_else(|| {
        anyhow::anyhow!(
            "goal {} has no start baseline, so authority-gate independence cannot be proven",
            goal.id
        )
    })?;
    let mut conflicts = Vec::new();
    for key in dependencies {
        let baseline_hash = baseline_hash_for_path(baseline, &key)?;
        let captured_hash = decision
            .captured_workspace_file(&key)?
            .map(|file| crate::hash::sha256_bytes(file.bytes));
        if baseline_hash.map(String::as_str) != captured_hash.as_deref() {
            conflicts.push(key);
        }
    }
    Ok(conflicts)
}

fn authority_command_goal_delta_conflicts(
    goal: &Goal,
    root: &Path,
    command: &str,
) -> Result<Vec<String>> {
    let parsed = parse_validation_command(command)?;
    let Some(dependencies) = trusted_workspace_gate_dependency_paths(root, &parsed, None)? else {
        return Ok(Vec::new());
    };
    let baseline = goal.baseline.as_ref().ok_or_else(|| {
        anyhow::anyhow!(
            "goal {} has no start baseline, so authority-gate independence cannot be proven",
            goal.id
        )
    })?;
    let mut conflicts = Vec::new();
    for key in dependencies {
        let baseline_hash = baseline_hash_for_path(baseline, &key)?;
        let path = root.join(&key);
        let current_hash = match crate::context::ensure_source_file(root, &path) {
            Ok(()) => Some(crate::hash::sha256_file(&path)?),
            Err(error) if error_is_not_found(&error) => None,
            Err(error) => return Err(error),
        };
        if baseline_hash.map(String::as_str) != current_hash.as_deref() {
            conflicts.push(key);
        }
    }
    Ok(conflicts)
}

pub(super) fn validate_authority_command_for_goal_with_context(
    decision: &GoalDecisionContext<'_>,
    goal: &Goal,
    command: &str,
) -> Result<()> {
    validate_authority_command_with_context(decision, command)?;
    let conflicts = authority_command_goal_delta_conflicts_with_context(goal, decision, command)?;
    if !conflicts.is_empty() {
        bail!(
            "refusing a self-validating authority gate: this goal changed the gate or a proven repository dependency ({})",
            conflicts.join(", ")
        );
    }
    Ok(())
}

/// Preflight a goal-bound authority before the command is executed.
pub fn validate_authority_command_for_goal(root: &Path, goal: &Goal, command: &str) -> Result<()> {
    validate_authority_command(root, command)?;
    let conflicts = authority_command_goal_delta_conflicts(goal, root, command)?;
    if !conflicts.is_empty() {
        bail!(
            "refusing a self-validating authority gate: this goal changed the gate or a proven repository dependency ({}). The command will not execute or be recorded as authority; use an unchanged gate or selector-free workspace Cargo/pytest authority",
            conflicts.join(", ")
        );
    }
    Ok(())
}

#[cfg(test)]
mod launch_path_tests {
    use super::{powershell_launch_argument, powershell_launch_argument_with};
    use std::path::Path;

    #[cfg(windows)]
    #[test]
    fn verbatim_drive_path_launches_only_after_identity_round_trip() {
        let canonical = Path::new(r"\\?\C:\workspace\scripts\check.ps1");
        let launch = powershell_launch_argument_with(canonical, |path| {
            assert_eq!(path, Path::new(r"C:\workspace\scripts\check.ps1"));
            Ok(canonical.to_path_buf())
        })
        .unwrap();
        assert_eq!(launch, r"C:\workspace\scripts\check.ps1");
    }

    #[cfg(windows)]
    #[test]
    fn verbatim_unc_path_launches_only_after_identity_round_trip() {
        let canonical = Path::new(r"\\?\UNC\server\share\scripts\check.ps1");
        let launch = powershell_launch_argument_with(canonical, |path| {
            assert_eq!(path, Path::new(r"\\server\share\scripts\check.ps1"));
            Ok(canonical.to_path_buf())
        })
        .unwrap();
        assert_eq!(launch, r"\\server\share\scripts\check.ps1");
    }

    #[test]
    fn launch_path_identity_mismatch_fails_closed() {
        let directory = tempfile::tempdir().unwrap();
        let canonical = directory.path().canonicalize().unwrap();
        let other = canonical.join("other.ps1");
        let error = powershell_launch_argument_with(&canonical, |_| Ok(other)).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("changed the canonical script identity")
        );
    }

    #[test]
    fn launch_path_canonicalization_failure_fails_closed() {
        let directory = tempfile::tempdir().unwrap();
        let canonical = directory.path().canonicalize().unwrap();
        assert!(
            powershell_launch_argument_with(&canonical, |_| {
                Err(std::io::Error::new(std::io::ErrorKind::NotFound, "missing"))
            })
            .is_err()
        );
    }

    #[test]
    fn real_canonical_path_round_trips_through_the_launch_argument() {
        let directory = tempfile::tempdir().unwrap();
        let script = directory.path().join("check.ps1");
        std::fs::write(&script, "exit 0\n").unwrap();
        let canonical = script.canonicalize().unwrap();
        let launch = powershell_launch_argument(&canonical).unwrap();
        assert_eq!(Path::new(&launch).canonicalize().unwrap(), canonical);
        #[cfg(windows)]
        assert!(!launch.starts_with(r"\\?\"));
    }
}
