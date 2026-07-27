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

$agents = Read-StrictUtf8 'AGENTS.md'
$skill = Read-StrictUtf8 'SKILL.md'
$claude = Read-StrictUtf8 'CLAUDE.md'
$workflow = Read-StrictUtf8 'references/workflow-contract.md'
$manifestText = Read-StrictUtf8 'install-manifest.json'

$marker = 'AGENT_CONTRACT: rayman-shared-v1'
foreach ($entry in @{ 'AGENTS.md' = $agents; 'SKILL.md' = $skill; 'CLAUDE.md' = $claude }.GetEnumerator()) {
    $count = ([regex]::Matches($entry.Value, [regex]::Escape($marker))).Count
    if ($count -ne 1) {
        throw "Agent contract marker must appear exactly once: $($entry.Key)"
    }
}

foreach ($entry in @{ 'SKILL.md' = $skill; 'CLAUDE.md' = $claude }.GetEnumerator()) {
    foreach ($required in @('AGENTS.md', 'references/workflow-contract.md')) {
        if (-not $entry.Value.Contains($required, [StringComparison]::Ordinal)) {
            throw "Client adapter is missing '$required': $($entry.Key)"
        }
    }
    foreach ($forbidden in @('## Shared working rules', '## Shared workflow authority')) {
        if ($entry.Value.Contains($forbidden, [StringComparison]::Ordinal)) {
            throw "Client adapter duplicates shared policy '$forbidden': $($entry.Key)"
        }
    }
}

if (($skill -split "`n").Count -gt 100 -or ($claude -split "`n").Count -gt 60) {
    throw 'Client adapters are too large; move client-neutral workflow detail to the shared reference.'
}
if (-not $agents.Contains('references/workflow-contract.md', [StringComparison]::Ordinal)) {
    throw 'AGENTS.md must route non-trivial workflow claims to the shared reference.'
}
if (-not $agents.Contains('Do not permit direct or indirect skill-invocation cycles.', [StringComparison]::Ordinal)) {
    throw 'AGENTS.md must prohibit direct and indirect skill-invocation cycles.'
}
foreach ($required in @('--must-proof KIND::TEXT', 'goal handoff start', 'Unbound `rayman check`', 'checkpoint save')) {
    if (-not $workflow.Contains($required, [StringComparison]::OrdinalIgnoreCase)) {
        throw "Shared workflow reference is missing required contract text: $required"
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
if ($manifest.schema_version -ne 1) { throw 'Unsupported install manifest schema.' }
if ($manifest.clients.codex.deployment_scope -ne 'global_skill') {
    throw 'Codex deployment scope must be global_skill.'
}
if ($manifest.clients.claude_code.deployment_scope -ne 'repository_entrypoint_only' -or
    $manifest.clients.claude_code.entrypoint -ne 'CLAUDE.md') {
    throw 'Claude Code must remain a repository-only entrypoint.'
}

$expected = @(
    'AGENTS.md|AGENTS.md'
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
