[CmdletBinding()]
param()

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

$contract = Read-StrictUtf8 'AGENT_CONTRACT.md'
$agents = Read-StrictUtf8 'AGENTS.md'
$skill = Read-StrictUtf8 'SKILL.md'
$claude = Read-StrictUtf8 'CLAUDE.md'
$workflow = Read-StrictUtf8 'references/workflow-contract.md'
$readme = Read-StrictUtf8 'README.md'
$manifestText = Read-StrictUtf8 'install-manifest.json'

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
if ($contract.Contains('save-work-status:managed-', [StringComparison]::Ordinal) -or
    $contract -match '(?m)(?<![A-Za-z0-9_])[A-Za-z]:[\\/]') {
    throw 'Published AGENT_CONTRACT.md must not contain a managed block or an absolute Windows path.'
}

function Get-ManagedBlock {
    param(
        [Parameter(Mandatory = $true)][string]$Name,
        [Parameter(Mandatory = $true)][string]$Text
    )
    $pattern = '(?ms)^<!-- save-work-status:managed-begin v5 -->\r?\n.*?^<!-- save-work-status:managed-end v5 -->'
    $matches = [regex]::Matches($Text, $pattern)
    if ($matches.Count -ne 1) {
        throw "Client entrypoint must contain exactly one v5 managed block: $Name"
    }
    return $matches[0].Value
}

$codexBlock = Get-ManagedBlock -Name 'AGENTS.md' -Text $agents
$claudeBlock = Get-ManagedBlock -Name 'CLAUDE.md' -Text $claude
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
if (-not $agents.Contains('Codex checkpoint registration', [StringComparison]::Ordinal) -or
    -not $agents.Contains('Claude Code must not execute it', [StringComparison]::Ordinal) -or
    -not $claude.Contains('Claude Code checkpoint registration', [StringComparison]::Ordinal) -or
    -not $claude.Contains('Do not execute the Codex-scoped managed block', [StringComparison]::Ordinal)) {
    throw 'Client entrypoints must state explicit managed-block ownership and exclusion.'
}
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
    -not $readme.Contains('consultation=none|deferred|ready', [StringComparison]::Ordinal)) {
    throw 'Public pending contracts must describe current-event observation and the none/deferred/ready frontier.'
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
if ($manifest.schema_version -ne 1) { throw 'Unsupported install manifest schema.' }
if ($manifest.clients.codex.deployment_scope -ne 'global_skill') {
    throw 'Codex deployment scope must be global_skill.'
}
if ($manifest.clients.claude_code.deployment_scope -ne 'repository_entrypoint_only' -or
    $manifest.clients.claude_code.entrypoint -ne 'CLAUDE.md') {
    throw 'Claude Code must remain a repository-only entrypoint.'
}

$expected = @(
    'AGENT_CONTRACT.md|AGENTS.md'
    'SKILL.md|SKILL.md'
    'references/workflow-contract.md|references/workflow-contract.md'
) | Sort-Object
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
$actual = @($actual | Sort-Object)
if (@(Compare-Object $expected $actual).Count -ne 0) {
    throw "Codex install resources differ from the required manifest set: $($actual -join ', ')"
}
if ($actual -match '^CLAUDE.md\|') {
    throw 'CLAUDE.md must not be advertised as a global installed resource.'
}

Write-Output 'agent-instructions: PASS'
