[CmdletBinding()]
param(
    [switch]$SelfTest
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if ($PSVersionTable.PSVersion.Major -lt 7) {
    throw 'check-agent-instructions.ps1 requires PowerShell 7+. Run it with pwsh, not Windows PowerShell.'
}

$repoRoot = Split-Path -Parent $PSScriptRoot
$utf8 = [System.Text.UTF8Encoding]::new($false, $true)

function Read-StrictUtf8 {
    param([Parameter(Mandatory = $true)][string]$RelativePath)
    $path = Join-Path $repoRoot $RelativePath
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
        throw "Required agent resource is missing: $RelativePath"
    }
    try {
        return $utf8.GetString([IO.File]::ReadAllBytes($path))
    } catch [System.Text.DecoderFallbackException] {
        throw "Agent resource must be valid UTF-8: $RelativePath"
    }
}

function Assert-PublishedContractSafe {
    param([Parameter(Mandatory = $true)][string]$Text)
    if ($Text.Contains('save-work-status:managed-', [StringComparison]::Ordinal) -or
        $Text -match '(?m)(?<![A-Za-z0-9_])[A-Za-z]:[\\/]') {
        throw 'Published AGENT_CONTRACT.md must not contain a managed block or an absolute Windows path.'
    }
}

function Assert-Throws {
    param(
        [Parameter(Mandatory = $true)][string]$Label,
        [Parameter(Mandatory = $true)][scriptblock]$Action
    )
    $threw = $false
    try {
        & $Action
    } catch {
        $threw = $true
    }
    if (-not $threw) {
        throw "Agent-instruction self-test did not fail closed: $Label"
    }
}

$contract = Read-StrictUtf8 'AGENT_CONTRACT.md'
$agents = Read-StrictUtf8 'AGENTS.md'
$skill = Read-StrictUtf8 'SKILL.md'
$claude = Read-StrictUtf8 'CLAUDE.md'
$workflow = Read-StrictUtf8 'references/workflow-contract.md'
$readme = Read-StrictUtf8 'README.md'
$manifestText = Read-StrictUtf8 'install-manifest.json'
$pendingSource = Read-StrictUtf8 'crates/rayman/src/goal/pending.rs'
$goalCliSource = Read-StrictUtf8 'crates/rayman/src/goal_cli.rs'
$codexHookSource = Read-StrictUtf8 'crates/rayman/src/codex_hook.rs'
$auditDocumentation = Read-StrictUtf8 'docs/AUDIT.md'
$auditSource = Read-StrictUtf8 'scripts/audit-repository.ps1'
$workspaceManifest = Read-StrictUtf8 'Cargo.toml'
$ciWorkflow = Read-StrictUtf8 '.github/workflows/ci.yml'
$releaseVerifier = Read-StrictUtf8 'scripts/verify-release-contract.ps1'

function Get-SingleContractValue {
    param(
        [Parameter(Mandatory = $true)][string]$Label,
        [Parameter(Mandatory = $true)][string]$Text,
        [Parameter(Mandatory = $true)][string]$Pattern
    )
    $found = [regex]::Matches(
        $Text,
        $Pattern,
        [System.Text.RegularExpressions.RegexOptions]::Multiline
    )
    if ($found.Count -ne 1 -or [string]::IsNullOrWhiteSpace($found[0].Groups[1].Value)) {
        throw "$Label must have exactly one non-empty contract value."
    }
    return $found[0].Groups[1].Value
}

function Assert-MsrvContractConsistency {
    param(
        [Parameter(Mandatory = $true)][string]$WorkspaceManifest,
        [Parameter(Mandatory = $true)][string]$Readme,
        [Parameter(Mandatory = $true)][string]$AuditDocumentation,
        [Parameter(Mandatory = $true)][string]$AuditSource,
        [Parameter(Mandatory = $true)][string]$CiWorkflow,
        [Parameter(Mandatory = $true)][string]$ReleaseVerifier
    )
    $declaredMsrv = Get-SingleContractValue `
        -Label 'Cargo.toml rust-version' `
        -Text $WorkspaceManifest `
        -Pattern '^rust-version\s*=\s*"([^"]+)"\s*$'
    $releaseMsrv = Get-SingleContractValue `
        -Label 'release verifier required MSRV' `
        -Text $ReleaseVerifier `
        -Pattern '^\$requiredMsrv\s*=\s*''([^'']+)''\s*$'
    if ($releaseMsrv -cne $declaredMsrv) {
        throw "Release verifier MSRV $releaseMsrv differs from Cargo.toml $declaredMsrv."
    }

    $coverageJobs = [regex]::Matches(
        $CiWorkflow,
        '(?ms)^  coverage:\r?\n(?<body>.*?)(?=^  [A-Za-z0-9_-]+:\r?\n|\z)'
    )
    if ($coverageJobs.Count -ne 1) {
        throw 'CI workflow must define exactly one coverage job.'
    }
    $coverageMsrv = Get-SingleContractValue `
        -Label 'coverage job toolchain' `
        -Text $coverageJobs[0].Groups['body'].Value `
        -Pattern '^\s+toolchain:\s*([^\s#]+)\s*(?:#.*)?$'
    if ($coverageMsrv -cne $declaredMsrv) {
        throw "Coverage MSRV $coverageMsrv differs from Cargo.toml $declaredMsrv."
    }

    foreach ($required in @(
            "minimum supported Rust version is **$declaredMsrv**",
            ('rust-version = "{0}"' -f $declaredMsrv)
        )) {
        if (-not $Readme.Contains($required, [StringComparison]::Ordinal)) {
            throw "README MSRV contract is missing: $required"
        }
    }
    foreach ($required in @(
            "[ValidateSet('$declaredMsrv')]",
            ('$MsrvToolchain = ''{0}''' -f $declaredMsrv)
        )) {
        if (-not $AuditSource.Contains($required, [StringComparison]::Ordinal)) {
            throw "Audit source MSRV contract is missing: $required"
        }
    }
    foreach ($required in @(
            ('MSRV and the coverage tool are fixed at {0}{1}{0}' -f [char]96, $declaredMsrv),
            "rustup which ... --toolchain $declaredMsrv"
        )) {
        if (-not $AuditDocumentation.Contains($required, [StringComparison]::Ordinal)) {
            throw "Audit documentation MSRV contract is missing: $required"
        }
    }
    return $declaredMsrv
}

function Assert-AuditAuthorizationDocumentation {
    param(
        [Parameter(Mandatory = $true)][string]$Documentation,
        [Parameter(Mandatory = $true)][string]$Source
    )
    foreach ($required in @(
            'an audit request by itself does not grant write authorization',
            'When the user explicitly asks to repair or close audit findings',
            'The default audit never provisions host audit tools or changes rustup components',
            '`-PrepareAuditTools` is the sole provisioning authorization'
        )) {
        if (-not $Documentation.Contains($required, [StringComparison]::Ordinal)) {
            throw "Audit documentation is missing the explicit write-authority boundary: $required"
        }
    }
    foreach ($required in @(
            '[switch]$PrepareAuditTools',
            "[Parameter(Mandatory = `$true, ParameterSetName = 'PrepareAuditTools')]",
            'switch ($PSCmdlet.ParameterSetName)',
            'if (-not $PrepareAuditTools.IsPresent)',
            "if (`$PSCmdlet.ParameterSetName -eq 'PrepareAuditTools')",
            "Join-Path `$PSScriptRoot 'repository-quality.ps1'",
            "schema -cne 'rayman.repository-quality.commands.v1'",
            "schema = 'rayman.audit.tool-preparation.v1'",
            'Resolve-PersistentCargoInstallRoot',
            'Get-MsrvLlvmPreparationArguments',
            'Get-CoverageToolPreparationArguments',
            "-IncludePreinstalledCoverageTool (`$PSCmdlet.ParameterSetName -eq 'Audit')"
        )) {
        if (-not $Source.Contains($required, [StringComparison]::Ordinal)) {
            throw "Audit source is missing its explicit tool-preparation authority boundary: $required"
        }
    }
    foreach ($name in @('CliPath', 'SkillPath')) {
        $escapedVariable = [regex]::Escape('$' + $name)
        $pattern = "(?ms)(?:\[Parameter\([^\]]*ParameterSetName\s*=\s*'PrepareAuditTools'[^\]]*\)\]\s*)+(?:\[[^\]]+\]\s*)*\[string\]\s*$escapedVariable\b"
        if ([regex]::IsMatch($Source, $pattern)) {
            throw "Audit preparation mode must not require the $name release identity."
        }
    }
    $prepareBranch = $Source.IndexOf("if (`$PSCmdlet.ParameterSetName -eq 'PrepareAuditTools')", [StringComparison]::Ordinal)
    $helperLoad = $Source.IndexOf("function Get-RepositoryQualityCommands", [StringComparison]::Ordinal)
    if ($prepareBranch -lt 0 -or $helperLoad -lt 0 -or $prepareBranch -ge $helperLoad) {
        throw 'Audit preparation must branch before loading the repository quality helper.'
    }
    $preparation = $Source.Substring($prepareBranch, $helperLoad - $prepareBranch)
    if (-not $preparation.Contains('return', [StringComparison]::Ordinal)) {
        throw 'Audit preparation must return before loading repository audit helpers.'
    }
}

$declaredMsrv = Assert-MsrvContractConsistency `
    -WorkspaceManifest $workspaceManifest `
    -Readme $readme `
    -AuditDocumentation $auditDocumentation `
    -AuditSource $auditSource `
    -CiWorkflow $ciWorkflow `
    -ReleaseVerifier $releaseVerifier
Assert-AuditAuthorizationDocumentation `
    -Documentation $auditDocumentation `
    -Source $auditSource

$canonicalSkillAsset = 'crates/rayman/assets/canonical-skill.md'
$null = Read-StrictUtf8 $canonicalSkillAsset
$skillBytes = [IO.File]::ReadAllBytes((Join-Path $repoRoot 'SKILL.md'))
$canonicalSkillBytes = [IO.File]::ReadAllBytes((Join-Path $repoRoot $canonicalSkillAsset))
if ($skillBytes.Length -ne $canonicalSkillBytes.Length -or
    [Convert]::ToBase64String($skillBytes) -cne [Convert]::ToBase64String($canonicalSkillBytes)) {
    throw "Packaged canonical skill bytes differ from repository SKILL.md: $canonicalSkillAsset"
}

$marker = 'AGENT_CONTRACT: rayman-shared-v1'
foreach ($entry in @{
        'AGENT_CONTRACT.md' = $contract
        'AGENTS.md' = $agents
        'SKILL.md' = $skill
        'CLAUDE.md' = $claude
    }.GetEnumerator()) {
    $count = ([regex]::Matches($entry.Value, [regex]::Escape($marker))).Count
    if ($count -ne 1) {
        throw "Agent contract marker must appear exactly once: $($entry.Key)"
    }
}

$adapterRoutes = @{
    'AGENTS.md' = @{ Text = $agents; Required = @('AGENT_CONTRACT.md', 'SKILL.md', 'references/workflow-contract.md') }
    'SKILL.md' = @{ Text = $skill; Required = @('AGENTS.md', 'references/workflow-contract.md') }
    'CLAUDE.md' = @{ Text = $claude; Required = @('AGENT_CONTRACT.md', 'references/workflow-contract.md') }
}
foreach ($entry in $adapterRoutes.GetEnumerator()) {
    foreach ($required in $entry.Value.Required) {
        if (-not $entry.Value.Text.Contains($required, [StringComparison]::Ordinal)) {
            throw "Client adapter is missing '$required': $($entry.Key)"
        }
    }
    foreach ($forbidden in @('## Shared working rules', '## Shared workflow authority')) {
        if ($entry.Value.Text.Contains($forbidden, [StringComparison]::Ordinal)) {
            throw "Client adapter duplicates shared policy '$forbidden': $($entry.Key)"
        }
    }
}

if (($skill -split "`n").Count -gt 100 -or ($claude -split "`n").Count -gt 60) {
    throw 'Client adapters are too large; move client-neutral workflow detail to the shared reference.'
}
if (-not $contract.Contains('references/workflow-contract.md', [StringComparison]::Ordinal)) {
    throw 'AGENT_CONTRACT.md must route non-trivial workflow claims to the shared reference.'
}
if (-not $contract.Contains('Do not permit direct or indirect skill-invocation cycles.', [StringComparison]::Ordinal)) {
    throw 'AGENT_CONTRACT.md must prohibit direct and indirect skill-invocation cycles.'
}
Assert-PublishedContractSafe -Text $contract

function Assert-MsrvAuditDocumentation {
    param(
        [Parameter(Mandatory = $true)][string]$Documentation,
        [Parameter(Mandatory = $true)][string]$Source,
        [Parameter(Mandatory = $true)][string]$Msrv
    )
    foreach ($required in @(
            "rustup which ... --toolchain $Msrv",
            'CARGO_BUILD_RUSTC',
            'CARGO_TARGET_DIR',
            '.RaymanCodingSkill/tmp',
            'ProviderPath',
            'CargoDenyDatabaseSeedPath'
        )) {
        if (-not $Documentation.Contains($required, [StringComparison]::Ordinal)) {
            throw "MSRV audit documentation is missing required contract text: $required"
        }
    }
    foreach ($required in @(
            "@('which', `$Name, '--toolchain', `$Toolchain)",
            'Invoke-IsolatedMsrvChecks',
            "'CARGO_BUILD_RUSTC'",
            "New-ManagedAuditDirectory -Label 'msrv-target'",
            '.ProviderPath',
            '$CargoDenyDatabaseSeedPath',
            '-DatabaseSeedPath $CargoDenyDatabaseSeedPath'
        )) {
        if (-not $Source.Contains($required, [StringComparison]::Ordinal)) {
            throw "MSRV audit source is missing its documented implementation token: $required"
        }
    }
}

Assert-MsrvAuditDocumentation `
    -Documentation $auditDocumentation `
    -Source $auditSource `
    -Msrv $declaredMsrv

function Assert-FileSystemProviderPaths {
    param(
        [Parameter(Mandatory = $true)][string]$RelativePath,
        [Parameter(Mandatory = $true)][string]$Text
    )
    if ($Text -match '(?m)Resolve-Path[^\r\n]*\)\.Path(?![A-Za-z])') {
        throw "$RelativePath uses Resolve-Path.Path for a filesystem identity; use ProviderPath so extended Windows paths remain filesystem paths."
    }
}

foreach ($relativePath in @(
        'scripts/audit-repository.ps1',
        'scripts/install-rayman.ps1',
        'scripts/release-closeout.ps1',
        'scripts/repair-rayman-powershell-profile.ps1',
        'scripts/update-rayman.ps1',
        'scripts/verify-release-contract.ps1'
    )) {
    Assert-FileSystemProviderPaths `
        -RelativePath $relativePath `
        -Text (Read-StrictUtf8 $relativePath)
}

function Get-ManagedBlock {
    param(
        [Parameter(Mandatory = $true)][string]$Name,
        [Parameter(Mandatory = $true)][string]$Text
    )
    $pattern = '(?ms)^<!-- save-work-status:managed-begin v6 -->\r?\n.*?^<!-- save-work-status:managed-end v6 -->'
    $matches = [regex]::Matches($Text, $pattern)
    if ($matches.Count -ne 1) {
        throw "Client entrypoint must contain exactly one v6 managed block: $Name"
    }
    return $matches[0].Value
}

function Assert-ManagedBlockIsolation {
    param(
        [Parameter(Mandatory = $true)][string]$CodexText,
        [Parameter(Mandatory = $true)][string]$ClaudeText
    )
    $codexBlock = Get-ManagedBlock -Name 'AGENTS.md' -Text $CodexText
    $claudeBlock = Get-ManagedBlock -Name 'CLAUDE.md' -Text $ClaudeText
    $legacyRuntimePattern = '(?i)(?:\bpython(?:\.exe)?\b|\bpy(?:\.exe)?\b|workspace_activation\.py|status_checkpoint\.py)'
    $rustRuntimePattern = '(?i)save-work-status(?:-[0-9a-f]{8,64})?\.exe'
    foreach ($entry in @(
            @{ Name = 'AGENTS.md'; Block = $codexBlock },
            @{ Name = 'CLAUDE.md'; Block = $claudeBlock }
        )) {
        if ($entry.Block -match $legacyRuntimePattern) {
            throw "v6 managed block must not invoke the Python compatibility bridge: $($entry.Name)"
        }
        if ($entry.Block -notmatch $rustRuntimePattern) {
            throw "v6 managed block must invoke the compiled save-work-status runtime: $($entry.Name)"
        }
    }
    $normalizeAgent = {
        param([string]$Text)
        return [regex]::Replace($Text, '--agent\s+(?:codex|claude-code)', '--agent <client>')
    }
    if ((& $normalizeAgent $codexBlock) -cne (& $normalizeAgent $claudeBlock)) {
        throw 'Codex and Claude Code managed blocks may differ only in the --agent value.'
    }
    if ($codexBlock -notmatch '--agent\s+codex(?:\s|$)' -or
        $codexBlock -match '--agent\s+claude-code(?:\s|$)' -or
        $claudeBlock -notmatch '--agent\s+claude-code(?:\s|$)' -or
        $claudeBlock -match '--agent\s+codex(?:\s|$)') {
        throw 'Managed block ownership does not match its client entrypoint.'
    }
    if (-not $CodexText.Contains('Codex checkpoint registration', [StringComparison]::Ordinal) -or
        -not $CodexText.Contains('Claude Code must not execute it', [StringComparison]::Ordinal) -or
        -not $ClaudeText.Contains('Claude Code checkpoint registration', [StringComparison]::Ordinal) -or
        -not $ClaudeText.Contains('Do not execute the Codex-scoped managed block', [StringComparison]::Ordinal)) {
        throw 'Client entrypoints must state explicit managed-block ownership and exclusion.'
    }
}

Assert-ManagedBlockIsolation -CodexText $agents -ClaudeText $claude
foreach ($required in @('--must-proof KIND::TEXT', 'goal handoff start', 'Unbound `rayman check`', 'checkpoint save')) {
    if (-not $workflow.Contains($required, [StringComparison]::OrdinalIgnoreCase)) {
        throw "Shared workflow reference is missing required contract text: $required"
    }
}

# Keep the public human-boundary contract aligned with the event-local Stop
# implementation. Byte equality alone can make SKILL.md and its packaged copy
# consistently wrong, while README drift would otherwise remain invisible.
$publicPendingContracts = @{
    'SKILL.md' = $skill
    'references/workflow-contract.md' = $workflow
    'README.md' = $readme
}
foreach ($entry in $publicPendingContracts.GetEnumerator()) {
    foreach ($forbidden in @('goal pending present', 'consultation=presented', 'decision=await_user')) {
        if ($entry.Value.Contains($forbidden, [StringComparison]::OrdinalIgnoreCase)) {
            throw "Public pending contract contains retired durable-presentation semantics '$forbidden': $($entry.Key)"
        }
    }
    if ($entry.Value -notmatch 'goal\s+pending\s+render\s+--current') {
        throw "Public pending contract is missing deterministic workspace aggregate rendering: $($entry.Key)"
    }
}
if (-not $skill.Contains('current Codex Stop event', [StringComparison]::Ordinal) -or
    -not $workflow.Contains('current Codex Stop event', [StringComparison]::Ordinal) -or
    $workflow -notmatch 'Claude\s+Code must not execute or emulate that Codex hook' -or
    -not $readme.Contains('consultation=none|deferred|ready', [StringComparison]::Ordinal) -or
    -not $readme.Contains('rayman.human-boundary-aggregate.v1', [StringComparison]::Ordinal)) {
    throw 'Public pending contracts must describe current-event observation and the none/deferred/ready frontier.'
}
if (-not $pendingSource.Contains('rayman.human-boundary-aggregate.v1', [StringComparison]::Ordinal) -or
    -not $pendingSource.Contains('current_response_only', [StringComparison]::Ordinal)) {
    throw 'Shared pending implementation is missing the client-neutral aggregate schema and scope.'
}
foreach ($entry in @{
        'crates/rayman/src/goal/pending.rs' = $pendingSource
        'crates/rayman/src/goal_cli.rs' = $goalCliSource
    }.GetEnumerator()) {
    foreach ($forbidden in @('rayman.codex-stop-candidate', 'current_stop_event_only', 'last_assistant_message', 'current Codex Stop event')) {
        if ($entry.Value.Contains($forbidden, [StringComparison]::Ordinal)) {
            throw "Client-neutral pending implementation contains Codex-specific semantics '$forbidden': $($entry.Key)"
        }
    }
}
foreach ($required in @('last_assistant_message', 'normalize_human_boundary_message', 'codex_stop_candidate_observed')) {
    if (-not $codexHookSource.Contains($required, [StringComparison]::Ordinal)) {
        throw "Codex Stop adapter is missing its strict native observation contract: $required"
    }
}

$skillLines = $skill -split "`n"
$frontmatter = @($skillLines)[1..2]
# Enforce the whole block, not just the two keys: line 0 must open the frontmatter and
# line 3 must close it, so extra keys after description cannot slip through while the
# error still claims the block holds only name and description.
if ($skillLines[0].TrimEnd("`r") -ne '---' -or
    $frontmatter.Count -ne 2 -or
    -not $frontmatter[0].StartsWith('name: ') -or
    -not $frontmatter[1].StartsWith('description: ') -or
    $skillLines[3].TrimEnd("`r") -ne '---') {
    throw 'SKILL.md frontmatter must contain only name and description.'
}

try {
    $manifest = $manifestText | ConvertFrom-Json -ErrorAction Stop
} catch {
    throw "install-manifest.json is invalid JSON: $($_.Exception.Message)"
}
if ($manifest.schema_version -ne 2) { throw 'Unsupported install manifest schema.' }
if ($manifest.clients.codex.deployment_scope -ne 'global_skill') {
    throw 'Codex deployment scope must be global_skill.'
}
if ($manifest.clients.claude_code.deployment_scope -ne 'repository_entrypoint_only' -or
    $manifest.clients.claude_code.entrypoint -ne 'CLAUDE.md') {
    throw 'Claude Code must remain a repository-only entrypoint.'
}
$updateRuntimeProperties = @($manifest.update_runtime.PSObject.Properties.Name | Sort-Object)
if (($updateRuntimeProperties -join ',') -ne 'manifest_asset,protocol,receipt_relative_to_user_data,signature_asset,worker_artifact_base,worker_destination_pattern' -or
    $manifest.update_runtime.protocol -ne 'rayman.update.manifest.v1' -or
    $manifest.update_runtime.manifest_asset -ne 'rayman-update-manifest-v1.json' -or
    $manifest.update_runtime.signature_asset -ne 'rayman-update-manifest-v1.sig' -or
    $manifest.update_runtime.worker_artifact_base -ne 'rayman-update-worker' -or
    $manifest.update_runtime.worker_destination_pattern -ne 'rayman-update-worker-{version}{exe_suffix}' -or
    $manifest.update_runtime.receipt_relative_to_user_data -ne 'Rayman/install/receipt.json') {
    throw 'Install manifest trusted update runtime contract is invalid.'
}

function Assert-ManifestResourceSet {
    param([Parameter(Mandatory = $true)][string[]]$Actual)
    $expected = @(
        'AGENT_CONTRACT.md|AGENTS.md'
        'SKILL.md|SKILL.md'
        'references/workflow-contract.md|references/workflow-contract.md'
    ) | Sort-Object
    $Actual = @($Actual | Sort-Object)
    if (@(Compare-Object $expected $Actual).Count -ne 0) {
        throw "Codex install resources differ from the required manifest set: $($Actual -join ', ')"
    }
    if ($Actual -match '^CLAUDE.md\|') {
        throw 'CLAUDE.md must not be advertised as a global installed resource.'
    }
}

$actual = @()
foreach ($resource in @($manifest.codex_skill_resources)) {
    $properties = @($resource.PSObject.Properties.Name | Sort-Object)
    if (($properties -join ',') -ne 'destination,source') {
        throw 'Install manifest resource has unknown or missing fields.'
    }
    $source = [string]$resource.source
    $destination = [string]$resource.destination
    if ([IO.Path]::IsPathRooted($source) -or [IO.Path]::IsPathRooted($destination) -or
        $source.Contains('..') -or $destination.Contains('..') -or
        $source.Contains('\') -or $destination.Contains('\')) {
        throw "Install manifest path must be an ordinary forward-slash relative path: $source -> $destination"
    }
    $null = Read-StrictUtf8 $source
    $actual += "$source|$destination"
}
Assert-ManifestResourceSet -Actual $actual

if ($SelfTest) {
    Assert-Throws -Label 'Cargo manifest MSRV drifts from release and audit contracts' -Action {
        Assert-MsrvContractConsistency `
            -WorkspaceManifest $workspaceManifest.Replace(
                ('rust-version = "{0}"' -f $declaredMsrv),
                'rust-version = "0.0.0"'
            ) `
            -Readme $readme `
            -AuditDocumentation $auditDocumentation `
            -AuditSource $auditSource `
            -CiWorkflow $ciWorkflow `
            -ReleaseVerifier $releaseVerifier
    }
    Assert-Throws -Label 'coverage job loses exact MSRV binding' -Action {
        Assert-MsrvContractConsistency `
            -WorkspaceManifest $workspaceManifest `
            -Readme $readme `
            -AuditDocumentation $auditDocumentation `
            -AuditSource $auditSource `
            -CiWorkflow ($ciWorkflow -replace '(?ms)(^  coverage:.*?^\s+toolchain:\s*)[^\s#]+', '${1}stable') `
            -ReleaseVerifier $releaseVerifier
    }
    Assert-Throws -Label 'audit documentation implies write authority' -Action {
        Assert-AuditAuthorizationDocumentation `
            -Documentation $auditDocumentation.Replace(
                'an audit request by itself does not grant write authorization',
                'an audit request may imply write authorization'
            ) `
            -Source $auditSource
    }
    Assert-Throws -Label 'published managed block' -Action {
        Assert-PublishedContractSafe -Text ($contract + "`n<!-- save-work-status:managed-begin v6 -->")
    }
    Assert-Throws -Label 'published absolute checkout path' -Action {
        Assert-PublishedContractSafe -Text ($contract + "`nC:\private\checkout")
    }
    Assert-Throws -Label 'Claude block running as Codex' -Action {
        $wrongClaude = $claude.Replace('--agent claude-code', '--agent codex')
        Assert-ManagedBlockIsolation -CodexText $agents -ClaudeText $wrongClaude
    }
    Assert-Throws -Label 'legacy v5 managed block' -Action {
        $legacyCodex = $agents.Replace('managed-begin v6', 'managed-begin v5').Replace('managed-end v6', 'managed-end v5')
        Assert-ManagedBlockIsolation -CodexText $legacyCodex -ClaudeText $claude
    }
    Assert-Throws -Label 'v6 managed block invokes Python bridge' -Action {
        $pythonCodex = $agents.Replace('save-work-status.exe', 'workspace_activation.py')
        $pythonClaude = $claude.Replace('save-work-status.exe', 'workspace_activation.py')
        Assert-ManagedBlockIsolation -CodexText $pythonCodex -ClaudeText $pythonClaude
    }
    Assert-Throws -Label 'missing client ownership exclusion' -Action {
        $ambiguousCodex = $agents.Replace('Claude Code must not execute it', 'Another client may execute it')
        Assert-ManagedBlockIsolation -CodexText $ambiguousCodex -ClaudeText $claude
    }
    Assert-Throws -Label 'repository AGENTS deployed globally' -Action {
        Assert-ManifestResourceSet -Actual @(
            'AGENTS.md|AGENTS.md'
            'SKILL.md|SKILL.md'
            'references/workflow-contract.md|references/workflow-contract.md'
        )
    }
    Assert-Throws -Label 'Claude entrypoint deployed globally' -Action {
        Assert-ManifestResourceSet -Actual @(
            'AGENT_CONTRACT.md|AGENTS.md'
            'CLAUDE.md|CLAUDE.md'
            'SKILL.md|SKILL.md'
            'references/workflow-contract.md|references/workflow-contract.md'
        )
    }
    Assert-Throws -Label 'MSRV documentation loses compiler binding' -Action {
        Assert-MsrvAuditDocumentation `
            -Documentation $auditDocumentation.Replace('CARGO_BUILD_RUSTC', 'REMOVED_COMPILER_BINDING') `
            -Source $auditSource `
            -Msrv $declaredMsrv
    }
    Assert-Throws -Label 'audit documentation loses explicit advisory seed contract' -Action {
        Assert-MsrvAuditDocumentation `
            -Documentation $auditDocumentation.Replace('CargoDenyDatabaseSeedPath', 'REMOVED_ADVISORY_SEED') `
            -Source $auditSource `
            -Msrv $declaredMsrv
    }
    Assert-Throws -Label 'audit source loses explicit advisory seed propagation' -Action {
        Assert-MsrvAuditDocumentation `
            -Documentation $auditDocumentation `
            -Source $auditSource.Replace('-DatabaseSeedPath $CargoDenyDatabaseSeedPath', '-DatabaseSeedPath $null') `
            -Msrv $declaredMsrv
    }
    Assert-Throws -Label 'filesystem identity regresses to provider-qualified Path' -Action {
        Assert-FileSystemProviderPaths `
            -RelativePath 'scripts/example.ps1' `
            -Text '$resolved = (Resolve-Path -LiteralPath $path).Path'
    }
    Write-Output 'agent-instructions self-test: PASS'
}

Write-Output 'agent-instructions: PASS'
