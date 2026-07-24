[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if ($PSVersionTable.PSVersion.Major -lt 7) {
    throw 'check-repo.ps1 requires PowerShell 7+. Run it with pwsh, not Windows PowerShell.'
}

$repoRoot = Split-Path -Parent $PSScriptRoot
Set-Location -LiteralPath $repoRoot

& (Join-Path $PSScriptRoot 'check-agent-instructions.ps1')
& (Join-Path $PSScriptRoot 'release-closeout.ps1') -SelfTest
& (Join-Path $PSScriptRoot 'audit-repository.ps1') -SelfTest

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
Invoke-NativeChecked -Application $cargo -Arguments @('deny', 'check', '--config', 'deny.toml')
Invoke-NativeChecked -Application $cargo -Arguments @(
    'deny', '--manifest-path', 'evals/Cargo.toml', 'check', '--config', 'evals/deny.toml'
)
$raymanName = if ($IsWindows) { 'rayman.exe' } else { 'rayman' }
$rayman = Join-Path $repoRoot "target/debug/$raymanName"
if (-not (Test-Path -LiteralPath $rayman -PathType Leaf)) {
    throw "cargo test did not produce the expected debug rayman application: $rayman"
}
Invoke-NativeChecked -Application $rayman -Arguments @('context', 'refresh')
Invoke-NativeChecked -Application $rayman -Arguments @(
    'map', 'quality', '--profile', 'strict', '--check'
)
Invoke-NativeChecked -Application $git -Arguments @('diff', '--check')

Write-Output 'check-repo: PASS'
