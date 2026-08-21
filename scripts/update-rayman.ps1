[CmdletBinding(DefaultParameterSetName = 'Create')]
param(
    [Parameter(ParameterSetName = 'Create', Mandatory = $true)]
    [string]$AssetRoot,

    [Parameter(ParameterSetName = 'Create', Mandatory = $true)]
    [string]$InstallManifest,

    [Parameter(ParameterSetName = 'Create', Mandatory = $true)]
    [string]$Commit,

    [Parameter(ParameterSetName = 'Create', Mandatory = $true)]
    [UInt64]$Sequence,

    [Parameter(ParameterSetName = 'Create', Mandatory = $true)]
    [string]$IssuedAt,

    [Parameter(ParameterSetName = 'Create', Mandatory = $true)]
    [string]$ExpiresAt,

    [Parameter(ParameterSetName = 'Create', Mandatory = $true)]
    [string]$OutputDirectory,

    [Parameter(ParameterSetName = 'Create')]
    [string]$WorkerPath,

    [Parameter(ParameterSetName = 'Create')]
    [string]$SigningKeyPath,

    [Parameter(ParameterSetName = 'SelfTest', Mandatory = $true)]
    [switch]$SelfTest
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if ($PSVersionTable.PSVersion.Major -lt 7) {
    throw 'update-rayman.ps1 requires PowerShell 7+.'
}

function Resolve-OrdinaryFile {
    param([Parameter(Mandatory = $true)][string]$Path, [string]$Label)

    $resolved = (Resolve-Path -LiteralPath $Path -ErrorAction Stop).ProviderPath
    $item = Get-Item -LiteralPath $resolved -Force
    if ($item.PSIsContainer -or
        $item.Attributes -band [IO.FileAttributes]::ReparsePoint) {
        throw "$Label must be an ordinary non-reparse file: $resolved"
    }
    return $resolved
}

function Resolve-OrdinaryDirectory {
    param([Parameter(Mandatory = $true)][string]$Path, [string]$Label)

    $resolved = (Resolve-Path -LiteralPath $Path -ErrorAction Stop).ProviderPath
    $item = Get-Item -LiteralPath $resolved -Force
    if (-not $item.PSIsContainer -or
        $item.Attributes -band [IO.FileAttributes]::ReparsePoint) {
        throw "$Label must be an ordinary non-reparse directory: $resolved"
    }
    return $resolved
}

function Invoke-CheckedApplication {
    param(
        [Parameter(Mandatory = $true)][string]$FilePath,
        [string[]]$Arguments
    )

    & $FilePath @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "$FilePath $($Arguments -join ' ') failed with exit code $LASTEXITCODE."
    }
}

if ($SelfTest) {
    $source = Get-Content -LiteralPath $PSCommandPath -Raw -Encoding utf8
    foreach ($required in @(
        'rayman-update-manifest-v1.json',
        'rayman-update-manifest-v1.payload',
        'rayman-update-manifest-v1.sig',
        'pkeyutl',
        'create-manifest',
        'verify-manifest'
    )) {
        if (-not $source.Contains($required, [StringComparison]::Ordinal)) {
            throw "Release helper lost required fixed contract token: $required"
        }
    }
    Write-Host 'Update release helper self-test passed: fixed manifest/payload/signature names and signing command are present.'
    return
}

$assetRoot = Resolve-OrdinaryDirectory -Path $AssetRoot -Label 'Release asset root'
$installManifest = Resolve-OrdinaryFile -Path $InstallManifest -Label 'Install manifest'
$outputDirectory = Resolve-OrdinaryDirectory -Path $OutputDirectory -Label 'Release output directory'
if ([string]::IsNullOrWhiteSpace($WorkerPath)) {
    $workerName = if ($IsWindows) { 'rayman-update-worker.exe' } else { 'rayman-update-worker' }
    $WorkerPath = Join-Path $PSScriptRoot "../target/release/$workerName"
}
$worker = Resolve-OrdinaryFile -Path $WorkerPath -Label 'Update worker manifest generator'
if ($Commit -notmatch '^[0-9a-f]{40}$' -or $Sequence -eq 0) {
    throw 'Release commit must be lowercase 40-hex and sequence must be positive.'
}
$null = [DateTimeOffset]::ParseExact($IssuedAt, 'yyyy-MM-ddTHH:mm:ssZ', [Globalization.CultureInfo]::InvariantCulture)
$null = [DateTimeOffset]::ParseExact($ExpiresAt, 'yyyy-MM-ddTHH:mm:ssZ', [Globalization.CultureInfo]::InvariantCulture)

$manifest = Join-Path $outputDirectory 'rayman-update-manifest-v1.json'
$payload = Join-Path $outputDirectory 'rayman-update-manifest-v1.payload'
$signature = Join-Path $outputDirectory 'rayman-update-manifest-v1.sig'
foreach ($path in @($manifest, $payload, $signature)) {
    if (Test-Path -LiteralPath $path) {
        throw "Refusing to overwrite release output: $path"
    }
}

Invoke-CheckedApplication -FilePath $worker -Arguments @(
    'create-manifest',
    '--asset-root', $assetRoot,
    '--install-manifest', $installManifest,
    '--commit', $Commit,
    '--sequence', [string]$Sequence,
    '--issued-at', $IssuedAt,
    '--expires-at', $ExpiresAt,
    '--output', $manifest,
    '--signing-payload', $payload
)

if (-not [string]::IsNullOrWhiteSpace($SigningKeyPath)) {
    $signingKey = Resolve-OrdinaryFile -Path $SigningKeyPath -Label 'Ed25519 signing key'
    $opensslCommands = @(Get-Command openssl -All -ErrorAction Stop)
    if ($opensslCommands.Count -eq 0 -or $opensslCommands[0].CommandType -ne 'Application') {
        throw 'openssl must resolve directly to an Application for release signing.'
    }
    $openssl = Resolve-OrdinaryFile -Path $opensslCommands[0].Source -Label 'OpenSSL signer'
    $temporaryPublic = Join-Path $outputDirectory ('.rayman-update-public-' + [Guid]::NewGuid().ToString('N') + '.pem')
    try {
        Invoke-CheckedApplication -FilePath $openssl -Arguments @(
            'pkeyutl', '-sign', '-rawin', '-inkey', $signingKey,
            '-in', $payload, '-out', $signature
        )
        Invoke-CheckedApplication -FilePath $openssl -Arguments @(
            'pkey', '-in', $signingKey, '-pubout', '-out', $temporaryPublic
        )
        Invoke-CheckedApplication -FilePath $openssl -Arguments @(
            'pkeyutl', '-verify', '-rawin', '-pubin', '-inkey', $temporaryPublic,
            '-in', $payload, '-sigfile', $signature
        )
        if ((Get-Item -LiteralPath $signature).Length -ne 64) {
            throw 'Ed25519 detached signature is not exactly 64 bytes.'
        }
        Invoke-CheckedApplication -FilePath $worker -Arguments @(
            'verify-manifest',
            '--manifest', $manifest,
            '--signature', $signature
        )
    } finally {
        if (Test-Path -LiteralPath $temporaryPublic -PathType Leaf) {
            Remove-Item -LiteralPath $temporaryPublic -Force
        }
    }
}

[ordered]@{
    status = $(if (Test-Path -LiteralPath $signature -PathType Leaf) { 'signed' } else { 'manifest_only' })
    manifest = $manifest
    signing_payload = $payload
    signature = $(if (Test-Path -LiteralPath $signature -PathType Leaf) { $signature } else { $null })
    manifest_sha256 = (Get-FileHash -LiteralPath $manifest -Algorithm SHA256).Hash.ToLowerInvariant()
    payload_sha256 = (Get-FileHash -LiteralPath $payload -Algorithm SHA256).Hash.ToLowerInvariant()
} | ConvertTo-Json -Depth 4
