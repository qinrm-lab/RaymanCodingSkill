[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if ($PSVersionTable.PSVersion.Major -lt 7) {
    throw 'check-repo.ps1 requires PowerShell 7+. Run it with pwsh, not Windows PowerShell.'
}

$repoRoot = Split-Path -Parent $PSScriptRoot
Set-Location -LiteralPath $repoRoot
$auditIntegrationTestName = 'audit_self_test_exercises_only_the_audit_contract'

& (Join-Path $PSScriptRoot 'check-agent-instructions.ps1') -SelfTest
& (Join-Path $PSScriptRoot 'release-closeout.ps1') -SelfTest
# Keep sibling PowerShell regressions explicit and single-owner. The audit
# self-test now exercises only audit-repository.ps1, so root Cargo tests do not
# recursively launch the installer/verifier/profile suites.
& (Join-Path $PSScriptRoot 'install-rayman.ps1') -SelfTest
& (Join-Path $PSScriptRoot 'update-rayman.ps1') -SelfTest
& (Join-Path $PSScriptRoot 'verify-release-contract.ps1') -SelfTest
& (Join-Path $PSScriptRoot 'repair-rayman-powershell-profile.ps1') -SelfTest
& (Join-Path $PSScriptRoot 'configure-codex-validation-temp.ps1') -SelfTest
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

    $helper = $ProviderPath
    if (-not (Test-Path -LiteralPath $helper -PathType Leaf)) {
        throw "Repository quality command provider is missing: $helper"
    }
    $json = & $helper -Suite $Suite | Out-String
    if (-not $? -or [string]::IsNullOrWhiteSpace($json)) {
        throw "Repository quality command provider failed for suite $Suite"
    }
    try {
        $document = $json | ConvertFrom-Json -Depth 8 -NoEnumerate -ErrorAction Stop
    } catch {
        throw "Repository quality command provider returned invalid JSON for suite ${Suite}: $($_.Exception.Message)"
    }
    $expectedNames = @('fmt', 'clippy', 'test')
    if ($document -is [array] -or
        $document -isnot [pscustomobject] -or
        $document.schema -isnot [string] -or
        $document.suite -isnot [string] -or
        $document.commands -isnot [array]) {
        throw "Repository quality command provider returned invalid JSON types for suite $Suite"
    }
    $commands = $document.commands
    if ($document.schema -cne 'rayman.repository-quality.commands.v1' -or
        $document.suite -cne $Suite -or
        $commands.Count -ne $expectedNames.Count) {
        throw "Repository quality command provider contract mismatch for suite $Suite"
    }
    for ($index = 0; $index -lt $commands.Count; $index++) {
        $command = $commands[$index]
        if ($command -is [array] -or
            $command -isnot [pscustomobject] -or
            $command.name -isnot [string] -or
            $command.argv -isnot [array]) {
            throw "Repository quality command provider returned invalid command types at index $index for suite $Suite"
        }
        $argv = $command.argv
        if ($command.name -cne $expectedNames[$index] -or
            $argv.Count -eq 0 -or
            @($argv | Where-Object { $_ -isnot [string] -or [string]::IsNullOrWhiteSpace($_) }).Count -ne 0) {
            throw "Repository quality command provider returned an invalid command at index $index for suite $Suite"
        }
    }
    return $commands
}

$cargo = Resolve-NativeApplication -Name 'cargo'
$git = Resolve-NativeApplication -Name 'git'

foreach ($suite in @('Root', 'Evals')) {
    foreach ($qualityCommand in @(Get-RepositoryQualityCommands -Suite $suite)) {
        $qualityArguments = @($qualityCommand.argv)
        if ($suite -eq 'Root' -and $qualityCommand.name -eq 'test') {
            # DependencyPolicyOnly already executed the real audit PowerShell
            # self-test above. Skip exactly that one integration test here so
            # the selector-free outer Cargo suite does not recursively launch it.
            $qualityArguments += @('--', '--skip', $auditIntegrationTestName)
        }
        Invoke-NativeChecked -Application $cargo -Arguments $qualityArguments
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
