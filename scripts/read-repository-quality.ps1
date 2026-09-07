[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidateSet('Root', 'Evals')]
    [string]$Suite,
    [string]$ProviderPath = (Join-Path $PSScriptRoot 'repository-quality.ps1'),
    [string]$ExpectedProviderSha256
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$helper = $ProviderPath
if (-not (Test-Path -LiteralPath $helper -PathType Leaf)) {
    throw "Repository quality command provider is missing: $helper"
}
if (-not [string]::IsNullOrWhiteSpace($ExpectedProviderSha256)) {
    $actualProviderHash = (Get-FileHash -LiteralPath $helper -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($actualProviderHash -cne $ExpectedProviderSha256) {
        throw "Repository quality command provider hash drifted: $actualProviderHash"
    }
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
