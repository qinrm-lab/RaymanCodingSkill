[CmdletBinding(DefaultParameterSetName = 'Check')]
param(
    [Parameter(Mandatory = $true, ParameterSetName = 'Check')]
    [string]$ManifestPath,
    [Parameter(Mandatory = $true, ParameterSetName = 'Check')]
    [string]$SignaturePath,
    [Parameter(Mandatory = $true, ParameterSetName = 'Check')]
    [string]$WorkerPath,
    [Parameter(ParameterSetName = 'Check')]
    [string]$ExpectedVersion,
    [Parameter(ParameterSetName = 'Check')]
    [ValidateRange(1, 30)]
    [int]$MinimumRemainingDays = 14,
    [Parameter(Mandatory = $true, ParameterSetName = 'SelfTest')]
    [switch]$SelfTest
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if ($PSVersionTable.PSVersion.Major -lt 7) {
    throw 'check-update-freshness.ps1 requires PowerShell 7+. Run it with pwsh, not Windows PowerShell.'
}

function Resolve-OrdinaryFile {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$Label
    )

    $item = Get-Item -LiteralPath $Path -Force -ErrorAction Stop
    $linkType = [string]$item.LinkType
    if ($item.PSIsContainer -or
        ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -or
        (-not [string]::IsNullOrWhiteSpace($linkType) -and $linkType -cne 'HardLink')) {
        throw "$Label must be an ordinary non-reparse file: $Path"
    }
    return $item.FullName
}

function Get-Sha256 {
    param([Parameter(Mandatory = $true)][string]$Path)

    return (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}

function Resolve-NativeApplication {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$Label
    )

    $resolved = Resolve-OrdinaryFile -Path $Path -Label $Label
    if ($IsWindows -and [IO.Path]::GetExtension($resolved) -cne '.exe') {
        throw "$Label must be a native .exe application on Windows: $resolved"
    }
    $commands = @(Get-Command -Name $resolved -All -CommandType Application -ErrorAction SilentlyContinue)
    if ($commands.Count -ne 1) {
        throw "$Label must resolve as exactly one native application: $resolved"
    }
    $comparison = if ($IsWindows) {
        [StringComparison]::OrdinalIgnoreCase
    } else {
        [StringComparison]::Ordinal
    }
    if (-not ([IO.Path]::GetFullPath($commands[0].Source)).Equals(
            [IO.Path]::GetFullPath($resolved),
            $comparison
        )) {
        throw "$Label command identity differs from the requested file: $resolved"
    }
    return $resolved
}

function Convert-ManifestTimestamp {
    param(
        [Parameter(Mandatory = $true)]$Value,
        [Parameter(Mandatory = $true)][string]$Field
    )

    if ($Value -isnot [string] -or [string]::IsNullOrWhiteSpace($Value)) {
        throw "Verified update manifest is missing string field '$Field'."
    }
    [DateTimeOffset]$parsed = [DateTimeOffset]::MinValue
    if (-not [DateTimeOffset]::TryParseExact(
            $Value,
            'yyyy-MM-ddTHH:mm:ssZ',
            [Globalization.CultureInfo]::InvariantCulture,
            [Globalization.DateTimeStyles]::AssumeUniversal,
            [ref]$parsed
        )) {
        throw "Verified update manifest field '$Field' is not canonical UTC: $Value"
    }
    return $parsed.ToUniversalTime()
}

function Assert-ManifestFreshness {
    param(
        [Parameter(Mandatory = $true)][pscustomobject]$Manifest,
        [Parameter(Mandatory = $true)][DateTimeOffset]$Now,
        [Parameter(Mandatory = $true)][int]$MinimumDays
    )

    if ($Manifest.version -isnot [string] -or
        [string]::IsNullOrWhiteSpace($Manifest.version) -or
        $Manifest.release_tag -isnot [string] -or
        [string]::IsNullOrWhiteSpace($Manifest.release_tag)) {
        throw 'Verified update manifest is missing version or release_tag.'
    }
    $issuedAt = Convert-ManifestTimestamp -Value $Manifest.issued_at -Field 'issued_at'
    $expiresAt = Convert-ManifestTimestamp -Value $Manifest.expires_at -Field 'expires_at'
    if ($expiresAt -le $issuedAt) {
        throw 'Verified update manifest expires_at must be later than issued_at.'
    }
    $deadline = $Now.ToUniversalTime().AddDays($MinimumDays)
    if ($expiresAt -lt $deadline) {
        $remaining = $expiresAt - $Now.ToUniversalTime()
        throw "Signed update metadata for $($Manifest.release_tag) has only $([Math]::Round($remaining.TotalDays, 2)) days remaining; at least $MinimumDays days are required. Publish a new patch release from a clean HEAD through the protected rayman-release environment before $($expiresAt.ToString('O')). Do not replace the existing release assets or weaken manifest expiry."
    }
    return [pscustomobject]@{
        issued_at = $issuedAt
        expires_at = $expiresAt
        remaining_days = ($expiresAt - $Now.ToUniversalTime()).TotalDays
    }
}

function Invoke-SelfTest {
    $now = [DateTimeOffset]::Parse('2026-01-01T00:00:00Z')
    $fresh = [pscustomobject]@{
        version = '9.8.7'
        release_tag = 'v9.8.7'
        issued_at = '2026-01-01T00:00:00Z'
        expires_at = '2026-01-15T00:00:00Z'
    }
    $result = Assert-ManifestFreshness -Manifest $fresh -Now $now -MinimumDays 14
    if ([Math]::Round($result.remaining_days, 0) -ne 14) {
        throw 'Freshness self-test did not preserve the exact boundary.'
    }

    foreach ($case in @(
        [pscustomobject]@{ Name = 'below threshold'; Manifest = [pscustomobject]@{ version = '9.8.7'; release_tag = 'v9.8.7'; issued_at = '2026-01-01T00:00:00Z'; expires_at = '2026-01-14T23:59:59Z' } },
        [pscustomobject]@{ Name = 'expired'; Manifest = [pscustomobject]@{ version = '9.8.7'; release_tag = 'v9.8.7'; issued_at = '2025-12-01T00:00:00Z'; expires_at = '2025-12-31T00:00:00Z' } },
        [pscustomobject]@{ Name = 'reversed interval'; Manifest = [pscustomobject]@{ version = '9.8.7'; release_tag = 'v9.8.7'; issued_at = '2026-01-02T00:00:00Z'; expires_at = '2026-01-01T00:00:00Z' } },
        [pscustomobject]@{ Name = 'noncanonical time'; Manifest = [pscustomobject]@{ version = '9.8.7'; release_tag = 'v9.8.7'; issued_at = '2026-01-01T00:00:00+00:00'; expires_at = '2026-01-15T00:00:00Z' } },
        [pscustomobject]@{ Name = 'missing identity'; Manifest = [pscustomobject]@{ version = ''; release_tag = ''; issued_at = '2026-01-01T00:00:00Z'; expires_at = '2026-01-15T00:00:00Z' } }
    )) {
        $rejected = $false
        try {
            $null = Assert-ManifestFreshness -Manifest $case.Manifest -Now $now -MinimumDays 14
        } catch {
            $rejected = $true
        }
        if (-not $rejected) {
            throw "Freshness self-test accepted $($case.Name)."
        }
    }
    Write-Output 'check-update-freshness self-test: PASS'
}

if ($SelfTest) {
    Invoke-SelfTest
    return
}

$manifest = Resolve-OrdinaryFile -Path $ManifestPath -Label 'Update manifest'
$signature = Resolve-OrdinaryFile -Path $SignaturePath -Label 'Update signature'
$worker = Resolve-NativeApplication -Path $WorkerPath -Label 'Update worker'
$initial = [ordered]@{
    manifest = Get-Sha256 $manifest
    signature = Get-Sha256 $signature
    worker = Get-Sha256 $worker
}

$previousNativePreference = $PSNativeCommandUseErrorActionPreference
$PSNativeCommandUseErrorActionPreference = $false
try {
    $workerArguments = @('verify-manifest', '--manifest', $manifest, '--signature', $signature)
    if (-not [string]::IsNullOrWhiteSpace($ExpectedVersion)) {
        $workerArguments += @('--expected-version', $ExpectedVersion)
    }
    $workerOutput = & $worker @workerArguments 2>&1
    $workerExit = $LASTEXITCODE
} finally {
    $PSNativeCommandUseErrorActionPreference = $previousNativePreference
}
if ($workerExit -ne 0) {
    throw "Update worker rejected the signed manifest with exit code ${workerExit}: $($workerOutput | Out-String)"
}
$workerText = ($workerOutput | Out-String).Trim()
try {
    $verified = $workerText | ConvertFrom-Json -Depth 8 -ErrorAction Stop
} catch {
    throw "Update worker returned invalid verification JSON: $workerText"
}
if ($verified -is [array] -or
    $verified -isnot [pscustomobject] -or
    $verified.status -cne 'manifest_verified' -or
    $verified.manifest_sha256 -cne $initial.manifest -or
    (-not [string]::IsNullOrWhiteSpace($ExpectedVersion) -and
        $verified.version -cne $ExpectedVersion)) {
    throw 'Update worker verification report does not bind the exact manifest bytes.'
}

$terminal = [ordered]@{
    manifest = Get-Sha256 $manifest
    signature = Get-Sha256 $signature
    worker = Get-Sha256 $worker
}
foreach ($name in $initial.Keys) {
    if ($initial[$name] -cne $terminal[$name]) {
        throw "Update freshness input '$name' changed during signature verification."
    }
}

try {
    $convertArguments = @{
        Depth = 12
        ErrorAction = 'Stop'
    }
    # PowerShell 7.6 defaults to parsing ISO timestamps into DateTime values;
    # older supported PowerShell 7 releases do not expose -DateKind. Preserve
    # the signed JSON scalar spelling when that compatibility switch exists.
    if ((Get-Command ConvertFrom-Json).Parameters.ContainsKey('DateKind')) {
        $convertArguments.DateKind = 'String'
    }
    $document = Get-Content -Raw -LiteralPath $manifest -Encoding utf8 |
        ConvertFrom-Json @convertArguments
} catch {
    throw "Verified update manifest cannot be parsed for freshness: $($_.Exception.Message)"
}
if ($document -is [array] -or
    $document -isnot [pscustomobject] -or
    $document.version -cne $verified.version -or
    [uint64]$document.sequence -ne [uint64]$verified.sequence -or
    $document.key_id -cne $verified.key_id -or
    $document.release_tag -cne $verified.release_tag -or
    $document.commit_sha -cne $verified.commit_sha) {
    throw 'Freshness metadata does not match the signed worker verification report.'
}
$freshness = Assert-ManifestFreshness `
    -Manifest $document `
    -Now ([DateTimeOffset]::UtcNow) `
    -MinimumDays $MinimumRemainingDays

$final = [ordered]@{
    manifest = Get-Sha256 $manifest
    signature = Get-Sha256 $signature
    worker = Get-Sha256 $worker
}
foreach ($name in $initial.Keys) {
    if ($initial[$name] -cne $final[$name]) {
        throw "Update freshness input '$name' changed after manifest inspection."
    }
}

[ordered]@{
    status = 'fresh'
    version = [string]$document.version
    release_tag = [string]$document.release_tag
    sequence = [uint64]$document.sequence
    key_id = [string]$document.key_id
    commit_sha = [string]$document.commit_sha
    issued_at = $freshness.issued_at.ToString('O')
    expires_at = $freshness.expires_at.ToString('O')
    remaining_days = [Math]::Round($freshness.remaining_days, 2)
    minimum_remaining_days = $MinimumRemainingDays
    manifest_sha256 = $initial.manifest
    signature_sha256 = $initial.signature
    worker_sha256 = $initial.worker
} | ConvertTo-Json -Depth 4
