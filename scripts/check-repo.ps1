[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if ($PSVersionTable.PSVersion.Major -lt 7) {
    throw 'check-repo.ps1 requires PowerShell 7+. Run it with pwsh, not Windows PowerShell.'
}

$repoRoot = Split-Path -Parent $PSScriptRoot
Set-Location -LiteralPath $repoRoot
& (Join-Path $PSScriptRoot 'source-bytes.ps1')
& (Join-Path $PSScriptRoot 'source-bytes.ps1') -SelfTest
& (Join-Path $PSScriptRoot 'update-test-traceability.ps1') -SelfTest
$script:RepositoryQualityProviderSha256 = '47f405e725ad272b2d2c0d2b189855375962689f2b356eadc306305f957a0b77'
& (Join-Path $PSScriptRoot 'check-agent-instructions.ps1') -SelfTest
& (Join-Path $PSScriptRoot 'check-ci-workflow.ps1') -SelfTest
& (Join-Path $PSScriptRoot 'check-ci-workflow.ps1')
& (Join-Path $PSScriptRoot 'check-test-traceability.ps1') -SelfTest
& (Join-Path $PSScriptRoot 'check-test-traceability.ps1') -RuntimeInventory
& (Join-Path $PSScriptRoot 'release-closeout.ps1') -SelfTest
# Keep sibling PowerShell regressions explicit and single-owner. The audit
# dependency-policy lane executes its own PowerShell self-test directly.
& (Join-Path $PSScriptRoot 'install-rayman.ps1') -SelfTest
& (Join-Path $PSScriptRoot 'update-rayman.ps1') -SelfTest
& (Join-Path $PSScriptRoot 'check-update-freshness.ps1') -SelfTest
& (Join-Path $PSScriptRoot 'verify-release-contract.ps1') -SelfTest
& (Join-Path $PSScriptRoot 'repair-rayman-powershell-profile.ps1') -SelfTest
& (Join-Path $PSScriptRoot 'configure-codex-validation-temp.ps1') -SelfTest
if ($IsWindows) {
    & (Join-Path $PSScriptRoot 'configure-global-codex-execution.ps1') -SelfTest
    & (Join-Path $PSScriptRoot 'install-global-codex-execution.ps1') -SelfTest
    & (Join-Path $PSScriptRoot 'enroll-global-codex-projects.ps1') -SelfTest
    & (Join-Path $PSScriptRoot 'repair-codex-workspace-acl.ps1') -SelfTest
    & (Join-Path $PSScriptRoot 'check-checkpoint-integration.ps1')
}
& (Join-Path $PSScriptRoot 'codex-powershell-broker.ps1') -SelfTest
& (Join-Path $PSScriptRoot 'install-codex-powershell-broker.ps1') -SelfTest
# Runs the audit script self-test plus the isolated-advisory-DB dependency
# policy checks before the multi-minute fmt/clippy/test stages.
& (Join-Path $PSScriptRoot 'audit-repository.ps1') -DependencyPolicyOnly

function Resolve-NativeApplication {
    param([Parameter(Mandatory = $true)][string]$Name)

    $commands = @(Get-Command -Name $Name -All -ErrorAction SilentlyContinue)
    if ($commands.Count -eq 0 -or $commands[0].CommandType -ne 'Application') {
        throw "$Name must resolve directly to an application on PATH"
    }
    return $commands[0].Source
}

function Invoke-NativeChecked {
    param(
        [Parameter(Mandatory = $true)][string]$Application,
        [Parameter(ValueFromRemainingArguments = $true)][string[]]$Arguments
    )

    & $Application @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "$Application failed with exit code $LASTEXITCODE"
    }
}

function Get-RepositoryQualityCommands {
    param(
        [Parameter(Mandatory = $true)]
        [ValidateSet('Root', 'Evals')]
        [string]$Suite,
        [string]$ProviderPath = (Join-Path $PSScriptRoot 'repository-quality.ps1')
    )

    $usingDefaultProvider = $ProviderPath -ceq (Join-Path $PSScriptRoot 'repository-quality.ps1')
    $reader = Join-Path $PSScriptRoot 'read-repository-quality.ps1'
    $readerHash = (Get-FileHash -LiteralPath $reader -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($readerHash -cne 'd540cb7b63b67f9ed28616ed1f8dd9a52d17c1a8096e7b39cd789dd5525afd4b') {
        throw "Repository quality reader hash drifted: $readerHash"
    }
    $expectedHash = if ($usingDefaultProvider) { $script:RepositoryQualityProviderSha256 } else { '' }
    & $reader -Suite $Suite -ProviderPath $ProviderPath -ExpectedProviderSha256 $expectedHash
}

$cargo = Resolve-NativeApplication -Name 'cargo'
$git = Resolve-NativeApplication -Name 'git'

foreach ($suite in @('Root', 'Evals')) {
    foreach ($qualityCommand in @(Get-RepositoryQualityCommands -Suite $suite)) {
        $phase = [ordered]@{ schema = 'rayman.repository.phase.v1'; suite = $suite; check = $qualityCommand.name; status = 'start'; timestamp_utc = [DateTimeOffset]::UtcNow.ToString('O') }
        Write-Output ('RAYMAN_REPOSITORY_PHASE ' + ($phase | ConvertTo-Json -Compress))
        $timer = [Diagnostics.Stopwatch]::StartNew()
        try {
            Invoke-NativeChecked -Application $cargo -Arguments $qualityCommand.argv
            $phase.status = 'pass'
        } catch {
            $phase.status = 'fail'
            throw
        } finally {
            $phase.timestamp_utc = [DateTimeOffset]::UtcNow.ToString('O')
            $phase['elapsed_ms'] = $timer.ElapsedMilliseconds
            Write-Output ('RAYMAN_REPOSITORY_PHASE ' + ($phase | ConvertTo-Json -Compress))
        }
    }
}
$raymanName = if ($IsWindows) { 'rayman.exe' } else { 'rayman' }
# The dogfood gates below must run the binary this script just built. An ambient
# CARGO_TARGET_DIR redirects cargo's output elsewhere, which left a stale
# target/debug binary silently running the gates.
# `Join-Path` does not let an absolute second segment win, so an absolute
# CARGO_TARGET_DIR (the usual way the variable is set, for a shared cache) built
# `<repoRoot>\C:\cache\...` and the script threw at the very end of a multi-minute
# run. `[IO.Path]::Combine` has the resolve-against-base semantics this needs.
$targetRoot = if ($env:CARGO_TARGET_DIR) {
    [IO.Path]::GetFullPath([IO.Path]::Combine($repoRoot, $env:CARGO_TARGET_DIR))
} else {
    Join-Path $repoRoot 'target'
}
$rayman = Join-Path $targetRoot "debug/$raymanName"
if (-not (Test-Path -LiteralPath $rayman -PathType Leaf)) {
    throw "cargo test did not produce the expected debug rayman application: $rayman"
}
Invoke-NativeChecked -Application $rayman -Arguments @('context', 'refresh')
Invoke-NativeChecked -Application $rayman -Arguments @(
    'map', 'quality', '--profile', 'strict', '--check'
)
Invoke-NativeChecked -Application $git -Arguments @('diff', '--check')

Write-Output 'check-repo: PASS'
