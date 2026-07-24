[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if ($PSVersionTable.PSVersion.Major -lt 7) {
    throw 'check-agent-instructions.ps1 requires PowerShell 7+. Run it with pwsh, not Windows PowerShell.'
}

$repoRoot = Split-Path -Parent $PSScriptRoot
$utf8 = [System.Text.UTF8Encoding]::new($false, $true)
$requiredFiles = @{
    'AGENTS.md' = @('AGENT_CONTRACT: rayman-shared-v1', 'single source of truth')
    'SKILL.md' = @('AGENT_CONTRACT: rayman-shared-v1', 'AGENTS.md')
    'CLAUDE.md' = @('AGENT_CONTRACT: rayman-shared-v1', 'AGENTS.md')
}

foreach ($relativePath in $requiredFiles.Keys) {
    $path = Join-Path $repoRoot $relativePath
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
        throw "Required agent entry file is missing: $relativePath"
    }

    try {
        $content = $utf8.GetString([IO.File]::ReadAllBytes($path))
    } catch [System.Text.DecoderFallbackException] {
        throw "Agent entry file must be valid UTF-8: $relativePath"
    }

    foreach ($marker in $requiredFiles[$relativePath]) {
        if (-not $content.Contains($marker, [StringComparison]::Ordinal)) {
            throw "Agent entry file is missing required marker '$marker': $relativePath"
        }
    }
}

Write-Output 'agent-instructions: PASS'
