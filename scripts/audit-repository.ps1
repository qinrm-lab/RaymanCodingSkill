[CmdletBinding(DefaultParameterSetName = 'Audit')]
param(
    [Parameter(Mandatory = $true, ParameterSetName = 'Audit')]
    [ValidateNotNullOrEmpty()]
    [string]$CliPath,

    [Parameter(Mandatory = $true, ParameterSetName = 'Audit')]
    [ValidateNotNullOrEmpty()]
    [string]$SkillPath,

    [Parameter(ParameterSetName = 'Audit')]
    [Parameter(ParameterSetName = 'PrepareAuditTools')]
    [ValidateSet('1.97.1')]
    [string]$MsrvToolchain = '1.97.1',

    [Parameter(ParameterSetName = 'Audit')]
    [ValidateRange(75, 100)]
    [int]$MinimumCliLineCoverage = 75,

    [Parameter(ParameterSetName = 'Audit')]
    [Parameter(ParameterSetName = 'PrepareAuditTools')]
    [ValidateSet('0.8.7')]
    [string]$CoverageToolVersion = '0.8.7',

    [Parameter(Mandatory = $true, ParameterSetName = 'PrepareAuditTools')]
    [switch]$PrepareAuditTools,

    [Parameter(ParameterSetName = 'Audit')]
    [Parameter(ParameterSetName = 'DependencyPolicy')]
    [ValidateNotNullOrEmpty()]
    [string]$CargoDenyDatabaseSeedPath,

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

switch ($PSCmdlet.ParameterSetName) {
    'PrepareAuditTools' {
        if (-not $PrepareAuditTools.IsPresent) {
            throw 'The PrepareAuditTools parameter set requires -PrepareAuditTools to be present and true; -PrepareAuditTools:$false grants no provisioning authority.'
        }
    }
    'DependencyPolicy' {
        if (-not $DependencyPolicyOnly.IsPresent) {
            throw 'The DependencyPolicy parameter set requires -DependencyPolicyOnly to be present and true.'
        }
    }
    'SelfTest' {
        if (-not $SelfTest.IsPresent) {
            throw 'The SelfTest parameter set requires -SelfTest to be present and true.'
        }
    }
}

$repoRoot = Split-Path -Parent $PSScriptRoot
$artifactName = if ($IsWindows) { 'rayman.exe' } else { 'rayman' }
$script:CurrentAuditPhase = 'bootstrap'
$script:AuditIntegrationTestName = 'audit_self_test_exercises_only_the_audit_contract'

function Write-AuditPhase {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Name,
        [Parameter(Mandatory = $true)]
        [ValidateSet('start', 'pass', 'fail')]
        [string]$Status,
        [string]$Detail
    )

    if ($Status -eq 'start') {
        $script:CurrentAuditPhase = $Name
    }
    $record = [ordered]@{
        schema = 'rayman.audit.phase.v1'
        phase = $Name
        status = $Status
        timestamp_utc = [DateTimeOffset]::UtcNow.ToString('O')
    }
    if (-not [string]::IsNullOrWhiteSpace($Detail)) {
        $record.detail = $Detail
    }
    Write-Output ('RAYMAN_AUDIT_PHASE ' + ($record | ConvertTo-Json -Compress))
}

function Invoke-SourceFreshInputInspection {
    $verifier = Join-Path $PSScriptRoot 'verify-release-contract.ps1'
    if (-not (Test-Path -LiteralPath $verifier -PathType Leaf)) {
        throw "Audit cannot find the release verifier source-fresh input inspector: $verifier"
    }
    $output = & $verifier -InspectSourceFreshInputs
    $text = ($output | Out-String).Trim()
    if ([string]::IsNullOrWhiteSpace($text)) {
        throw 'Release verifier source-fresh input inspection returned no output.'
    }
    try {
        $inspection = $text | ConvertFrom-Json -Depth 20 -ErrorAction Stop
        if ($inspection.schema -ne 'rayman.source-fresh.input-inspection.v1' -or
            $inspection.workspace_activation.schema -ne 'rayman.workspace-activation.snapshot.v1' -or
            $inspection.source_fresh_environment.schema -ne 'rayman.source-fresh.environment.v1' -or
            $inspection.source_fresh_environment.clear -ne $true -or
            @($inspection.source_fresh_environment.rejected_names).Count -ne 0) {
            throw 'inspection contract is incomplete or not clear'
        }
    } catch {
        throw "Release verifier source-fresh input inspection failed closed: $($_.Exception.Message)"
    }
    return $inspection
}

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
    $resolved = (Resolve-Path -LiteralPath $effective.Source).ProviderPath
    return [pscustomobject]@{
        Name = $Name
        Label = $Label
        Path = $resolved
        Sha256 = Get-FileSha256 -Path $resolved
    }
}

function Resolve-AuditNativeApplications {
    param(
        [Parameter(Mandatory = $true)]
        [bool]$IncludeCompleteAuditTools,

        [Parameter(Mandatory = $true)]
        [bool]$IncludePreinstalledCoverageTool,

        [scriptblock]$Resolver
    )

    if ($null -eq $Resolver) {
        $Resolver = {
            param([string]$Name, [string]$Label)
            Resolve-NativeApplication -Name $Name -Label $Label
        }
    }
    $applications = [ordered]@{
        Cargo = & $Resolver -Name 'cargo' -Label 'Cargo'
        CargoDeny = & $Resolver -Name 'cargo-deny' -Label 'cargo-deny'
    }
    if ($IncludeCompleteAuditTools) {
        $applications.Rustup = & $Resolver -Name 'rustup' -Label 'rustup'
        $applications.Git = & $Resolver -Name 'git' -Label 'Git'
        $applications.Rustc = & $Resolver -Name 'rustc' -Label 'rustc'
    }
    if ($IncludePreinstalledCoverageTool) {
        try {
            $applications.CargoLlvmCov = & $Resolver `
                -Name 'cargo-llvm-cov' `
                -Label 'cargo-llvm-cov'
        } catch {
            throw "A preinstalled cargo-llvm-cov $CoverageToolVersion Application is required by the default audit. Run 'pwsh -NoProfile -File scripts/audit-repository.ps1 -PrepareAuditTools' as a separate explicit provisioning step, then rerun the audit. $($_.Exception.Message)"
        }
    }
    return $applications
}

function Invoke-AuditBootstrap {
    param(
        [Parameter(Mandatory = $true)]
        [bool]$IncludeCompleteAuditTools,

        [Parameter(Mandatory = $true)]
        [bool]$IncludePreinstalledCoverageTool,

        [Parameter(Mandatory = $true)]
        [ref]$Applications,
        [scriptblock]$Resolver,
        [scriptblock]$PhaseWriter
    )

    if ($null -eq $PhaseWriter) {
        $PhaseWriter = {
            param([string]$Name, [string]$Status, [string]$Detail)
            Write-AuditPhase -Name $Name -Status $Status -Detail $Detail
        }
    }
    & $PhaseWriter -Name 'bootstrap' -Status 'start' -Detail $null
    try {
        $Applications.Value = Resolve-AuditNativeApplications `
            -IncludeCompleteAuditTools $IncludeCompleteAuditTools `
            -IncludePreinstalledCoverageTool $IncludePreinstalledCoverageTool `
            -Resolver $Resolver
        & $PhaseWriter -Name 'bootstrap' -Status 'pass' -Detail $null
    } catch {
        & $PhaseWriter -Name 'bootstrap' -Status 'fail' -Detail $_.Exception.Message
        throw
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
    $resolvedExpected = (Resolve-Path -LiteralPath $ExpectedPath).ProviderPath
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

function New-ExactApplicationIdentity {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Name,

        [Parameter(Mandatory = $true)]
        [string]$Label,

        [Parameter(Mandatory = $true)]
        [string]$Path
    )

    $item = Get-Item -LiteralPath $Path -Force -ErrorAction Stop
    if ($item.PSIsContainer -or
        $item.Attributes -band [IO.FileAttributes]::ReparsePoint) {
        throw "$Label must resolve to an ordinary application file: $Path"
    }
    $resolved = (Resolve-Path -LiteralPath $item.FullName).ProviderPath
    return [pscustomobject]@{
        Name = $Name
        Label = $Label
        Path = $resolved
        Sha256 = Get-FileSha256 -Path $resolved
    }
}

function Assert-ExactApplicationIdentity {
    param(
        [Parameter(Mandatory = $true)]
        $Identity
    )

    $current = New-ExactApplicationIdentity `
        -Name $Identity.Name `
        -Label $Identity.Label `
        -Path $Identity.Path
    if (-not $current.Path.Equals($Identity.Path, $pathComparison) -or
        $current.Sha256 -ne $Identity.Sha256) {
        throw "$($Identity.Label) application identity changed during audit: $($Identity.Path) [$($Identity.Sha256)] -> $($current.Path) [$($current.Sha256)]"
    }
}

function Invoke-NativeCaptured {
    param(
        [Parameter(Mandatory = $true)]
        [string]$FilePath,
        [string[]]$Arguments
    )

    Write-Host "> $FilePath $($Arguments -join ' ')"
    $output = & $FilePath @Arguments 2>&1
    return [pscustomobject]@{
        ExitCode = $LASTEXITCODE
        Output = @($output)
    }
}

function Resolve-RustupToolchainApplication {
    param(
        [Parameter(Mandatory = $true)]
        $RustupIdentity,

        [Parameter(Mandatory = $true)]
        [string]$Toolchain,

        [Parameter(Mandatory = $true)]
        [ValidateSet('cargo', 'rustc')]
        [string]$Name,

        [scriptblock]$Invoker
    )

    if ($null -eq $Invoker) {
        $Invoker = {
            param([string]$FilePath, [string[]]$Arguments)
            Invoke-NativeCaptured -FilePath $FilePath -Arguments $Arguments
        }
    }
    $label = "MSRV $Toolchain $Name"
    $result = & $Invoker `
        -FilePath $RustupIdentity.Path `
        -Arguments @('which', $Name, '--toolchain', $Toolchain)
    if ($null -eq $result -or $result.ExitCode -ne 0) {
        $detail = if ($null -eq $result) { '<no result>' } else { ($result.Output | Out-String).Trim() }
        throw "$label resolution through rustup failed: $detail"
    }
    $lines = @(
        $result.Output |
            ForEach-Object { $_.ToString().Trim() } |
            Where-Object { -not [string]::IsNullOrWhiteSpace($_) }
    )
    if ($lines.Count -ne 1) {
        throw "$label resolution returned $($lines.Count) non-empty lines; expected one exact application path."
    }
    return New-ExactApplicationIdentity `
        -Name $Name `
        -Label $label `
        -Path $lines[0]
}

function Resolve-MsrvToolchainApplications {
    param(
        [Parameter(Mandatory = $true)]
        $RustupIdentity,

        [Parameter(Mandatory = $true)]
        [string]$Toolchain,

        [scriptblock]$Invoker
    )

    return [pscustomobject]@{
        Cargo = Resolve-RustupToolchainApplication `
            -RustupIdentity $RustupIdentity `
            -Toolchain $Toolchain `
            -Name 'cargo' `
            -Invoker $Invoker
        Rustc = Resolve-RustupToolchainApplication `
            -RustupIdentity $RustupIdentity `
            -Toolchain $Toolchain `
            -Name 'rustc' `
            -Invoker $Invoker
    }
}

function Resolve-MsrvLlvmApplications {
    param(
        [Parameter(Mandatory = $true)]
        $RustcIdentity,

        [scriptblock]$Invoker
    )

    if ($null -eq $Invoker) {
        $Invoker = {
            param([string]$FilePath, [string[]]$Arguments)
            Invoke-NativeCaptured -FilePath $FilePath -Arguments $Arguments
        }
    }
    Assert-ExactApplicationIdentity -Identity $RustcIdentity
    $result = & $Invoker `
        -FilePath $RustcIdentity.Path `
        -Arguments @('--print', 'target-libdir')
    if ($null -eq $result -or $result.ExitCode -ne 0) {
        $detail = if ($null -eq $result) { '<no result>' } else { ($result.Output | Out-String).Trim() }
        throw "$($RustcIdentity.Label) target-libdir resolution failed: $detail"
    }
    $lines = @(
        $result.Output |
            ForEach-Object { $_.ToString().Trim() } |
            Where-Object { -not [string]::IsNullOrWhiteSpace($_) }
    )
    if ($lines.Count -ne 1) {
        throw "$($RustcIdentity.Label) target-libdir returned $($lines.Count) non-empty lines; expected one exact directory path."
    }
    $targetLibItem = Get-Item -LiteralPath $lines[0] -Force -ErrorAction Stop
    if (-not $targetLibItem.PSIsContainer -or
        $targetLibItem.Attributes -band [IO.FileAttributes]::ReparsePoint) {
        throw "$($RustcIdentity.Label) target-libdir must be an ordinary directory: $($lines[0])"
    }
    $targetLibDirectory = (Resolve-Path -LiteralPath $targetLibItem.FullName).ProviderPath
    $llvmBin = Join-Path (Split-Path -Parent $targetLibDirectory) 'bin'
    $suffix = if ($IsWindows) { '.exe' } else { '' }
    $llvmCov = New-ExactApplicationIdentity `
        -Name 'llvm-cov' `
        -Label "$($RustcIdentity.Label) llvm-cov" `
        -Path (Join-Path $llvmBin "llvm-cov$suffix")
    $llvmProfdata = New-ExactApplicationIdentity `
        -Name 'llvm-profdata' `
        -Label "$($RustcIdentity.Label) llvm-profdata" `
        -Path (Join-Path $llvmBin "llvm-profdata$suffix")
    Assert-ExactApplicationIdentity -Identity $RustcIdentity
    return [pscustomobject]@{
        LlvmCov = $llvmCov
        LlvmProfdata = $llvmProfdata
    }
}

function Assert-RustToolVersion {
    param(
        [Parameter(Mandatory = $true)]
        $Identity,

        [Parameter(Mandatory = $true)]
        [string]$ExpectedVersion
    )

    $result = Invoke-NativeCaptured -FilePath $Identity.Path -Arguments @('--version')
    $text = ($result.Output | Out-String).Trim()
    $pattern = '^' + [regex]::Escape($Identity.Name) + ' ' + [regex]::Escape($ExpectedVersion) + '(?:\s|$)'
    if ($result.ExitCode -ne 0 -or $text -notmatch $pattern) {
        throw "$($Identity.Label) reports '$text', expected exact $($Identity.Name) $ExpectedVersion."
    }
}

function Get-EnvironmentSnapshot {
    param(
        [Parameter(Mandatory = $true)]
        [string[]]$Names
    )

    $snapshot = @{}
    foreach ($name in $Names) {
        $path = "Env:$name"
        $present = Test-Path -LiteralPath $path
        $snapshot[$name] = [pscustomobject]@{
            Present = $present
            Value = if ($present) { (Get-Item -LiteralPath $path).Value } else { $null }
        }
    }
    return $snapshot
}

function Restore-EnvironmentSnapshot {
    param(
        [Parameter(Mandatory = $true)]
        [hashtable]$Snapshot
    )

    foreach ($name in $Snapshot.Keys) {
        $path = "Env:$name"
        if ($Snapshot[$name].Present) {
            Set-Item -LiteralPath $path -Value $Snapshot[$name].Value
        } else {
            Remove-Item -LiteralPath $path -ErrorAction SilentlyContinue
        }
    }
}

function Invoke-IsolatedMsrvChecks {
    param(
        [Parameter(Mandatory = $true)]
        $CargoIdentity,

        [Parameter(Mandatory = $true)]
        $RustcIdentity,

        [string]$SkippedIntegrationTest,

        [scriptblock]$CommandRunner
    )

    if ($null -eq $CommandRunner) {
        $CommandRunner = {
            param([string]$FilePath, [string[]]$Arguments)
            Invoke-NativeChecked -FilePath $FilePath -Arguments $Arguments
        }
    }
    $environmentNames = @(
        'RUSTC',
        'CARGO_BUILD_RUSTC',
        'CARGO_TARGET_DIR',
        'RUSTC_WRAPPER',
        'RUSTC_WORKSPACE_WRAPPER'
    )
    $environmentSnapshot = Get-EnvironmentSnapshot -Names $environmentNames
    $targetDirectory = New-ManagedAuditDirectory -Label 'msrv-target'
    try {
        $env:RUSTC = $RustcIdentity.Path
        $env:CARGO_BUILD_RUSTC = $RustcIdentity.Path
        $env:CARGO_TARGET_DIR = $targetDirectory
        Remove-Item Env:RUSTC_WRAPPER -ErrorAction SilentlyContinue
        Remove-Item Env:RUSTC_WORKSPACE_WRAPPER -ErrorAction SilentlyContinue

        Assert-ExactApplicationIdentity -Identity $CargoIdentity
        Assert-ExactApplicationIdentity -Identity $RustcIdentity
        & $CommandRunner `
            -FilePath $CargoIdentity.Path `
            -Arguments @('build', '--locked', '--release', '-p', 'rayman')
        $testArguments = @('test', '--locked', '--workspace', '--all-targets')
        if (-not [string]::IsNullOrWhiteSpace($SkippedIntegrationTest)) {
            $testArguments += @('--', '--skip', $SkippedIntegrationTest)
        }
        & $CommandRunner `
            -FilePath $CargoIdentity.Path `
            -Arguments $testArguments
        Assert-ExactApplicationIdentity -Identity $CargoIdentity
        Assert-ExactApplicationIdentity -Identity $RustcIdentity
    } finally {
        try {
            Restore-EnvironmentSnapshot -Snapshot $environmentSnapshot
        } finally {
            Remove-ManagedAuditDirectory -Path $targetDirectory
        }
    }
}

function Invoke-IsolatedCoverageCheck {
    param(
        [Parameter(Mandatory = $true)]
        $CoverageIdentity,

        [Parameter(Mandatory = $true)]
        $CargoIdentity,

        [Parameter(Mandatory = $true)]
        $RustcIdentity,

        [Parameter(Mandatory = $true)]
        $LlvmCovIdentity,

        [Parameter(Mandatory = $true)]
        $LlvmProfdataIdentity,

        [Parameter(Mandatory = $true)]
        [int]$MinimumLineCoverage,

        [string]$SkippedIntegrationTest,

        [scriptblock]$CommandRunner
    )

    if ($null -eq $CommandRunner) {
        $CommandRunner = {
            param([string]$FilePath, [string[]]$Arguments)
            Invoke-NativeChecked -FilePath $FilePath -Arguments $Arguments
        }
    }
    $environmentNames = @(
        'PATH',
        'CARGO',
        'RUSTC',
        'CARGO_BUILD_RUSTC',
        'CARGO_TARGET_DIR',
        'LLVM_COV',
        'LLVM_PROFDATA',
        'RUSTC_WRAPPER',
        'RUSTC_WORKSPACE_WRAPPER'
    )
    $environmentSnapshot = Get-EnvironmentSnapshot -Names $environmentNames
    $targetDirectory = New-ManagedAuditDirectory -Label 'coverage-target'
    try {
        $pathEntries = @(
            (Split-Path -Parent $CoverageIdentity.Path),
            (Split-Path -Parent $CargoIdentity.Path)
        )
        if ($environmentSnapshot.PATH.Present -and
            -not [string]::IsNullOrWhiteSpace($environmentSnapshot.PATH.Value)) {
            $pathEntries += $environmentSnapshot.PATH.Value
        }
        $env:PATH = $pathEntries -join [IO.Path]::PathSeparator
        $env:CARGO = $CargoIdentity.Path
        $env:RUSTC = $RustcIdentity.Path
        $env:CARGO_BUILD_RUSTC = $RustcIdentity.Path
        $env:CARGO_TARGET_DIR = $targetDirectory
        $env:LLVM_COV = $LlvmCovIdentity.Path
        $env:LLVM_PROFDATA = $LlvmProfdataIdentity.Path
        Remove-Item Env:RUSTC_WRAPPER -ErrorAction SilentlyContinue
        Remove-Item Env:RUSTC_WORKSPACE_WRAPPER -ErrorAction SilentlyContinue

        foreach ($identity in @(
            $CoverageIdentity,
            $CargoIdentity,
            $RustcIdentity,
            $LlvmCovIdentity,
            $LlvmProfdataIdentity
        )) {
            Assert-ExactApplicationIdentity -Identity $identity
        }
        $coverageArguments = @(
            'llvm-cov', '--locked', '--workspace', '--all-features', '--all-targets',
            '--fail-under-lines', $MinimumLineCoverage.ToString()
        )
        if (-not [string]::IsNullOrWhiteSpace($SkippedIntegrationTest)) {
            $coverageArguments += @('--', '--skip', $SkippedIntegrationTest)
        }
        & $CommandRunner `
            -FilePath $CoverageIdentity.Path `
            -Arguments $coverageArguments
        foreach ($identity in @(
            $CoverageIdentity,
            $CargoIdentity,
            $RustcIdentity,
            $LlvmCovIdentity,
            $LlvmProfdataIdentity
        )) {
            Assert-ExactApplicationIdentity -Identity $identity
        }
    } finally {
        try {
            Restore-EnvironmentSnapshot -Snapshot $environmentSnapshot
        } finally {
            Remove-ManagedAuditDirectory -Path $targetDirectory
        }
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

function Get-MsrvLlvmPreparationArguments {
    param(
        [Parameter(Mandatory = $true)]
        [bool]$Authorized,

        [Parameter(Mandatory = $true)]
        [string]$Toolchain
    )

    if (-not $Authorized) {
        return @()
    }
    return [string[]]@(
        'component', 'add', 'llvm-tools-preview', '--toolchain', $Toolchain
    )
}

function Get-CoverageToolPreparationArguments {
    param(
        [Parameter(Mandatory = $true)]
        [bool]$Authorized,

        [Parameter(Mandatory = $true)]
        [string]$Version,

        [string]$Root
    )

    if (-not $Authorized) {
        return @()
    }
    if ([string]::IsNullOrWhiteSpace($Root)) {
        throw 'Coverage tool preparation requires a managed installation root.'
    }
    return [string[]]@(
        'install', 'cargo-llvm-cov', '--locked', '--version', $Version,
        '--root', $Root
    )
}

function Resolve-PersistentCargoInstallRoot {
    $candidate = if (-not [string]::IsNullOrWhiteSpace($env:CARGO_INSTALL_ROOT)) {
        $env:CARGO_INSTALL_ROOT
    } elseif (-not [string]::IsNullOrWhiteSpace($env:CARGO_HOME)) {
        $env:CARGO_HOME
    } else {
        $profileRoot = [Environment]::GetFolderPath([Environment+SpecialFolder]::UserProfile)
        if ([string]::IsNullOrWhiteSpace($profileRoot)) {
            throw 'Cannot resolve a persistent Cargo install root: UserProfile is empty.'
        }
        Join-Path $profileRoot '.cargo'
    }
    if (-not [IO.Path]::IsPathFullyQualified($candidate)) {
        throw "Persistent Cargo install root must be an absolute path: $candidate"
    }
    return [IO.Path]::GetFullPath($candidate)
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
    $resolved = (Resolve-Path -LiteralPath $fullPath).ProviderPath
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
    $managedRoot = (Resolve-Path -LiteralPath (Join-Path $repoRoot '.RaymanCodingSkill/tmp')).ProviderPath
    $fullPath = (Resolve-Path -LiteralPath $Path).ProviderPath
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
        [string]$EvalConfig,

        [string]$DatabaseSeedPath
    )

    $root = New-ManagedAuditDirectory -Label 'cargo-deny-state'
    try {
        $database = Join-Path $root 'advisory-dbs'
        $explicitSeed = -not [string]::IsNullOrWhiteSpace($DatabaseSeedPath)
        $sourceDatabase = if ($explicitSeed) {
            [IO.Path]::GetFullPath($DatabaseSeedPath)
        } else {
            $cargoHome = if ([string]::IsNullOrWhiteSpace($env:CARGO_HOME)) {
                Join-Path $HOME '.cargo'
            } else {
                $env:CARGO_HOME
            }
            [IO.Path]::GetFullPath((Join-Path $cargoHome 'advisory-dbs'))
        }
        if (Test-Path -LiteralPath $sourceDatabase -PathType Container) {
            Copy-OrdinaryDirectoryTree `
                -Source $sourceDatabase `
                -Destination $database `
                -Label 'cargo-deny advisory database'
        } elseif (Test-Path -LiteralPath $sourceDatabase) {
            throw "cargo-deny advisory database seed must be a directory: $sourceDatabase"
        } elseif ($explicitSeed) {
            throw "Explicit cargo-deny advisory database seed is missing: $sourceDatabase"
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
        $CargoDenyIdentity,

        [string]$DatabaseSeedPath
    )

    $state = New-IsolatedCargoDenyState `
        -RootConfig (Join-Path $repoRoot 'deny.toml') `
        -EvalConfig (Join-Path $repoRoot 'evals/deny.toml') `
        -DatabaseSeedPath $DatabaseSeedPath
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

    $bootstrapRecords = [Collections.Generic.List[object]]::new()
    $bootstrapWriter = {
        param([string]$Name, [string]$Status, [string]$Detail)
        $bootstrapRecords.Add([pscustomobject]@{
            Name = $Name
            Status = $Status
            Detail = $Detail
        })
    }.GetNewClosure()
    $missingRustupResolver = {
        param([string]$Name, [string]$Label)
        if ($Name -eq 'rustup') {
            throw 'rustup application is missing from PATH: rustup'
        }
        return [pscustomobject]@{
            Name = $Name
            Label = $Label
            Path = "self-test-$Name"
            Sha256 = ('a' * 64)
        }
    }
    $bootstrapApplications = $null
    $missingRustupRejected = $false
    try {
        Invoke-AuditBootstrap `
            -IncludeCompleteAuditTools $true `
            -IncludePreinstalledCoverageTool $true `
            -Applications ([ref]$bootstrapApplications) `
            -Resolver $missingRustupResolver `
            -PhaseWriter $bootstrapWriter
    } catch {
        $missingRustupRejected = $_.Exception.Message -match 'rustup application is missing'
    }
    if (-not $missingRustupRejected -or
        $bootstrapRecords.Count -ne 2 -or
        $bootstrapRecords[0].Name -ne 'bootstrap' -or
        $bootstrapRecords[0].Status -ne 'start' -or
        $bootstrapRecords[1].Name -ne 'bootstrap' -or
        $bootstrapRecords[1].Status -ne 'fail' -or
        $bootstrapRecords[1].Detail -notmatch 'rustup application is missing') {
        throw 'Audit self-test failed: missing rustup did not produce a structured bootstrap start/fail pair.'
    }

    $missingCoverageRecords = [Collections.Generic.List[object]]::new()
    $missingCoverageWriter = {
        param([string]$Name, [string]$Status, [string]$Detail)
        $missingCoverageRecords.Add([pscustomobject]@{
            Name = $Name
            Status = $Status
            Detail = $Detail
        })
    }.GetNewClosure()
    $missingCoverageResolver = {
        param([string]$Name, [string]$Label)
        if ($Name -eq 'cargo-llvm-cov') {
            throw 'cargo-llvm-cov application is missing from PATH: cargo-llvm-cov'
        }
        return [pscustomobject]@{
            Name = $Name
            Label = $Label
            Path = "self-test-$Name"
            Sha256 = ('a' * 64)
        }
    }
    $missingCoverageApplications = $null
    $missingCoverageRejected = $false
    try {
        Invoke-AuditBootstrap `
            -IncludeCompleteAuditTools $true `
            -IncludePreinstalledCoverageTool $true `
            -Applications ([ref]$missingCoverageApplications) `
            -Resolver $missingCoverageResolver `
            -PhaseWriter $missingCoverageWriter
    } catch {
        $missingCoverageRejected = $_.Exception.Message -match 'preinstalled cargo-llvm-cov 0\.8\.7.*-PrepareAuditTools'
    }
    if (-not $missingCoverageRejected -or
        $missingCoverageRecords.Count -ne 2 -or
        $missingCoverageRecords[0].Status -ne 'start' -or
        $missingCoverageRecords[1].Status -ne 'fail') {
        throw 'Audit self-test failed: missing preinstalled coverage tool did not fail closed with the explicit preparation recovery.'
    }

    if (@(Get-MsrvLlvmPreparationArguments -Authorized $false -Toolchain '1.97.1').Count -ne 0 -or
        @(Get-CoverageToolPreparationArguments -Authorized $false -Version '0.8.7').Count -ne 0) {
        throw 'Audit self-test failed: default audit generated an unauthorized tool preparation command.'
    }
    $llvmPreparation = @(
        Get-MsrvLlvmPreparationArguments -Authorized $true -Toolchain '1.97.1'
    )
    $coveragePreparation = @(
        Get-CoverageToolPreparationArguments `
            -Authorized $true `
            -Version '0.8.7' `
            -Root 'persistent-cargo-root'
    )
    if (($llvmPreparation -join ' ') -ne 'component add llvm-tools-preview --toolchain 1.97.1' -or
        ($coveragePreparation -join ' ') -ne 'install cargo-llvm-cov --locked --version 0.8.7 --root persistent-cargo-root') {
        throw 'Audit self-test failed: explicit tool preparation command shape drifted.'
    }

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

    $msrvFixture = New-ManagedAuditDirectory -Label 'msrv-selftest'
    $msrvEnvironmentNames = @(
        'PATH',
        'CARGO',
        'RUSTC',
        'CARGO_BUILD_RUSTC',
        'CARGO_TARGET_DIR',
        'LLVM_COV',
        'LLVM_PROFDATA',
        'RUSTC_WRAPPER',
        'RUSTC_WORKSPACE_WRAPPER'
    )
    $msrvOuterEnvironment = Get-EnvironmentSnapshot -Names $msrvEnvironmentNames
    try {
        $fakeCargo = Join-Path $msrvFixture 'cargo-fixture'
        $fakeRustc = Join-Path $msrvFixture 'rustc-fixture'
        Set-Content -LiteralPath $fakeCargo -Value 'fixture cargo' -Encoding utf8
        Set-Content -LiteralPath $fakeRustc -Value 'fixture rustc' -Encoding utf8
        $fakeTargetLib = Join-Path $msrvFixture 'rustlib-target/lib'
        $fakeLlvmBin = Join-Path $msrvFixture 'rustlib-target/bin'
        New-Item -ItemType Directory -Path $fakeTargetLib,$fakeLlvmBin | Out-Null
        $llvmSuffix = if ($IsWindows) { '.exe' } else { '' }
        $fakeLlvmCov = Join-Path $fakeLlvmBin "llvm-cov$llvmSuffix"
        $fakeLlvmProfdata = Join-Path $fakeLlvmBin "llvm-profdata$llvmSuffix"
        Set-Content -LiteralPath $fakeLlvmCov -Value 'fixture llvm-cov' -Encoding utf8
        Set-Content -LiteralPath $fakeLlvmProfdata -Value 'fixture llvm-profdata' -Encoding utf8
        $rustupCalls = [Collections.Generic.List[string]]::new()
        $rustupInvoker = {
            param([string]$FilePath, [string[]]$Arguments)
            $rustupCalls.Add(($Arguments -join ' '))
            $resolvedPath = if ($Arguments[1] -eq 'cargo') { $fakeCargo } else { $fakeRustc }
            return [pscustomobject]@{
                ExitCode = 0
                Output = @($resolvedPath)
            }
        }.GetNewClosure()
        $msrvApplications = Resolve-MsrvToolchainApplications `
            -RustupIdentity ([pscustomobject]@{ Path = 'rustup-fixture' }) `
            -Toolchain '1.97.1' `
            -Invoker $rustupInvoker
        if ($rustupCalls.Count -ne 2 -or
            $rustupCalls[0] -ne 'which cargo --toolchain 1.97.1' -or
            $rustupCalls[1] -ne 'which rustc --toolchain 1.97.1' -or
            -not $msrvApplications.Cargo.Path.Equals((Resolve-Path -LiteralPath $fakeCargo).ProviderPath, $pathComparison) -or
            -not $msrvApplications.Rustc.Path.Equals((Resolve-Path -LiteralPath $fakeRustc).ProviderPath, $pathComparison)) {
            throw 'Audit self-test failed: MSRV cargo/rustc were not bound to the exact rustup which results.'
        }

        $llvmResolutionCalls = [Collections.Generic.List[string]]::new()
        $llvmInvoker = {
            param([string]$FilePath, [string[]]$Arguments)
            $llvmResolutionCalls.Add(($Arguments -join ' '))
            return [pscustomobject]@{
                ExitCode = 0
                Output = @($fakeTargetLib)
            }
        }.GetNewClosure()
        $msrvLlvmApplications = Resolve-MsrvLlvmApplications `
            -RustcIdentity $msrvApplications.Rustc `
            -Invoker $llvmInvoker
        if ($llvmResolutionCalls.Count -ne 1 -or
            $llvmResolutionCalls[0] -ne '--print target-libdir' -or
            -not $msrvLlvmApplications.LlvmCov.Path.Equals((Resolve-Path -LiteralPath $fakeLlvmCov).ProviderPath, $pathComparison) -or
            -not $msrvLlvmApplications.LlvmProfdata.Path.Equals((Resolve-Path -LiteralPath $fakeLlvmProfdata).ProviderPath, $pathComparison)) {
            throw 'Audit self-test failed: MSRV llvm-tools were not bound to the exact rustc target-libdir sibling bin.'
        }

        $env:RUSTC = 'ambient-rustc'
        Remove-Item Env:CARGO_BUILD_RUSTC -ErrorAction SilentlyContinue
        $env:CARGO_TARGET_DIR = 'ambient-target'
        $env:RUSTC_WRAPPER = 'ambient-wrapper'
        Remove-Item Env:RUSTC_WORKSPACE_WRAPPER -ErrorAction SilentlyContinue
        $createdTargets = [Collections.Generic.List[string]]::new()
        $commandCalls = [Collections.Generic.List[string]]::new()
        $msrvSelfTestPathComparison = $pathComparison
        $commandRunner = {
            param([string]$FilePath, [string[]]$Arguments)
            if ($createdTargets.Count -eq 0) {
                $createdTargets.Add($env:CARGO_TARGET_DIR)
            }
            if (-not [string]::Equals($FilePath, $msrvApplications.Cargo.Path, $msrvSelfTestPathComparison) -or
                $env:RUSTC -ne $msrvApplications.Rustc.Path -or
                $env:CARGO_BUILD_RUSTC -ne $msrvApplications.Rustc.Path -or
                $env:CARGO_TARGET_DIR -ne $createdTargets[0] -or
                (Test-Path Env:RUSTC_WRAPPER) -or
                (Test-Path Env:RUSTC_WORKSPACE_WRAPPER)) {
                throw 'MSRV self-test runner observed an unbound compiler, wrapper, or target directory.'
            }
            $commandCalls.Add(($Arguments -join ' '))
            if ($commandCalls.Count -eq 2) {
                throw 'intentional MSRV command failure'
            }
        }.GetNewClosure()

        $expectedFailureObserved = $false
        $msrvFailureMessage = '<none>'
        try {
            Invoke-IsolatedMsrvChecks `
                -CargoIdentity $msrvApplications.Cargo `
                -RustcIdentity $msrvApplications.Rustc `
                -SkippedIntegrationTest $script:AuditIntegrationTestName `
                -CommandRunner $commandRunner
        } catch {
            $msrvFailureMessage = $_.Exception.Message
            $expectedFailureObserved = $_.Exception.Message -match 'intentional MSRV command failure'
        }
        if (-not $expectedFailureObserved -or
            $commandCalls.Count -ne 2 -or
            $commandCalls[0] -ne 'build --locked --release -p rayman' -or
            $commandCalls[1] -ne "test --locked --workspace --all-targets -- --skip $script:AuditIntegrationTestName" -or
            $createdTargets.Count -ne 1 -or
            (Test-Path -LiteralPath $createdTargets[0]) -or
            $env:RUSTC -ne 'ambient-rustc' -or
            (Test-Path Env:CARGO_BUILD_RUSTC) -or
            $env:CARGO_TARGET_DIR -ne 'ambient-target' -or
            $env:RUSTC_WRAPPER -ne 'ambient-wrapper' -or
            (Test-Path Env:RUSTC_WORKSPACE_WRAPPER)) {
            $detail = @(
                "failure=$msrvFailureMessage",
                "commands=$($commandCalls -join '|')",
                "created=$($createdTargets.Count)",
                "target_exists=$(if ($createdTargets.Count -eq 1) { Test-Path -LiteralPath $createdTargets[0] } else { 'unknown' })",
                "rustc=$env:RUSTC",
                "cargo_build_rustc_present=$(Test-Path Env:CARGO_BUILD_RUSTC)",
                "target=$env:CARGO_TARGET_DIR",
                "wrapper=$env:RUSTC_WRAPPER",
                "workspace_wrapper_present=$(Test-Path Env:RUSTC_WORKSPACE_WRAPPER)"
            ) -join '; '
            throw "Audit self-test failed: isolated MSRV commands, cleanup, or environment restoration are incomplete: $detail"
        }

        $fakeCoverageBin = Join-Path $msrvFixture 'coverage-bin'
        New-Item -ItemType Directory -Path $fakeCoverageBin | Out-Null
        $coverageSuffix = if ($IsWindows) { '.exe' } else { '' }
        $fakeCoverage = Join-Path $fakeCoverageBin "cargo-llvm-cov$coverageSuffix"
        Set-Content -LiteralPath $fakeCoverage -Value 'fixture cargo-llvm-cov' -Encoding utf8
        $coverageIdentity = New-ExactApplicationIdentity `
            -Name 'cargo-llvm-cov' `
            -Label 'Coverage fixture' `
            -Path $fakeCoverage

        $env:PATH = 'ambient-path'
        $env:CARGO = 'ambient-cargo'
        $env:LLVM_COV = 'ambient-llvm-cov'
        $env:LLVM_PROFDATA = 'ambient-llvm-profdata'
        $coverageTargets = [Collections.Generic.List[string]]::new()
        $coverageCalls = [Collections.Generic.List[string]]::new()
        $coverageSelfTestPathComparison = $pathComparison
        $coverageRunner = {
            param([string]$FilePath, [string[]]$Arguments)
            if ($coverageTargets.Count -eq 0) {
                $coverageTargets.Add($env:CARGO_TARGET_DIR)
            }
            $pathParts = @($env:PATH -split [regex]::Escape([IO.Path]::PathSeparator))
            if (-not [string]::Equals($FilePath, $coverageIdentity.Path, $coverageSelfTestPathComparison) -or
                $pathParts.Count -lt 2 -or
                -not [string]::Equals($pathParts[0], $fakeCoverageBin, $coverageSelfTestPathComparison) -or
                -not [string]::Equals($pathParts[1], (Split-Path -Parent $msrvApplications.Cargo.Path), $coverageSelfTestPathComparison) -or
                $env:CARGO -ne $msrvApplications.Cargo.Path -or
                $env:RUSTC -ne $msrvApplications.Rustc.Path -or
                $env:CARGO_BUILD_RUSTC -ne $msrvApplications.Rustc.Path -or
                $env:LLVM_COV -ne $msrvLlvmApplications.LlvmCov.Path -or
                $env:LLVM_PROFDATA -ne $msrvLlvmApplications.LlvmProfdata.Path -or
                $env:CARGO_TARGET_DIR -ne $coverageTargets[0] -or
                (Test-Path Env:RUSTC_WRAPPER) -or
                (Test-Path Env:RUSTC_WORKSPACE_WRAPPER)) {
                throw 'Coverage self-test runner observed an unbound compiler, LLVM tool, wrapper, PATH, or target directory.'
            }
            $coverageCalls.Add(($Arguments -join ' '))
            throw 'intentional coverage command failure'
        }.GetNewClosure()

        $coverageFailureObserved = $false
        $coverageFailureMessage = '<none>'
        try {
            Invoke-IsolatedCoverageCheck `
                -CoverageIdentity $coverageIdentity `
                -CargoIdentity $msrvApplications.Cargo `
                -RustcIdentity $msrvApplications.Rustc `
                -LlvmCovIdentity $msrvLlvmApplications.LlvmCov `
                -LlvmProfdataIdentity $msrvLlvmApplications.LlvmProfdata `
                -MinimumLineCoverage 75 `
                -SkippedIntegrationTest $script:AuditIntegrationTestName `
                -CommandRunner $coverageRunner
        } catch {
            $coverageFailureMessage = $_.Exception.Message
            $coverageFailureObserved = $_.Exception.Message -match 'intentional coverage command failure'
        }
        if (-not $coverageFailureObserved -or
            $coverageCalls.Count -ne 1 -or
            $coverageCalls[0] -ne "llvm-cov --locked --workspace --all-features --all-targets --fail-under-lines 75 -- --skip $script:AuditIntegrationTestName" -or
            $coverageTargets.Count -ne 1 -or
            (Test-Path -LiteralPath $coverageTargets[0]) -or
            $env:PATH -ne 'ambient-path' -or
            $env:CARGO -ne 'ambient-cargo' -or
            $env:RUSTC -ne 'ambient-rustc' -or
            (Test-Path Env:CARGO_BUILD_RUSTC) -or
            $env:CARGO_TARGET_DIR -ne 'ambient-target' -or
            $env:LLVM_COV -ne 'ambient-llvm-cov' -or
            $env:LLVM_PROFDATA -ne 'ambient-llvm-profdata' -or
            $env:RUSTC_WRAPPER -ne 'ambient-wrapper' -or
            (Test-Path Env:RUSTC_WORKSPACE_WRAPPER)) {
            $detail = @(
                "failure=$coverageFailureMessage",
                "commands=$($coverageCalls -join '|')",
                "created=$($coverageTargets.Count)",
                "target_exists=$(if ($coverageTargets.Count -eq 1) { Test-Path -LiteralPath $coverageTargets[0] } else { 'unknown' })",
                "path=$env:PATH",
                "cargo=$env:CARGO",
                "rustc=$env:RUSTC",
                "llvm_cov=$env:LLVM_COV",
                "llvm_profdata=$env:LLVM_PROFDATA"
            ) -join '; '
            throw "Audit self-test failed: isolated coverage binding, cleanup, or environment restoration is incomplete: $detail"
        }
    } finally {
        Restore-EnvironmentSnapshot -Snapshot $msrvOuterEnvironment
        Remove-ManagedAuditDirectory -Path $msrvFixture
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

        $explicitSeedState = New-IsolatedCargoDenyState `
            -RootConfig $fixtureConfig `
            -EvalConfig $fixtureConfig `
            -DatabaseSeedPath $fixtureDatabase
        try {
            if (-not (Test-Path -LiteralPath (Join-Path $explicitSeedState.Database 'advisory-db/HEAD') -PathType Leaf) -or
                (Get-Content -LiteralPath $explicitSeedState.RootConfig -Raw) -notmatch '(?m)^db-path\s*=' -or
                (Get-Content -LiteralPath $explicitSeedState.EvalConfig -Raw) -notmatch '(?m)^db-path\s*=') {
                throw 'Audit self-test failed: explicit cargo-deny database seed was not copied and bound to both configs.'
            }
        } finally {
            Remove-ManagedAuditDirectory -Path $explicitSeedState.Root
        }

        $missingExplicitSeedRejected = $false
        try {
            $null = New-IsolatedCargoDenyState `
                -RootConfig $fixtureConfig `
                -EvalConfig $fixtureConfig `
                -DatabaseSeedPath (Join-Path $cargoDenyFixture 'missing-seed')
        } catch {
            $missingExplicitSeedRejected = $_.Exception.Message -match 'Explicit cargo-deny advisory database seed is missing'
        }
        if (-not $missingExplicitSeedRejected) {
            throw 'Audit self-test failed: a missing explicit cargo-deny database seed was accepted.'
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

    foreach ($identity in $NativeIdentities) {
        Assert-NativeApplicationIdentity -Identity $identity
    }
    Write-Host 'Audit script self-test passed: native shadow/identity, explicit preparation authority, exact isolated MSRV, isolated advisory state, and managed coverage guards fail closed.'
}

$nativeApplications = $null
$msrvApplications = $null
if ($PSCmdlet.ParameterSetName -eq 'PrepareAuditTools') {
    Write-AuditPhase -Name 'prepare_audit_tools' -Status 'start'
    try {
        # This parameter set is deliberately a provisioning command, not an
        # audit modifier. It needs no installed CLI/SKILL identity, runs no
        # repository lane, persists the pinned tools, verifies their exact
        # effective paths/versions/hashes, and exits.
        $cargoIdentity = Resolve-NativeApplication -Name 'cargo' -Label 'Cargo'
        $rustupIdentity = Resolve-NativeApplication -Name 'rustup' -Label 'rustup'
        Invoke-NativeChecked `
            -FilePath $rustupIdentity.Path `
            -Arguments (Get-MsrvLlvmPreparationArguments -Authorized $true -Toolchain $MsrvToolchain)

        $preparedMsrv = Resolve-MsrvToolchainApplications `
            -RustupIdentity $rustupIdentity `
            -Toolchain $MsrvToolchain
        Assert-RustToolVersion -Identity $preparedMsrv.Cargo -ExpectedVersion $MsrvToolchain
        Assert-RustToolVersion -Identity $preparedMsrv.Rustc -ExpectedVersion $MsrvToolchain
        $preparedLlvm = Resolve-MsrvLlvmApplications -RustcIdentity $preparedMsrv.Rustc

        $cargoInstallRoot = Resolve-PersistentCargoInstallRoot
        Invoke-NativeChecked `
            -FilePath $cargoIdentity.Path `
            -Arguments (Get-CoverageToolPreparationArguments `
                -Authorized $true `
                -Version $CoverageToolVersion `
                -Root $cargoInstallRoot)
        $coverageIdentity = Resolve-NativeApplication `
            -Name 'cargo-llvm-cov' `
            -Label 'cargo-llvm-cov'
        $coverageApplicationName = if ($IsWindows) { 'cargo-llvm-cov.exe' } else { 'cargo-llvm-cov' }
        Assert-ExpectedApplicationPath `
            -Identity $coverageIdentity `
            -ExpectedPath (Join-Path (Join-Path $cargoInstallRoot 'bin') $coverageApplicationName)
        Get-CoverageToolVersion `
            -Identity $coverageIdentity `
            -ExpectedVersion $CoverageToolVersion

        Assert-NativeApplicationIdentity -Identity $cargoIdentity
        Assert-NativeApplicationIdentity -Identity $rustupIdentity
        Assert-NativeApplicationIdentity -Identity $coverageIdentity
        $terminalMsrv = Resolve-MsrvToolchainApplications `
            -RustupIdentity $rustupIdentity `
            -Toolchain $MsrvToolchain
        foreach ($name in @('Cargo', 'Rustc')) {
            $initial = $preparedMsrv.$name
            $terminal = $terminalMsrv.$name
            if (-not $terminal.Path.Equals($initial.Path, $pathComparison) -or
                $terminal.Sha256 -ne $initial.Sha256) {
                throw "$($initial.Label) binding changed during audit-tool preparation."
            }
        }
        $terminalLlvm = Resolve-MsrvLlvmApplications -RustcIdentity $terminalMsrv.Rustc
        foreach ($name in @('LlvmCov', 'LlvmProfdata')) {
            $initial = $preparedLlvm.$name
            $terminal = $terminalLlvm.$name
            if (-not $terminal.Path.Equals($initial.Path, $pathComparison) -or
                $terminal.Sha256 -ne $initial.Sha256) {
                throw "$($initial.Label) binding changed during audit-tool preparation."
            }
        }
        $report = [ordered]@{
            schema = 'rayman.audit.tool-preparation.v1'
            status = 'pass'
            msrv_toolchain = $MsrvToolchain
            cargo_install_root = $cargoInstallRoot
            cargo_llvm_cov = [ordered]@{
                version = $CoverageToolVersion
                path = $coverageIdentity.Path
                sha256 = $coverageIdentity.Sha256
            }
            llvm_cov = [ordered]@{
                path = $preparedLlvm.LlvmCov.Path
                sha256 = $preparedLlvm.LlvmCov.Sha256
            }
            llvm_profdata = [ordered]@{
                path = $preparedLlvm.LlvmProfdata.Path
                sha256 = $preparedLlvm.LlvmProfdata.Sha256
            }
        }
        Write-Output ('RAYMAN_AUDIT_TOOL_PREPARATION ' + ($report | ConvertTo-Json -Depth 5 -Compress))
        Write-AuditPhase -Name 'prepare_audit_tools' -Status 'pass'
        return
    } catch {
        Write-AuditPhase -Name 'prepare_audit_tools' -Status 'fail' -Detail $_.Exception.Message
        throw
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
# Self-test and the focused dependency-policy lane intentionally avoid the
# complete-audit MSRV/Git/compiler resolver. A complete audit still requires
# all five applications, but a missing one now has a structured bootstrap fail.
Invoke-AuditBootstrap `
    -IncludeCompleteAuditTools ($PSCmdlet.ParameterSetName -eq 'Audit') `
    -IncludePreinstalledCoverageTool ($PSCmdlet.ParameterSetName -eq 'Audit') `
    -Applications ([ref]$nativeApplications)
$capturedNativeIdentities = @($nativeApplications.Values)
$coverageIdentity = $null
if ($PSCmdlet.ParameterSetName -eq 'Audit') {
    Write-AuditPhase -Name 'coverage_tool_preflight' -Status 'start'
    try {
        $coverageIdentity = $nativeApplications.CargoLlvmCov
        Get-CoverageToolVersion `
            -Identity $coverageIdentity `
            -ExpectedVersion $CoverageToolVersion
        Write-AuditPhase -Name 'coverage_tool_preflight' -Status 'pass'
    } catch {
        Write-AuditPhase `
            -Name 'coverage_tool_preflight' `
            -Status 'fail' `
            -Detail $_.Exception.Message
        throw
    }
}
Write-AuditPhase -Name 'script_self_test' -Status 'start'
Invoke-AuditScriptSelfTest -NativeIdentities $capturedNativeIdentities
Write-AuditPhase -Name 'script_self_test' -Status 'pass'
if ($PSCmdlet.ParameterSetName -eq 'SelfTest') {
    Write-Host 'audit-repository.ps1 self-test passed.'
    return
}
if ($PSCmdlet.ParameterSetName -eq 'Audit') {
    Write-AuditPhase -Name 'release_script_self_tests' -Status 'start'
    try {
        foreach ($scriptName in @(
            'check-update-freshness.ps1',
            'release-closeout.ps1',
            'install-rayman.ps1',
            'verify-release-contract.ps1',
            'repair-rayman-powershell-profile.ps1'
        )) {
            $scriptPath = Join-Path $PSScriptRoot $scriptName
            if (-not (Test-Path -LiteralPath $scriptPath -PathType Leaf)) {
                throw "Audit cannot find sibling self-test script: $scriptPath"
            }
            & $scriptPath -SelfTest
        }
        Write-AuditPhase -Name 'release_script_self_tests' -Status 'pass'
    } catch {
        Write-AuditPhase `
            -Name 'release_script_self_tests' `
            -Status 'fail' `
            -Detail $_.Exception.Message
        throw
    }
}
if ($PSCmdlet.ParameterSetName -eq 'DependencyPolicy') {
    Push-Location $repoRoot
    try {
        Write-AuditPhase -Name 'dependency_policy' -Status 'start'
        Invoke-IsolatedCargoDenyChecks `
            -CargoDenyIdentity $nativeApplications.CargoDeny `
            -DatabaseSeedPath $CargoDenyDatabaseSeedPath
        Write-AuditPhase -Name 'dependency_policy' -Status 'pass'
    } finally {
        Pop-Location
    }
    Write-Host 'Isolated root/evals dependency policy checks passed.'
    return
}
Push-Location $repoRoot
try {
    # Fail fast on environment and authorization boundaries before the
    # multi-minute lanes. A normal audit validates preinstalled host tools and
    # never changes the rustup component set or installs cargo-llvm-cov.
    Write-AuditPhase -Name 'environment_preflight' -Status 'start'
    # Keep the rejection policy in verify-release-contract.ps1. The audit only
    # invokes its read-only inspector, so it cannot drift to a smaller local
    # list of build-shaping environment variables.
    $sourceFreshInputInspection = Invoke-SourceFreshInputInspection
    Invoke-IsolatedCargoDenyChecks `
        -CargoDenyIdentity $nativeApplications.CargoDeny `
        -DatabaseSeedPath $CargoDenyDatabaseSeedPath
    $msrvApplications = Resolve-MsrvToolchainApplications `
        -RustupIdentity $nativeApplications.Rustup `
        -Toolchain $MsrvToolchain
    Assert-RustToolVersion -Identity $msrvApplications.Cargo -ExpectedVersion $MsrvToolchain
    Assert-RustToolVersion -Identity $msrvApplications.Rustc -ExpectedVersion $MsrvToolchain
    try {
        $msrvLlvmApplications = Resolve-MsrvLlvmApplications `
            -RustcIdentity $msrvApplications.Rustc
    } catch {
        throw "The MSRV $MsrvToolchain llvm-tools-preview component is unavailable. Run 'pwsh -NoProfile -File scripts/audit-repository.ps1 -PrepareAuditTools' as a separate explicit provisioning step, then rerun the audit. $($_.Exception.Message)"
    }
    Write-AuditPhase -Name 'environment_preflight' -Status 'pass'

    Write-AuditPhase -Name 'root_quality' -Status 'start'
    foreach ($qualityCommand in @(Get-RepositoryQualityCommands -Suite Root)) {
        $qualityArguments = @($qualityCommand.argv)
        if ($qualityCommand.name -eq 'test') {
            $qualityArguments += @('--', '--skip', $script:AuditIntegrationTestName)
        }
        Invoke-NativeChecked `
            -FilePath $nativeApplications.Cargo.Path `
            -Arguments $qualityArguments
    }
    Write-AuditPhase -Name 'root_quality' -Status 'pass'

    # MSRV is mandatory. Bind both cargo and rustc to rustup's exact toolchain
    # paths and isolate its target directory so ambient PATH precedence and
    # compiler-incompatible metadata from another lane cannot affect this one.
    Write-AuditPhase -Name 'msrv' -Status 'start'
    Invoke-IsolatedMsrvChecks `
        -CargoIdentity $msrvApplications.Cargo `
        -RustcIdentity $msrvApplications.Rustc `
        -SkippedIntegrationTest $script:AuditIntegrationTestName
    Write-AuditPhase -Name 'msrv' -Status 'pass'

    # Real shipped-CLI coverage uses the exact preinstalled PATH Application
    # captured and version-checked before any long lane. Provisioning is a
    # separate explicit command and never changes this audit invocation.
    Write-AuditPhase -Name 'cli_coverage' -Status 'start'
    Invoke-IsolatedCoverageCheck `
        -CoverageIdentity $coverageIdentity `
        -CargoIdentity $msrvApplications.Cargo `
        -RustcIdentity $msrvApplications.Rustc `
        -LlvmCovIdentity $msrvLlvmApplications.LlvmCov `
        -LlvmProfdataIdentity $msrvLlvmApplications.LlvmProfdata `
        -MinimumLineCoverage $MinimumCliLineCoverage `
        -SkippedIntegrationTest $script:AuditIntegrationTestName
    Write-AuditPhase -Name 'cli_coverage' -Status 'pass'

    Write-AuditPhase -Name 'evals' -Status 'start'
    foreach ($qualityCommand in @(Get-RepositoryQualityCommands -Suite Evals)) {
        Invoke-NativeChecked `
            -FilePath $nativeApplications.Cargo.Path `
            -Arguments $qualityCommand.argv
    }

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
    Write-AuditPhase -Name 'evals' -Status 'pass'
    Write-AuditPhase -Name 'package_install_smoke' -Status 'start'

    # Packaging and cargo-install are separate from a workspace build. Exercise
    # both so missing package metadata/files cannot survive until release day.
    Invoke-NativeChecked $nativeApplications.Cargo.Path @('package', '--locked', '-p', 'rayman')
    # Anchor on $repoRoot like every sibling helper. [IO.Path]::GetFullPath
    # resolves a relative path against [Environment]::CurrentDirectory, which
    # Push-Location/Set-Location never update, so a pwsh started outside the repo
    # built the smoke root under that unrelated directory and the containment
    # check below aborted the audit ~30 minutes in.
    $smokeRoot = [IO.Path]::GetFullPath(
        (Join-Path $repoRoot (Join-Path '.RaymanCodingSkill/tmp' ("audit-install-$PID-" + [Guid]::NewGuid().ToString('N'))))
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
    Write-AuditPhase -Name 'package_install_smoke' -Status 'pass'
    Write-AuditPhase -Name 'workspace_self_dogfood' -Status 'start'

    Invoke-NativeChecked $nativeApplications.Cargo.Path @('build', '--locked', '--release', '-p', 'rayman')
    # An ambient CARGO_TARGET_DIR sends cargo's output elsewhere, so a hardcoded
    # target/release either fails to resolve or silently certifies a stale
    # artifact. `[IO.Path]::Combine` (unlike Join-Path) lets an absolute value win.
    $releaseRoot = if ($env:CARGO_TARGET_DIR) {
        [IO.Path]::GetFullPath([IO.Path]::Combine($repoRoot, $env:CARGO_TARGET_DIR))
    } else {
        Join-Path $repoRoot 'target'
    }
    $referenceArtifact = (Resolve-Path -LiteralPath (Join-Path $releaseRoot "release/$artifactName")).ProviderPath
    $resolvedCli = (Resolve-Path -LiteralPath $CliPath).ProviderPath
    $resolvedSkill = (Resolve-Path -LiteralPath $SkillPath).ProviderPath

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
    Write-AuditPhase -Name 'workspace_self_dogfood' -Status 'pass'
    Write-AuditPhase -Name 'installed_release_identity' -Status 'start'

    & (Join-Path $PSScriptRoot 'verify-release-contract.ps1') `
        -CliPath $resolvedCli `
        -ReferenceCliPath $referenceArtifact `
        -SkillPath $resolvedSkill `
        -WorkspaceSkillPath (Join-Path $repoRoot 'SKILL.md') `
        -RequirePath `
        -RequireSourceFresh

    $terminalSourceFreshInputInspection = Invoke-SourceFreshInputInspection
    if (($sourceFreshInputInspection | ConvertTo-Json -Depth 20 -Compress) -cne
        ($terminalSourceFreshInputInspection | ConvertTo-Json -Depth 20 -Compress)) {
        throw 'Workspace activation or source-fresh environment policy drifted during repository audit.'
    }

    foreach ($identity in $capturedNativeIdentities) {
        Assert-NativeApplicationIdentity -Identity $identity
    }
    Assert-ExactApplicationIdentity -Identity $coverageIdentity
    $terminalMsrvApplications = Resolve-MsrvToolchainApplications `
        -RustupIdentity $nativeApplications.Rustup `
        -Toolchain $MsrvToolchain
    foreach ($name in @('Cargo', 'Rustc')) {
        $initial = $msrvApplications.$name
        $terminal = $terminalMsrvApplications.$name
        if (-not $terminal.Path.Equals($initial.Path, $pathComparison) -or
            $terminal.Sha256 -ne $initial.Sha256) {
            throw "$($initial.Label) rustup binding changed during audit: $($initial.Path) [$($initial.Sha256)] -> $($terminal.Path) [$($terminal.Sha256)]"
        }
    }
    $terminalMsrvLlvmApplications = Resolve-MsrvLlvmApplications `
        -RustcIdentity $terminalMsrvApplications.Rustc
    foreach ($name in @('LlvmCov', 'LlvmProfdata')) {
        $initial = $msrvLlvmApplications.$name
        $terminal = $terminalMsrvLlvmApplications.$name
        if (-not $terminal.Path.Equals($initial.Path, $pathComparison) -or
            $terminal.Sha256 -ne $initial.Sha256) {
            throw "$($initial.Label) MSRV LLVM binding changed during audit: $($initial.Path) [$($initial.Sha256)] -> $($terminal.Path) [$($terminal.Sha256)]"
        }
    }
    Write-AuditPhase -Name 'installed_release_identity' -Status 'pass'
} catch {
    Write-AuditPhase -Name $script:CurrentAuditPhase -Status 'fail' -Detail $_.Exception.Message
    throw
} finally {
    Pop-Location
}

Write-Host 'Complete repository audit passed: root/MSRV/CLI coverage, evals safety+mock provenance, package/install, strict workspace, checkpoint and state gates plus a report-only assets scan, and installed release identity.'
Write-AuditPhase -Name 'complete' -Status 'pass'
