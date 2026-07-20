[CmdletBinding(DefaultParameterSetName = 'Audit')]
param(
    [Parameter(Mandatory = $true, ParameterSetName = 'Audit')]
    [ValidateNotNullOrEmpty()]
    [string]$CliPath,

    [Parameter(Mandatory = $true, ParameterSetName = 'Audit')]
    [ValidateNotNullOrEmpty()]
    [string]$SkillPath,

    [Parameter(ParameterSetName = 'Audit')]
    [ValidateSet('1.88.0')]
    [string]$MsrvToolchain = '1.88.0',

    [Parameter(ParameterSetName = 'Audit')]
    [ValidateRange(75, 100)]
    [int]$MinimumCliLineCoverage = 75,

    [Parameter(ParameterSetName = 'Audit')]
    [ValidateSet('0.8.7')]
    [string]$CoverageToolVersion = '0.8.7',
    [Parameter(Mandatory = $true, ParameterSetName = 'DependencyPolicy')]
    [switch]$DependencyPolicyOnly,

    [Parameter(Mandatory = $true, ParameterSetName = 'SelfTest')]
    [switch]$SelfTest
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if ($PSVersionTable.PSVersion.Major -lt 7) {
    throw 'audit-repository.ps1 requires PowerShell 7+. Run it with pwsh, not Windows PowerShell.'
}

$repoRoot = Split-Path -Parent $PSScriptRoot
$artifactName = if ($IsWindows) { 'rayman.exe' } else { 'rayman' }
$pathComparison = if ($IsWindows) {
    [StringComparison]::OrdinalIgnoreCase
} else {
    [StringComparison]::Ordinal
}

function Get-FileSha256 {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Path
    )

    return (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}

function Resolve-NativeApplication {
    param(
        [Parameter(Mandatory = $true)]
        [ValidateNotNullOrEmpty()]
        [string]$Name,

        [ValidateNotNullOrEmpty()]
        [string]$Label = $Name
    )

    $commands = @(Get-Command -Name $Name -All -ErrorAction SilentlyContinue)
    if ($commands.Count -eq 0) {
        throw "$Label application is missing from PATH: $Name"
    }
    $effective = $commands[0]
    if ($effective.CommandType -ne 'Application') {
        throw "$Label command '$Name' is shadowed by $($effective.CommandType); only an Application is accepted."
    }
    if ([string]::IsNullOrWhiteSpace($effective.Source) -or
        -not (Test-Path -LiteralPath $effective.Source -PathType Leaf)) {
        throw "$Label did not resolve to an existing application file: $($effective.Source)"
    }
    $resolved = (Resolve-Path -LiteralPath $effective.Source).Path
    return [pscustomobject]@{
        Name = $Name
        Label = $Label
        Path = $resolved
        Sha256 = Get-FileSha256 -Path $resolved
    }
}

function Assert-ExpectedApplicationPath {
    param(
        [Parameter(Mandatory = $true)]
        $Identity,

        [Parameter(Mandatory = $true)]
        [string]$ExpectedPath
    )

    if (-not (Test-Path -LiteralPath $ExpectedPath -PathType Leaf)) {
        throw "$($Identity.Label) expected application is missing: $ExpectedPath"
    }
    $resolvedExpected = (Resolve-Path -LiteralPath $ExpectedPath).Path
    if (-not $Identity.Path.Equals($resolvedExpected, $pathComparison)) {
        throw "$($Identity.Label) resolved outside its exact managed path: $($Identity.Path) != $resolvedExpected"
    }
}

function Assert-NativeApplicationIdentity {
    param(
        [Parameter(Mandatory = $true)]
        $Identity
    )

    $current = Resolve-NativeApplication -Name $Identity.Name -Label $Identity.Label
    if (-not $current.Path.Equals($Identity.Path, $pathComparison) -or
        $current.Sha256 -ne $Identity.Sha256) {
        throw "$($Identity.Label) application identity changed during audit: $($Identity.Path) [$($Identity.Sha256)] -> $($current.Path) [$($current.Sha256)]"
    }
}

function Assert-CoverageToolVersionText {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Text,

        [Parameter(Mandatory = $true)]
        [string]$ExpectedVersion
    )

    $expected = "cargo-llvm-cov $ExpectedVersion"
    $actual = $Text.Trim()
    if ($actual -ne $expected) {
        throw "Managed coverage application reports '$actual', expected exact '$expected'."
    }
}

function Get-CoverageToolVersion {
    param(
        [Parameter(Mandatory = $true)]
        $Identity,

        [Parameter(Mandatory = $true)]
        [string]$ExpectedVersion
    )

    $output = & $Identity.Path 'llvm-cov' '--version' 2>&1
    $exitCode = $LASTEXITCODE
    $text = ($output | Out-String).Trim()
    if ($exitCode -ne 0) {
        throw "Managed coverage application version check failed with exit code ${exitCode}:`n$text"
    }
    Assert-CoverageToolVersionText -Text $text -ExpectedVersion $ExpectedVersion
}

function Invoke-NativeChecked {
    param(
        [Parameter(Mandatory = $true)]
        [string]$FilePath,
        [string[]]$Arguments
    )

    Write-Host "> $FilePath $($Arguments -join ' ')"
    & $FilePath @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "$FilePath $($Arguments -join ' ') failed with exit code $LASTEXITCODE."
    }
}

function Invoke-NativeExpectedFailure {
    param(
        [Parameter(Mandatory = $true)]
        [string]$FilePath,
        [string[]]$Arguments,
        [Parameter(Mandatory = $true)]
        [string]$RequiredPattern,
        [Parameter(Mandatory = $true)]
        [string]$Label
    )

    Write-Host "> $FilePath $($Arguments -join ' ')  # expected failure: $Label"
    $output = & $FilePath @Arguments 2>&1
    $exitCode = $LASTEXITCODE
    $text = ($output | Out-String)
    if ($exitCode -eq 0 -or $text -notmatch $RequiredPattern) {
        throw "$Label did not fail closed as required. Exit=$exitCode`n$text"
    }
}

function Resolve-OrCreateRealAuditDirectory {
    param([string]$Path, [string]$Label)

    $fullPath = [IO.Path]::GetFullPath($Path)
    $root = [IO.Path]::GetPathRoot($fullPath)
    $separators = [char[]]@(
        [IO.Path]::DirectorySeparatorChar,
        [IO.Path]::AltDirectorySeparatorChar
    )
    $segments = @(
        $fullPath.Substring($root.Length).Split(
            $separators,
            [StringSplitOptions]::RemoveEmptyEntries
        )
    )
    $current = $root
    foreach ($segment in $segments) {
        $current = Join-Path $current $segment
        # The parent was checked on the preceding iteration, so creating this
        # single component cannot first traverse an unchecked symlink/junction.
        if (-not (Test-Path -LiteralPath $current)) {
            New-Item -ItemType Directory -Path $current | Out-Null
        }
        $item = Get-Item -LiteralPath $current -Force
        if (-not $item.PSIsContainer -or
            $item.Attributes -band [IO.FileAttributes]::ReparsePoint) {
            throw "$Label ancestor must be a real directory: $current"
        }
    }
    $resolved = (Resolve-Path -LiteralPath $fullPath).Path
    $comparison = if ($IsWindows) { [StringComparison]::OrdinalIgnoreCase } else { [StringComparison]::Ordinal }
    if (-not $resolved.Equals($fullPath, $comparison)) {
        throw "$Label canonical path escaped the named directory: $fullPath -> $resolved"
    }
    return $resolved
}

function New-ManagedAuditDirectory {
    param([string]$Label)

    $tempRoot = Resolve-OrCreateRealAuditDirectory `
        -Path (Join-Path $repoRoot '.RaymanCodingSkill/tmp') `
        -Label 'Managed audit temp'
    $directory = Join-Path $tempRoot ("$Label-$PID-" + [Guid]::NewGuid().ToString('N'))
    return Resolve-OrCreateRealAuditDirectory -Path $directory -Label 'Managed audit run'
}

function Remove-ManagedAuditDirectory {
    param([string]$Path)

    if (-not (Test-Path -LiteralPath $Path)) {
        return
    }
    $managedRoot = (Resolve-Path -LiteralPath (Join-Path $repoRoot '.RaymanCodingSkill/tmp')).Path
    $fullPath = (Resolve-Path -LiteralPath $Path).Path
    $comparison = if ($IsWindows) { [StringComparison]::OrdinalIgnoreCase } else { [StringComparison]::Ordinal }
    $prefix = $managedRoot.TrimEnd([IO.Path]::DirectorySeparatorChar) + [IO.Path]::DirectorySeparatorChar
    $item = Get-Item -LiteralPath $fullPath -Force
    if (-not $item.PSIsContainer -or
        $item.Attributes -band [IO.FileAttributes]::ReparsePoint -or
        -not $fullPath.StartsWith($prefix, $comparison)) {
        throw "Refusing audit cleanup outside managed temp or through reparse: $fullPath"
    }
    Remove-Item -LiteralPath $fullPath -Recurse -Force
}

function Assert-OrdinaryDirectoryTree {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Path,

        [Parameter(Mandatory = $true)]
        [string]$Label
    )

    if (-not (Test-Path -LiteralPath $Path -PathType Container)) {
        throw "$Label is missing or is not a directory: $Path"
    }
    $items = @(
        Get-Item -LiteralPath $Path -Force
        Get-ChildItem -LiteralPath $Path -Force -Recurse
    )
    foreach ($item in $items) {
        if ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) {
            throw "$Label must not contain a symlink or reparse point: $($item.FullName)"
        }
    }
}

function Copy-OrdinaryDirectoryTree {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Source,

        [Parameter(Mandatory = $true)]
        [string]$Destination,

        [Parameter(Mandatory = $true)]
        [string]$Label
    )

    Assert-OrdinaryDirectoryTree -Path $Source -Label "$Label source"
    if (Test-Path -LiteralPath $Destination) {
        throw "$Label destination must be new: $Destination"
    }
    Copy-Item -LiteralPath $Source -Destination $Destination -Recurse -Force
    Assert-OrdinaryDirectoryTree -Path $Destination -Label "$Label copy"
}

function New-IsolatedCargoDenyConfig {
    param(
        [Parameter(Mandatory = $true)]
        [string]$SourceConfig,

        [Parameter(Mandatory = $true)]
        [string]$DatabasePath,

        [Parameter(Mandatory = $true)]
        [string]$OutputConfig
    )

    $sourceItem = Get-Item -LiteralPath $SourceConfig -Force -ErrorAction Stop
    if ($sourceItem.PSIsContainer -or
        $sourceItem.Attributes -band [IO.FileAttributes]::ReparsePoint) {
        throw "cargo-deny source config must be an ordinary file: $SourceConfig"
    }
    $text = Get-Content -LiteralPath $sourceItem.FullName -Raw
    if ($text -match '(?m)^\s*db-path\s*=') {
        throw "cargo-deny source config must not preconfigure db-path: $SourceConfig"
    }
    $sectionPattern = [regex]::new('(?m)^\[advisories\][ \t]*$')
    $sections = @($sectionPattern.Matches($text))
    if ($sections.Count -ne 1) {
        throw "cargo-deny source config must contain exactly one [advisories] section: $SourceConfig"
    }

    $databaseLiteral = ConvertTo-Json -InputObject ([IO.Path]::GetFullPath($DatabasePath)) -Compress
    $section = $sections[0]
    $insertion = [Environment]::NewLine + "db-path = $databaseLiteral"
    $rewritten = $text.Insert($section.Index + $section.Length, $insertion)
    [IO.File]::WriteAllText(
        [IO.Path]::GetFullPath($OutputConfig),
        $rewritten,
        [Text.UTF8Encoding]::new($false)
    )
    $outputItem = Get-Item -LiteralPath $OutputConfig -Force
    if ($outputItem.PSIsContainer -or
        $outputItem.Attributes -band [IO.FileAttributes]::ReparsePoint) {
        throw "cargo-deny isolated config is not an ordinary file: $OutputConfig"
    }
    return $outputItem.FullName
}

function New-IsolatedCargoDenyState {
    param(
        [Parameter(Mandatory = $true)]
        [string]$RootConfig,

        [Parameter(Mandatory = $true)]
        [string]$EvalConfig
    )

    $root = New-ManagedAuditDirectory -Label 'cargo-deny-state'
    try {
        $database = Join-Path $root 'advisory-dbs'
        $cargoHome = if ([string]::IsNullOrWhiteSpace($env:CARGO_HOME)) {
            Join-Path $HOME '.cargo'
        } else {
            $env:CARGO_HOME
        }
        $sourceDatabase = [IO.Path]::GetFullPath((Join-Path $cargoHome 'advisory-dbs'))
        if (Test-Path -LiteralPath $sourceDatabase) {
            Copy-OrdinaryDirectoryTree `
                -Source $sourceDatabase `
                -Destination $database `
                -Label 'cargo-deny advisory database'
        } else {
            $database = Resolve-OrCreateRealAuditDirectory `
                -Path $database `
                -Label 'Isolated cargo-deny advisory database'
        }

        return [pscustomobject]@{
            Root = $root
            Database = $database
            RootConfig = New-IsolatedCargoDenyConfig `
                -SourceConfig $RootConfig `
                -DatabasePath $database `
                -OutputConfig (Join-Path $root 'root-deny.toml')
            EvalConfig = New-IsolatedCargoDenyConfig `
                -SourceConfig $EvalConfig `
                -DatabasePath $database `
                -OutputConfig (Join-Path $root 'eval-deny.toml')
        }
    } catch {
        Remove-ManagedAuditDirectory -Path $root
        throw
    }
}

function Test-OfflineCargoMode {
    if ([string]::IsNullOrWhiteSpace($env:CARGO_NET_OFFLINE)) {
        return $false
    }
    return $env:CARGO_NET_OFFLINE.Trim().ToLowerInvariant() -in @('1', 'true')
}

function Get-CargoDenyArguments {
    param(
        [Parameter(Mandatory = $true)]
        [string]$ConfigPath,

        [string]$ManifestPath
    )

    $arguments = @('deny')
    if (-not [string]::IsNullOrWhiteSpace($ManifestPath)) {
        $arguments += @('--manifest-path', $ManifestPath)
    }
    $arguments += @('check', '--config', $ConfigPath)
    if (Test-OfflineCargoMode) {
        # cargo still receives CARGO_NET_OFFLINE; this flag independently keeps
        # cargo-deny from fetching while retaining the advisories check against
        # the isolated copy of the existing database.
        $arguments += '--disable-fetch'
    }
    return $arguments
}

function Invoke-IsolatedCargoDenyChecks {
    param(
        [Parameter(Mandatory = $true)]
        $CargoDenyIdentity
    )

    $state = New-IsolatedCargoDenyState `
        -RootConfig (Join-Path $repoRoot 'deny.toml') `
        -EvalConfig (Join-Path $repoRoot 'evals/deny.toml')
    try {
        Invoke-NativeChecked $CargoDenyIdentity.Path @(
            Get-CargoDenyArguments -ConfigPath $state.RootConfig
        )
        Invoke-NativeChecked $CargoDenyIdentity.Path @(
            Get-CargoDenyArguments `
                -ConfigPath $state.EvalConfig `
                -ManifestPath 'evals/Cargo.toml'
        )
        Assert-NativeApplicationIdentity -Identity $CargoDenyIdentity
    } finally {
        Remove-ManagedAuditDirectory -Path $state.Root
    }
}
function Assert-ShadowRejection {
    param(
        [Parameter(Mandatory = $true)]
        $Identity
    )

    $functionPath = "Function:local:$($Identity.Name)"
    Set-Item -Path $functionPath -Value { $global:LASTEXITCODE = 0 }
    $rejected = $false
    try {
        Assert-NativeApplicationIdentity -Identity $Identity
    } catch {
        $rejected = $_.Exception.Message -match 'shadowed by Function'
    } finally {
        Remove-Item -Path $functionPath -Force -ErrorAction SilentlyContinue
    }
    if (-not $rejected) {
        throw "Audit self-test failed: Function shadow for '$($Identity.Name)' was not rejected."
    }
}

function Invoke-AuditScriptSelfTest {
    param(
        [Parameter(Mandatory = $true)]
        [array]$NativeIdentities
    )

    foreach ($identity in $NativeIdentities) {
        Assert-ShadowRejection -Identity $identity
        # The injected Function is local to Assert-ShadowRejection and is gone
        # only after that helper scope returns; now prove the real Application
        # becomes effective again.
        Assert-NativeApplicationIdentity -Identity $identity
    }

    $cmdletRejected = $false
    try {
        $null = Resolve-NativeApplication -Name 'Get-Item' -Label 'Cmdlet shadow probe'
    } catch {
        $cmdletRejected = $_.Exception.Message -match 'shadowed by Cmdlet'
    }
    if (-not $cmdletRejected) {
        throw 'Audit self-test failed: a Cmdlet was accepted as a native Application.'
    }

    $aliasName = 'rayman-audit-native-alias-probe'
    Set-Alias -Name $aliasName -Value 'Get-Item' -Scope Local
    $aliasRejected = $false
    try {
        $null = Resolve-NativeApplication -Name $aliasName -Label 'Alias shadow probe'
    } catch {
        $aliasRejected = $_.Exception.Message -match 'shadowed by Alias'
    } finally {
        Remove-Item -Path "Alias:local:$aliasName" -Force -ErrorAction SilentlyContinue
    }
    if (-not $aliasRejected) {
        throw 'Audit self-test failed: an Alias was accepted as a native Application.'
    }

    $cargoIdentity = $NativeIdentities |
        Where-Object Name -EQ 'cargo' |
        Select-Object -First 1
    if ($null -eq $cargoIdentity) {
        throw 'Audit self-test failed: captured cargo identity is missing.'
    }
    Assert-ExpectedApplicationPath -Identity $cargoIdentity -ExpectedPath $cargoIdentity.Path
    $wrongCoveragePathRejected = $false
    try {
        Assert-ExpectedApplicationPath `
            -Identity ([pscustomobject]@{
                Label = 'Managed cargo-llvm-cov'
                Path = $cargoIdentity.Path
            }) `
            -ExpectedPath (Join-Path $PSScriptRoot 'audit-repository.ps1')
    } catch {
        $wrongCoveragePathRejected = $_.Exception.Message -match 'exact managed path'
    }
    if (-not $wrongCoveragePathRejected) {
        throw 'Audit self-test failed: managed coverage application path substitution was not rejected.'
    }

    Assert-CoverageToolVersionText `
        -Text 'cargo-llvm-cov 0.8.7' `
        -ExpectedVersion '0.8.7'
    $wrongCoverageVersionRejected = $false
    try {
        Assert-CoverageToolVersionText `
            -Text 'cargo-llvm-cov 0.8.6' `
            -ExpectedVersion '0.8.7'
    } catch {
        $wrongCoverageVersionRejected = $_.Exception.Message -match 'expected exact'
    }
    if (-not $wrongCoverageVersionRejected) {
        throw 'Audit self-test failed: wrong cargo-llvm-cov version was not rejected.'
    }

    $cargoDenyFixture = New-ManagedAuditDirectory -Label 'cargo-deny-selftest'
    try {
        $fixtureDatabase = Join-Path $cargoDenyFixture 'source-db'
        $fixtureNested = Join-Path $fixtureDatabase 'advisory-db'
        New-Item -ItemType Directory -Path $fixtureNested | Out-Null
        Set-Content -LiteralPath (Join-Path $fixtureDatabase 'db.lock') -Value 'lock' -Encoding utf8
        Set-Content -LiteralPath (Join-Path $fixtureNested 'HEAD') -Value 'fixture' -Encoding utf8
        $databaseCopy = Join-Path $cargoDenyFixture 'database-copy'
        Copy-OrdinaryDirectoryTree `
            -Source $fixtureDatabase `
            -Destination $databaseCopy `
            -Label 'cargo-deny self-test database'
        if (-not (Test-Path -LiteralPath (Join-Path $databaseCopy 'advisory-db/HEAD') -PathType Leaf)) {
            throw 'Audit self-test failed: isolated cargo-deny database copy is incomplete.'
        }

        $fixtureConfig = Join-Path $cargoDenyFixture 'source.toml'
        Set-Content -LiteralPath $fixtureConfig -Value @'
[graph]
all-features = true

[advisories]
version = 2
'@ -Encoding utf8
        $isolatedConfig = New-IsolatedCargoDenyConfig `
            -SourceConfig $fixtureConfig `
            -DatabasePath $databaseCopy `
            -OutputConfig (Join-Path $cargoDenyFixture 'isolated.toml')
        $isolatedText = Get-Content -LiteralPath $isolatedConfig -Raw
        if ($isolatedText -notmatch '(?m)^db-path\s*=' -or
            (Get-Content -LiteralPath $fixtureConfig -Raw) -match '(?m)^db-path\s*=') {
            throw 'Audit self-test failed: isolated cargo-deny db-path injection is missing or mutated the source config.'
        }

        $preconfigured = Join-Path $cargoDenyFixture 'preconfigured.toml'
        Set-Content -LiteralPath $preconfigured -Value @'
[advisories]
db-path = "unexpected"
'@ -Encoding utf8
        $preconfiguredRejected = $false
        try {
            $null = New-IsolatedCargoDenyConfig `
                -SourceConfig $preconfigured `
                -DatabasePath $databaseCopy `
                -OutputConfig (Join-Path $cargoDenyFixture 'must-not-exist.toml')
        } catch {
            $preconfiguredRejected = $_.Exception.Message -match 'must not preconfigure db-path'
        }
        if (-not $preconfiguredRejected) {
            throw 'Audit self-test failed: a preconfigured cargo-deny db-path was accepted.'
        }
    } finally {
        Remove-ManagedAuditDirectory -Path $cargoDenyFixture
    }

    $verifier = Join-Path $PSScriptRoot 'verify-release-contract.ps1'
    if (-not (Test-Path -LiteralPath $verifier -PathType Leaf)) {
        throw "Audit self-test cannot find the release verifier: $verifier"
    }
    & $verifier -SelfTest

    $profileRepair = Join-Path $PSScriptRoot 'repair-rayman-powershell-profile.ps1'
    if (-not (Test-Path -LiteralPath $profileRepair -PathType Leaf)) {
        throw "Audit self-test cannot find the PowerShell profile repair tool: $profileRepair"
    }
    & $profileRepair -SelfTest

    foreach ($identity in $NativeIdentities) {
        Assert-NativeApplicationIdentity -Identity $identity
    }
    Write-Host 'Audit script self-test passed: native shadow/identity, profile migration, isolated advisory state, and managed coverage guards fail closed.'
}

$nativeApplications = [ordered]@{
    Cargo = Resolve-NativeApplication -Name 'cargo' -Label 'Cargo'
    CargoDeny = Resolve-NativeApplication -Name 'cargo-deny' -Label 'cargo-deny'
    Rustup = Resolve-NativeApplication -Name 'rustup' -Label 'rustup'
    Git = Resolve-NativeApplication -Name 'git' -Label 'Git'
    Rustc = Resolve-NativeApplication -Name 'rustc' -Label 'rustc'
}
$capturedNativeIdentities = @($nativeApplications.Values)
Invoke-AuditScriptSelfTest -NativeIdentities $capturedNativeIdentities
if ($SelfTest) {
    Write-Host 'audit-repository.ps1 self-test passed.'
    return
}
if ($DependencyPolicyOnly) {
    Push-Location $repoRoot
    try {
        Invoke-IsolatedCargoDenyChecks -CargoDenyIdentity $nativeApplications.CargoDeny
    } finally {
        Pop-Location
    }
    Write-Host 'Isolated root/evals dependency policy checks passed.'
    return
}
Push-Location $repoRoot
try {
    Invoke-NativeChecked $nativeApplications.Cargo.Path @('fmt', '--all', '--check')
    Invoke-NativeChecked $nativeApplications.Cargo.Path @('clippy', '--locked', '--workspace', '--all-targets', '--all-features', '--', '-D', 'warnings')
    Invoke-NativeChecked $nativeApplications.Cargo.Path @('test', '--locked', '--workspace', '--all-targets')
    Invoke-IsolatedCargoDenyChecks -CargoDenyIdentity $nativeApplications.CargoDeny

    # MSRV is mandatory. rustup run fails explicitly when the declared toolchain
    # is unavailable; the complete audit never silently falls back to stable.
    Invoke-NativeChecked $nativeApplications.Rustup.Path @('run', $MsrvToolchain, 'rustc', '--version')
    Invoke-NativeChecked $nativeApplications.Rustup.Path @('run', $MsrvToolchain, 'cargo', 'build', '--locked', '--release', '-p', 'rayman')
    Invoke-NativeChecked $nativeApplications.Rustup.Path @('run', $MsrvToolchain, 'cargo', 'test', '--locked', '--workspace', '--all-targets')

    # Real shipped-CLI coverage. Always install the exact measurement tool into
    # a fresh managed root, resolve that exact Application, and invoke it by its
    # captured path. An arbitrary PATH copy is never accepted as coverage proof.
    Invoke-NativeChecked $nativeApplications.Rustup.Path @('component', 'add', 'llvm-tools-preview')
    $coverageToolRoot = New-ManagedAuditDirectory -Label "cargo-llvm-cov-$CoverageToolVersion"
    $originalPathForCoverage = $env:PATH
    try {
        Invoke-NativeChecked $nativeApplications.Cargo.Path @(
            'install', 'cargo-llvm-cov', '--locked', '--version', $CoverageToolVersion,
            '--root', $coverageToolRoot
        )
        $coverageBin = Join-Path $coverageToolRoot 'bin'
        $coverageApplicationName = if ($IsWindows) { 'cargo-llvm-cov.exe' } else { 'cargo-llvm-cov' }
        $expectedCoveragePath = Join-Path $coverageBin $coverageApplicationName
        $fixedCargoDirectory = Split-Path -Parent $nativeApplications.Cargo.Path
        $env:PATH = @(
            $coverageBin,
            $fixedCargoDirectory,
            $originalPathForCoverage
        ) -join [IO.Path]::PathSeparator
        $coverageIdentity = Resolve-NativeApplication `
            -Name 'cargo-llvm-cov' `
            -Label 'Managed cargo-llvm-cov'
        Assert-ExpectedApplicationPath `
            -Identity $coverageIdentity `
            -ExpectedPath $expectedCoveragePath
        Get-CoverageToolVersion `
            -Identity $coverageIdentity `
            -ExpectedVersion $CoverageToolVersion
        Invoke-NativeChecked $coverageIdentity.Path @(
            'llvm-cov', '--locked', '--workspace', '--all-features', '--all-targets',
            '--fail-under-lines', $MinimumCliLineCoverage.ToString()
        )
        Assert-NativeApplicationIdentity -Identity $coverageIdentity
    } finally {
        $env:PATH = $originalPathForCoverage
        Remove-ManagedAuditDirectory -Path $coverageToolRoot
    }

    Invoke-NativeChecked $nativeApplications.Cargo.Path @('fmt', '--manifest-path', 'evals/Cargo.toml', '--all', '--check')
    Invoke-NativeChecked $nativeApplications.Cargo.Path @('clippy', '--manifest-path', 'evals/Cargo.toml', '--locked', '--all-targets', '--all-features', '--', '-D', 'warnings')
    Invoke-NativeChecked $nativeApplications.Cargo.Path @('test', '--manifest-path', 'evals/Cargo.toml', '--locked', '--all-targets')

    Invoke-NativeExpectedFailure $nativeApplications.Cargo.Path @(
        'run', '--manifest-path', 'evals/Cargo.toml', '--locked', '--',
        '--backend', 'anthropic', '--task', 'fix-failing-test', '--trials', '1'
    ) '--unsafe-host-exec' 'Real eval backend host-exec guard'

    $customTasksRoot = New-ManagedAuditDirectory -Label 'custom-grade-guard'
    try {
        $customTask = Join-Path $customTasksRoot 'custom'
        New-Item -ItemType Directory -Path (Join-Path $customTask 'fixture/src') | Out-Null
        Set-Content -LiteralPath (Join-Path $customTask 'prompt.md') -Value 'Make no change.' -Encoding utf8
        Set-Content -LiteralPath (Join-Path $customTask 'grade.txt') -Value 'echo must-not-run' -Encoding utf8
        Set-Content -LiteralPath (Join-Path $customTask 'fixture/src/lib.rs') -Value 'pub fn sample() {}' -Encoding utf8
        Invoke-NativeExpectedFailure $nativeApplications.Cargo.Path @(
            'run', '--manifest-path', 'evals/Cargo.toml', '--locked', '--',
            '--backend', 'mock', '--tasks', $customTasksRoot, '--task', 'custom',
            '--trials', '1', '--runs-dir', (Join-Path $customTasksRoot 'runs')
        ) '--unsafe-custom-grade-exec' 'Custom grade host-exec guard'
    } finally {
        Remove-ManagedAuditDirectory -Path $customTasksRoot
    }

    $evalRuns = New-ManagedAuditDirectory -Label 'offline-eval-smoke'
    try {
        Invoke-NativeChecked $nativeApplications.Cargo.Path @(
            'run', '--manifest-path', 'evals/Cargo.toml', '--locked', '--',
            '--backend', 'mock', '--task', 'fix-failing-test', '--trials', '2',
            '--seed', '20260714', '--runs-dir', $evalRuns
        )
        $latest = Get-Content -Raw -LiteralPath (Join-Path $evalRuns 'latest.json') |
            ConvertFrom-Json -ErrorAction Stop
        if ([string]::IsNullOrWhiteSpace($latest.run_id) -or
            -not $latest.report_json.StartsWith("$($latest.run_id)/")) {
            throw 'Offline eval latest.json does not point into its immutable run.'
        }
        $report = Get-Content -Raw -LiteralPath (Join-Path $evalRuns $latest.report_json) |
            ConvertFrom-Json -ErrorAction Stop
        if ($report.provenance.execution_mode -ne 'mock' -or
            [uint64]$report.provenance.seed -ne [uint64]20260714 -or
            $report.provenance.grade_execution.mode -ne 'trusted_builtin_manifest' -or
            $report.provenance.grade_execution.custom_execution_acknowledged -or
            @($report.trials).Count -ne 4) {
            throw 'Offline eval smoke has unexpected execution/grade provenance or trial count.'
        }
    } finally {
        Remove-ManagedAuditDirectory -Path $evalRuns
    }

    # Packaging and cargo-install are separate from a workspace build. Exercise
    # both so missing package metadata/files cannot survive until release day.
    Invoke-NativeChecked $nativeApplications.Cargo.Path @('package', '--locked', '-p', 'rayman')
    $smokeRoot = [IO.Path]::GetFullPath(
        (Join-Path '.RaymanCodingSkill/tmp' ("audit-install-$PID-" + [Guid]::NewGuid().ToString('N')))
    )
    $managedTempRoot = [IO.Path]::GetFullPath((Join-Path $repoRoot '.RaymanCodingSkill/tmp'))
    $comparison = if ($IsWindows) { [StringComparison]::OrdinalIgnoreCase } else { [StringComparison]::Ordinal }
    $managedPrefix = $managedTempRoot.TrimEnd([IO.Path]::DirectorySeparatorChar) + [IO.Path]::DirectorySeparatorChar
    if (-not $smokeRoot.StartsWith($managedPrefix, $comparison)) {
        throw "Refusing install-smoke directory outside managed temp: $smokeRoot"
    }
    try {
        Invoke-NativeChecked $nativeApplications.Cargo.Path @('install', '--locked', '--path', 'crates/rayman', '--root', $smokeRoot)
        $smokeCli = Join-Path (Join-Path $smokeRoot 'bin') $artifactName
        Invoke-NativeChecked $smokeCli @('--version')
    } finally {
        if (Test-Path -LiteralPath $smokeRoot) {
            $smokeItem = Get-Item -LiteralPath $smokeRoot -Force
            if (-not $smokeItem.PSIsContainer -or
                $smokeItem.Attributes -band [IO.FileAttributes]::ReparsePoint -or
                -not $smokeItem.FullName.StartsWith($managedPrefix, $comparison)) {
                throw "Refusing unsafe install-smoke cleanup target: $smokeRoot"
            }
            Remove-Item -LiteralPath $smokeRoot -Recurse -Force
        }
    }

    Invoke-NativeChecked $nativeApplications.Cargo.Path @('build', '--locked', '--release', '-p', 'rayman')
    $referenceArtifact = (Resolve-Path -LiteralPath (Join-Path 'target/release' $artifactName)).Path
    $resolvedCli = (Resolve-Path -LiteralPath $CliPath).Path
    $resolvedSkill = (Resolve-Path -LiteralPath $SkillPath).Path

    # Use the current source artifact explicitly for workspace self-dogfood;
    # installed identity is a separate final contract below.
    Invoke-NativeChecked $referenceArtifact @('context', 'refresh')
    Invoke-NativeChecked $referenceArtifact @('map', 'quality', '--profile', 'strict', '--check')
    Invoke-NativeChecked $referenceArtifact @('check', '--profile', 'release')
    Invoke-NativeChecked $referenceArtifact @('state', 'audit', '--check')
    Invoke-NativeChecked $referenceArtifact @('assets')

    $checkpointRoot = New-ManagedAuditDirectory -Label 'checkpoint-smoke'
    try {
        Invoke-NativeChecked $referenceArtifact @('checkpoint', '--dir', $checkpointRoot, 'save', '--keep', '1')
        Invoke-NativeChecked $referenceArtifact @('checkpoint', '--dir', $checkpointRoot, 'verify', 'latest')
    } finally {
        Remove-ManagedAuditDirectory -Path $checkpointRoot
    }

    & './scripts/verify-release-contract.ps1' `
        -CliPath $resolvedCli `
        -ReferenceCliPath $referenceArtifact `
        -SkillPath $resolvedSkill `
        -RequirePath `
        -RequireSourceFresh

    foreach ($identity in $capturedNativeIdentities) {
        Assert-NativeApplicationIdentity -Identity $identity
    }
} finally {
    Pop-Location
}

Write-Host 'Complete repository audit passed: root/MSRV/CLI coverage, evals safety+mock provenance, package/install, strict workspace, checkpoint/state/assets, and installed release identity.'
