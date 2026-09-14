[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if (-not $IsWindows) { throw 'Checkpoint integration acceptance requires native Windows.' }
$repo = Split-Path -Parent $PSScriptRoot
$pin = Get-Content -LiteralPath (Join-Path $repo 'governance/checkpoint-integration.json') -Raw | ConvertFrom-Json
if ($pin.schema -cne 'rayman.checkpoint-integration.v1' -or
    $pin.repository -cne 'qinrm-lab/RaymanSaveStatusSkill' -or
    $pin.commit -cnotmatch '^[0-9a-f]{40}$') { throw 'Invalid checkpoint integration source pin.' }
$source = $env:RAYMAN_TEST_SAVE_SOURCE
if ([string]::IsNullOrWhiteSpace($source) -or -not [IO.Path]::IsPathFullyQualified($source)) {
    throw 'Set RAYMAN_TEST_SAVE_SOURCE to the clean checkout at governance/checkpoint-integration.json commit.'
}
$source = [IO.Path]::GetFullPath($source)

function Invoke-Checked([string]$Program, [string[]]$Arguments) {
    $output = & $Program @Arguments
    if ($LASTEXITCODE -ne 0) { throw "$Program failed with exit code $LASTEXITCODE" }
    return $output
}

function Assert-CandidateSource {
    $head = @(Invoke-Checked git @('-C', $source, 'rev-parse', 'HEAD')) -join "`n"
    $dirty = @(Invoke-Checked git @('-C', $source, 'status', '--porcelain=v1', '--untracked-files=all'))
    if ($head -cne $pin.commit -or $dirty.Count -ne 0) {
        throw 'Checkpoint integration requires the exact clean pinned SaveStatus commit.'
    }
}

function Build-Candidate([string]$Root, [string]$Package, [string]$Target) {
    $records = @(Invoke-Checked cargo @('build', '--locked', '--manifest-path',
        (Join-Path $Root 'Cargo.toml'), '-p', $Package, '--bins', '--target-dir', $Target,
        '--message-format=json'))
    $artifacts = @{}
    foreach ($line in $records) {
        $item = $line | ConvertFrom-Json
        if ($item.reason -ceq 'compiler-artifact' -and $null -ne $item.executable) {
            $artifacts[$item.target.name] = [string]$item.executable
        }
    }
    return $artifacts
}

function Invoke-CheckpointIntegration {
    Assert-CandidateSource
    # Both candidates come from this operation's Cargo artifact reports, never
    # an assumed target/debug path or an arbitrary prebuilt SaveStatus binary.
    $target = if ($env:CARGO_TARGET_DIR) {
        [IO.Path]::GetFullPath([IO.Path]::Combine($repo, $env:CARGO_TARGET_DIR))
    } else { Join-Path $repo 'target' }
    $save = Build-Candidate $source 'save-work-status-runtime' (Join-Path $target 'checkpoint-save')
    $global = Build-Candidate $repo 'rayman-global-execution' $target
    Assert-CandidateSource
    $runtime = $save['save-work-status']
    if ([string]::IsNullOrWhiteSpace($runtime)) { throw 'Cargo did not report the SaveStatus executable.' }
    $before = (Get-FileHash -LiteralPath $runtime -Algorithm SHA256).Hash.ToLowerInvariant()
    $result = @(Invoke-Checked python @('-I', (Join-Path $PSScriptRoot 'test-global-checkpoint-adapter.py'),
        '--save-runtime', $runtime, '--worker', $global['rayman-global-worker'],
        '--client', $global['rayman-global'])) -join "`n"
    $acceptance = $result | ConvertFrom-Json
    if ($acceptance.ok -ne $true -or $acceptance.production_installed -ne $false -or
        $acceptance.publication_and_rollback_recovery -ne $true -or
        $acceptance.failed_persistence_withheld_success -ne $true) {
        throw 'Checkpoint integration did not return complete acceptance evidence.'
    }
    Assert-CandidateSource
    if ((Get-FileHash -LiteralPath $runtime -Algorithm SHA256).Hash.ToLowerInvariant() -cne $before) {
        throw 'SaveStatus candidate changed during acceptance.'
    }
    [ordered]@{ schema = 'rayman.checkpoint-integration.result.v1'; status = 'pass';
        save_source_commit = $pin.commit; save_runtime_sha256 = $before; acceptance = $acceptance
    } | ConvertTo-Json -Depth 5
}

Invoke-CheckpointIntegration
