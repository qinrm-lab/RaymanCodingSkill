[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if ($PSVersionTable.PSVersion.Major -lt 7) {
    throw 'check-repo.ps1 requires PowerShell 7+. Run it with pwsh, not Windows PowerShell.'
}

$repoRoot = Split-Path -Parent $PSScriptRoot
Set-Location -LiteralPath $repoRoot

& (Join-Path $PSScriptRoot 'check-agent-instructions.ps1') -SelfTest
& (Join-Path $PSScriptRoot 'release-closeout.ps1') -SelfTest
# Runs the audit script self-test, then the isolated-advisory-DB dependency
# policy checks. Doing this before the multi-minute fmt/clippy/test stages
# fails fast under restricted sandboxes and never locks the user-profile
# advisory database.
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

$cargo = Resolve-NativeApplication -Name 'cargo'
$git = Resolve-NativeApplication -Name 'git'

Invoke-NativeChecked -Application $cargo -Arguments @('fmt', '--all', '--check')
Invoke-NativeChecked -Application $cargo -Arguments @(
    'clippy', '--locked', '--workspace', '--all-targets', '--all-features', '--', '-D', 'warnings'
)
Invoke-NativeChecked -Application $cargo -Arguments @(
    'test', '--locked', '--workspace', '--all-targets'
)
Invoke-NativeChecked -Application $cargo -Arguments @(
    'fmt', '--manifest-path', 'evals/Cargo.toml', '--all', '--check'
)
Invoke-NativeChecked -Application $cargo -Arguments @(
    'clippy', '--manifest-path', 'evals/Cargo.toml', '--locked', '--all-targets', '--all-features', '--', '-D', 'warnings'
)
Invoke-NativeChecked -Application $cargo -Arguments @(
    'test', '--manifest-path', 'evals/Cargo.toml', '--locked', '--all-targets'
)
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
