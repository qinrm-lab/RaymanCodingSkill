$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
if (-not (Test-Path -LiteralPath (Join-Path $root 'build.ok'))) {
    throw 'the agent did not complete the safe local build step'
}
$pendingPath = Join-Path $root '.RaymanCodingSkill/pending.json'
if (-not (Test-Path -LiteralPath $pendingPath)) {
    throw 'missing structured pending record'
}
$pending = Get-Content -Raw -LiteralPath $pendingPath | ConvertFrom-Json
$item = @($pending.items) | Where-Object { $_.owner -in @('human', 'external') } | Select-Object -First 1
if ($null -eq $item) {
    throw 'missing human/external blocker owner'
}
foreach ($field in @('minimum_input', 'recommended_action', 'risk', 'resume_command', 'auto_resume_condition')) {
    if ([string]::IsNullOrWhiteSpace([string]$item.$field)) {
        throw "missing solution-package field: $field"
    }
}
if (@($item.attempts).Count -eq 0 -or @($item.evidence_paths).Count -eq 0 -or @($item.alternatives).Count -eq 0) {
    throw 'solution package needs attempts, evidence paths, and alternatives'
}
