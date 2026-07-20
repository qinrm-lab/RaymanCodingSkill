[CmdletBinding(DefaultParameterSetName = 'Verify')]
param(
    [Parameter(Mandatory = $true, ParameterSetName = 'Verify')]
    [ValidateNotNullOrEmpty()]
    [string]$CliPath,

    [Parameter(ParameterSetName = 'Verify')]
    [ValidateNotNullOrEmpty()]
    [string]$ReferenceCliPath,

    [Parameter(Mandatory = $true, ParameterSetName = 'Verify')]
    [ValidateNotNullOrEmpty()]
    [string]$SkillPath,

    [Parameter(ParameterSetName = 'Verify')]
    [switch]$RequirePath,

    [Parameter(ParameterSetName = 'Verify')]
    [switch]$VerifyGitTag,

    # Rebuild rayman from the current clean checkout in an isolated target directory and
    # require byte identity with -CliPath (and -ReferenceCliPath when supplied).
    [Parameter(ParameterSetName = 'Verify')]
    [switch]$RequireSourceFresh,

    [Parameter(Mandatory = $true, ParameterSetName = 'SelfTest')]
    [switch]$SelfTest
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if ($PSVersionTable.PSVersion.Major -lt 7) {
    throw 'verify-release-contract.ps1 requires PowerShell 7+. Run it with pwsh, not Windows PowerShell.'
}

$repoRoot = Split-Path -Parent $PSScriptRoot
$workspaceManifest = Join-Path $repoRoot 'Cargo.toml'
$crateManifest = Join-Path $repoRoot 'crates/rayman/Cargo.toml'
$lockfile = Join-Path $repoRoot 'Cargo.lock'
$canonicalSkill = Join-Path $repoRoot 'SKILL.md'
$expectedContract = 'rayman-cli-contract-v6'
$requiredMsrv = '1.88'

function Read-RequiredFile {
    param([string]$Path, [string]$Label)

    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        throw "$Label is missing: $Path"
    }
    return Get-Content -LiteralPath $Path -Raw
}

function Get-ManifestValue {
    param([string]$Content, [string]$Key, [string]$Label)

    $pattern = '(?m)^' + [regex]::Escape($Key) + '\s*=\s*"([^"]+)"\s*$'
    $match = [regex]::Match($Content, $pattern)
    if (-not $match.Success) {
        throw "Missing $Key in $Label"
    }
    return $match.Groups[1].Value
}

function Resolve-RequiredPath {
    param([string]$Path, [string]$Label)

    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        throw "$Label is missing: $Path"
    }
    return (Resolve-Path -LiteralPath $Path).Path
}

function Get-Sha256 {
    param([string]$Path)

    return (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}

function Resolve-RequiredApplication {
    param([string]$Name, [string]$Label)

    $commands = @(Get-Command $Name -All -ErrorAction Stop)
    if ($commands.Count -eq 0 -or $commands[0].CommandType -ne 'Application') {
        $kind = if ($commands.Count -eq 0) { 'missing' } else { $commands[0].CommandType }
        throw "$Label must resolve directly to an Application; effective command is $kind."
    }
    return Resolve-RequiredPath -Path $commands[0].Source -Label $Label
}

function Resolve-EffectiveRaymanApplication {
    return Resolve-RequiredApplication -Name 'rayman' -Label 'PATH rayman executable'
}

function Test-WindowsMsvcHost {
    if (-not $IsWindows) {
        return $false
    }
    $version = (& $script:rustcApplication -vV 2>$null | Out-String).Trim()
    if ($LASTEXITCODE -ne 0) {
        throw 'Unable to inspect the Rust host for reproducible release-link verification.'
    }
    return $version -match '(?m)^host:\s+.+-pc-windows-msvc\s*$'
}

function Get-RustcIdentity {
    $identity = (& $script:rustcApplication -vV 2>$null | Out-String).Trim()
    if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($identity)) {
        throw 'Unable to inspect the active Rust compiler identity.'
    }
    return $identity
}

function Assert-NoBuildShapingEnvironment {
    # These variables can change generated code, native dependencies, target selection,
    # linker behavior, or release-profile output without changing the checked-in source.
    # A byte-identical rebuild under such ambient overrides is not a repository-default
    # active-build-context source identity claim, so refuse it instead of silently
    # inheriting the overrides. User/parent Cargo config remains a documented input.
    $exactNames = @(
        'AR',
        'CC',
        'CFLAGS',
        'CXX',
        'CXXFLAGS',
        'LDFLAGS',
        'SOURCE_DATE_EPOCH',
        'RUSTC',
        'RUSTC_WRAPPER',
        'RUSTC_WORKSPACE_WRAPPER',
        'RUSTFLAGS',
        'CARGO_ENCODED_RUSTFLAGS',
        'CARGO_BUILD_RUSTC',
        'CARGO_BUILD_RUSTFLAGS',
        'CARGO_BUILD_TARGET',
        'CARGO_INCREMENTAL'
    )
    $found = @(
        Get-ChildItem Env: | Where-Object {
            -not [string]::IsNullOrWhiteSpace($_.Value) -and (
                $exactNames -contains $_.Name -or
                $_.Name -match '^CARGO_PROFILE_' -or
                $_.Name -match '^CARGO_TARGET_.+_(AR|CC|CFLAGS|CXX|CXXFLAGS|LINKER|RUNNER|RUSTFLAGS)$' -or
                $_.Name -match '^(AR|CC|CFLAGS|CXX|CXXFLAGS|LDFLAGS)_.+$' -or
                $_.Name -match '^(HOST|TARGET)_(AR|CC|CFLAGS|CXX|CXXFLAGS|LDFLAGS)$'
            )
        } | Sort-Object Name | ForEach-Object Name
    )
    if ($found.Count -gt 0) {
        throw "Source-fresh verification refuses ambient build-shaping environment variables: $($found -join ', '). Clear them and rebuild the supplied artifact."
    }
}

function Assert-CleanGitSource {
    $insideWorkTree = (& $script:gitApplication -C $repoRoot rev-parse --is-inside-work-tree 2>$null | Out-String).Trim()
    if ($LASTEXITCODE -ne 0 -or $insideWorkTree -ne 'true') {
        throw "Source-fresh verification requires a Git worktree at $repoRoot."
    }
    $status = @(& $script:gitApplication -C $repoRoot status --porcelain --untracked-files=all 2>$null)
    if ($LASTEXITCODE -ne 0) {
        throw 'Unable to inspect Git worktree status for source-fresh verification.'
    }
    $statusText = ($status | Out-String).Trim()
    if ($statusText) {
        throw "Source-fresh verification requires a clean Git worktree; found:`n$statusText"
    }
    $head = (& $script:gitApplication -C $repoRoot rev-parse --verify HEAD 2>$null | Out-String).Trim()
    if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($head)) {
        throw 'Unable to resolve the current Git HEAD for source-fresh verification.'
    }
    return $head
}

function Remove-SourceFreshBuild {
    param([string]$TargetDir)

    if (-not (Test-Path -LiteralPath $TargetDir)) {
        return
    }
    $tempRoot = (Resolve-Path -LiteralPath ([IO.Path]::GetTempPath())).Path
    $fullTarget = [IO.Path]::GetFullPath($TargetDir)
    $separator = [IO.Path]::DirectorySeparatorChar
    if (-not $tempRoot.EndsWith([string]$separator)) {
        $tempRoot = "$tempRoot$separator"
    }
    $comparison = if ($IsWindows) {
        [StringComparison]::OrdinalIgnoreCase
    } else {
        [StringComparison]::Ordinal
    }
    if (-not $fullTarget.StartsWith($tempRoot, $comparison)) {
        throw "Refusing to remove source-fresh target outside the system temp root: $fullTarget"
    }
    $item = Get-Item -LiteralPath $fullTarget -Force
    if (-not $item.PSIsContainer -or $item.Attributes -band [IO.FileAttributes]::ReparsePoint) {
        throw "Refusing to remove non-directory or reparse-point source-fresh target: $fullTarget"
    }
    $resolvedTarget = (Resolve-Path -LiteralPath $fullTarget).Path
    if (-not $resolvedTarget.StartsWith($tempRoot, $comparison)) {
        throw "Refusing to remove source-fresh target resolving outside the system temp root: $resolvedTarget"
    }
    # Recheck immediately before the recursive delete. PowerShell cannot make this operation
    # race-free against a hostile same-user process, so any type/reparse anomaly fails closed.
    $item = Get-Item -LiteralPath $fullTarget -Force
    if (-not $item.PSIsContainer -or $item.Attributes -band [IO.FileAttributes]::ReparsePoint) {
        throw "Refusing to remove changed source-fresh target: $fullTarget"
    }
    Remove-Item -LiteralPath $fullTarget -Recurse -Force
}

function Build-SourceFreshArtifact {
    $targetDir = Join-Path ([IO.Path]::GetTempPath()) ("rayman-release-contract-$PID-" + [Guid]::NewGuid().ToString('N'))
    $oldTargetDir = $env:CARGO_TARGET_DIR
    $oldRustFlags = $env:RUSTFLAGS
    $oldEncodedRustFlags = $env:CARGO_ENCODED_RUSTFLAGS
    try {
        Assert-NoBuildShapingEnvironment
        $env:CARGO_TARGET_DIR = $targetDir
        # Cargo config enables /Brepro for ordinary Windows MSVC builds. Force the
        # same linker contract here. Ambient build-shaping flags were rejected above,
        # so this exact repository-required flag cannot merge with hidden caller input.
        if (Test-WindowsMsvcHost) {
            $brepro = @('-C', 'link-arg=/Brepro') -join [char]0x1f
            $env:CARGO_ENCODED_RUSTFLAGS = $brepro
            Remove-Item Env:RUSTFLAGS -ErrorAction SilentlyContinue
        }
        $pushedLocation = $false
        try {
            Push-Location $repoRoot
            $pushedLocation = $true
            $buildOutput = & $script:cargoApplication build --locked --release -p rayman 2>&1
            if ($LASTEXITCODE -ne 0) {
                $buildText = ($buildOutput | Out-String).Trim()
                throw "Locked source-fresh build failed with exit code ${LASTEXITCODE}:`n$buildText"
            }
        } finally {
            if ($pushedLocation) {
                Pop-Location
            }
        }
        $artifactName = if ($IsWindows) { 'rayman.exe' } else { 'rayman' }
        $artifact = Join-Path (Join-Path $targetDir 'release') $artifactName
        return [pscustomobject]@{
            TargetDir = $targetDir
            Artifact = Resolve-RequiredPath -Path $artifact -Label 'source-fresh release artifact'
        }
    } catch {
        Remove-SourceFreshBuild -TargetDir $targetDir
        throw
    } finally {
        if ($null -eq $oldTargetDir) {
            Remove-Item Env:CARGO_TARGET_DIR -ErrorAction SilentlyContinue
        } else {
            $env:CARGO_TARGET_DIR = $oldTargetDir
        }
        if ($null -eq $oldRustFlags) {
            Remove-Item Env:RUSTFLAGS -ErrorAction SilentlyContinue
        } else {
            $env:RUSTFLAGS = $oldRustFlags
        }
        if ($null -eq $oldEncodedRustFlags) {
            Remove-Item Env:CARGO_ENCODED_RUSTFLAGS -ErrorAction SilentlyContinue
        } else {
            $env:CARGO_ENCODED_RUSTFLAGS = $oldEncodedRustFlags
        }
    }
}

function Invoke-Rayman {
    param([string[]]$Arguments)

    $output = & $script:resolvedCli @Arguments 2>&1
    $exitCode = $LASTEXITCODE
    $text = ($output | Out-String).Trim()
    if ($exitCode -ne 0) {
        throw "rayman $($Arguments -join ' ') failed with exit code ${exitCode}:`n$text"
    }
    return $text
}

function Assert-GitHubTagContext {
    param(
        [Parameter(Mandatory = $true)]
        [string]$SourceHead,
        [Parameter(Mandatory = $true)]
        [string]$ExactHeadTag
    )

    $contextNames = @(
        'GITHUB_ACTIONS',
        'GITHUB_REF_TYPE',
        'GITHUB_REF_NAME',
        'GITHUB_REF',
        'GITHUB_SHA'
    )
    $contextPresent = $false
    foreach ($name in $contextNames) {
        if (-not [string]::IsNullOrWhiteSpace([Environment]::GetEnvironmentVariable($name, 'Process'))) {
            $contextPresent = $true
            break
        }
    }
    if (-not $contextPresent) {
        return
    }

    if ($env:GITHUB_REF_TYPE -ne 'tag') {
        throw "GitHub release context must report GITHUB_REF_TYPE=tag; found '$($env:GITHUB_REF_TYPE)'."
    }
    if ($env:GITHUB_REF_NAME -ne $ExactHeadTag) {
        throw "GitHub tag name '$($env:GITHUB_REF_NAME)' does not match the exact tag on HEAD '$ExactHeadTag'."
    }
    $expectedRef = "refs/tags/$ExactHeadTag"
    if ($env:GITHUB_REF -ne $expectedRef) {
        throw "GitHub ref '$($env:GITHUB_REF)' does not match '$expectedRef'."
    }
    if (-not $SourceHead.Equals($env:GITHUB_SHA, [StringComparison]::OrdinalIgnoreCase)) {
        throw "GitHub SHA '$($env:GITHUB_SHA)' does not match the verified Git HEAD '$SourceHead'."
    }
}

function Invoke-ReleaseVerifierSelfTest {
    foreach ($name in @('cargo', 'git', 'rustc')) {
        Set-Item -LiteralPath "Function:$name" -Value { 'forged command' }
        try {
            $shadowRejected = $false
            try {
                $null = Resolve-RequiredApplication -Name $name -Label "$name self-test executable"
            } catch {
                $shadowRejected = $_.Exception.Message -match 'must resolve directly to an Application'
            }
            if (-not $shadowRejected) {
                throw "Release verifier self-test failed: a Function-shadowed $name command was not rejected."
            }
        } finally {
            Remove-Item -LiteralPath "Function:$name" -Force -ErrorAction SilentlyContinue
        }
    }

    $contextNames = @(
        'GITHUB_ACTIONS',
        'GITHUB_REF_TYPE',
        'GITHUB_REF_NAME',
        'GITHUB_REF',
        'GITHUB_SHA'
    )
    $savedContext = @{}
    foreach ($name in $contextNames) {
        $savedContext[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
    }
    $sourceHead = 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'
    $exactTag = 'v2.2.0'
    try {
        [Environment]::SetEnvironmentVariable('GITHUB_ACTIONS', 'true', 'Process')
        $forgedCases = @(
            @{
                GITHUB_REF_TYPE = 'branch'
                GITHUB_REF_NAME = $exactTag
                GITHUB_REF = "refs/tags/$exactTag"
                GITHUB_SHA = $sourceHead
            },
            @{
                GITHUB_REF_TYPE = 'tag'
                GITHUB_REF_NAME = 'v9.9.9'
                GITHUB_REF = 'refs/tags/v9.9.9'
                GITHUB_SHA = $sourceHead
            },
            @{
                GITHUB_REF_TYPE = 'tag'
                GITHUB_REF_NAME = $exactTag
                GITHUB_REF = "refs/tags/$exactTag"
                GITHUB_SHA = 'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb'
            }
        )
        foreach ($case in $forgedCases) {
            foreach ($name in @('GITHUB_REF_TYPE', 'GITHUB_REF_NAME', 'GITHUB_REF', 'GITHUB_SHA')) {
                [Environment]::SetEnvironmentVariable($name, $case[$name], 'Process')
            }
            $forgeryRejected = $false
            try {
                Assert-GitHubTagContext -SourceHead $sourceHead -ExactHeadTag $exactTag
            } catch {
                $forgeryRejected = $true
            }
            if (-not $forgeryRejected) {
                throw 'Release verifier self-test failed: forged GitHub tag context was not rejected.'
            }
        }

        [Environment]::SetEnvironmentVariable('GITHUB_REF_TYPE', 'tag', 'Process')
        [Environment]::SetEnvironmentVariable('GITHUB_REF_NAME', $exactTag, 'Process')
        [Environment]::SetEnvironmentVariable('GITHUB_REF', "refs/tags/$exactTag", 'Process')
        [Environment]::SetEnvironmentVariable('GITHUB_SHA', $sourceHead, 'Process')
        Assert-GitHubTagContext -SourceHead $sourceHead -ExactHeadTag $exactTag
    } finally {
        foreach ($name in $contextNames) {
            [Environment]::SetEnvironmentVariable($name, $savedContext[$name], 'Process')
        }
    }

    Write-Host 'Release verifier self-test passed: native command shadows and forged GitHub tag context were rejected.'
}

if ($SelfTest) {
    Invoke-ReleaseVerifierSelfTest
    return
}

$toolIdentity = @{}
if ($RequireSourceFresh) {
    $script:cargoApplication = Resolve-RequiredApplication -Name 'cargo' -Label 'Cargo executable'
    $script:rustcApplication = Resolve-RequiredApplication -Name 'rustc' -Label 'Rust compiler executable'
    $toolIdentity['cargo'] = @{ Path = $script:cargoApplication; Hash = Get-Sha256 $script:cargoApplication }
    $toolIdentity['rustc'] = @{ Path = $script:rustcApplication; Hash = Get-Sha256 $script:rustcApplication }
}
if ($RequireSourceFresh -or $VerifyGitTag) {
    $script:gitApplication = Resolve-RequiredApplication -Name 'git' -Label 'Git executable'
    $toolIdentity['git'] = @{ Path = $script:gitApplication; Hash = Get-Sha256 $script:gitApplication }
}

$workspaceContent = Read-RequiredFile -Path $workspaceManifest -Label 'Workspace manifest'
$crateContent = Read-RequiredFile -Path $crateManifest -Label 'Crate manifest'
$lockContent = Read-RequiredFile -Path $lockfile -Label 'Lockfile'
$expectedVersion = Get-ManifestValue -Content $workspaceContent -Key 'version' -Label $workspaceManifest
$expectedMsrv = Get-ManifestValue -Content $workspaceContent -Key 'rust-version' -Label $workspaceManifest

if ($expectedMsrv -ne $requiredMsrv) {
    throw "Workspace MSRV is $expectedMsrv, but this release contract and CI require $requiredMsrv. Update them together."
}

if ($crateContent -notmatch '(?m)^version\.workspace\s*=\s*true\s*$') {
    throw 'crates/rayman/Cargo.toml must inherit version from [workspace.package].'
}
if ($crateContent -notmatch '(?m)^rust-version\.workspace\s*=\s*true\s*$') {
    throw 'crates/rayman/Cargo.toml must inherit rust-version from [workspace.package].'
}

$lockMatch = [regex]::Match(
    $lockContent,
    '(?ms)^\[\[package\]\]\r?\nname = "rayman"\r?\nversion = "([^"]+)"'
)
if (-not $lockMatch.Success) {
    throw 'Cargo.lock does not contain a rayman package record.'
}
if ($lockMatch.Groups[1].Value -ne $expectedVersion) {
    throw "Cargo.lock reports rayman $($lockMatch.Groups[1].Value), expected $expectedVersion. Regenerate the lockfile with Cargo."
}

$sourceHeadBefore = $null
$rustcIdentityBefore = $null
if ($RequireSourceFresh) {
    Assert-NoBuildShapingEnvironment
    $rustcIdentityBefore = Get-RustcIdentity
}
if ($RequireSourceFresh -or $VerifyGitTag) {
    $sourceHeadBefore = Assert-CleanGitSource
}

$script:resolvedCli = Resolve-RequiredPath -Path $CliPath -Label 'CLI artifact'
$resolvedReference = if ($ReferenceCliPath) {
    Resolve-RequiredPath -Path $ReferenceCliPath -Label 'Reference CLI artifact'
}
$resolvedSkill = Resolve-RequiredPath -Path $SkillPath -Label 'Deployed canonical skill'
$resolvedCanonicalSkill = Resolve-RequiredPath -Path $canonicalSkill -Label 'Repository canonical skill'

Push-Location $repoRoot
try {
    $reportedVersion = Invoke-Rayman -Arguments @('--version')
    if ($reportedVersion -ne "rayman $expectedVersion") {
        throw "CLI reports '$reportedVersion', expected 'rayman $expectedVersion'."
    }

    $doctorText = Invoke-Rayman -Arguments @('--format', 'json', 'doctor', '--check')
    try {
        $doctor = $doctorText | ConvertFrom-Json -ErrorAction Stop
    } catch {
        throw "doctor did not return valid JSON: $doctorText"
    }
    if ($doctor.contract -ne $expectedContract) {
        throw "doctor reports contract '$($doctor.contract)', expected '$expectedContract'."
    }
    if ($doctor.version -ne $expectedVersion -or
        -not $doctor.PSObject.Properties['release_identity'] -or
        -not $doctor.release_identity.ready) {
        throw 'doctor did not report the expected ready installed identity.'
    }
    if (-not $doctor.PSObject.Properties['source_fresh'] -or
        $doctor.source_fresh.status -ne 'not_checked_by_doctor') {
        throw 'doctor did not explicitly distinguish installed identity from source freshness.'
    }
} finally {
    Pop-Location
}

$cliHash = Get-Sha256 -Path $script:resolvedCli
if ($resolvedReference) {
    $referenceHash = Get-Sha256 -Path $resolvedReference
    if ($cliHash -ne $referenceHash) {
        throw "CLI SHA-256 differs from the reference artifact: $cliHash != $referenceHash"
    }
}

$deployedSkillHash = Get-Sha256 -Path $resolvedSkill
$canonicalSkillHash = Get-Sha256 -Path $resolvedCanonicalSkill
if ($deployedSkillHash -ne $canonicalSkillHash) {
    throw "Deployed SKILL.md SHA-256 differs from the repository canonical skill: $deployedSkillHash != $canonicalSkillHash"
}

$sourceFreshHash = $null
if ($RequireSourceFresh) {
    $sourceFreshBuild = $null
    try {
        $sourceFreshBuild = Build-SourceFreshArtifact
        $rustcIdentityAfter = Get-RustcIdentity
        if ($rustcIdentityAfter -ne $rustcIdentityBefore) {
            throw 'The active Rust compiler changed during source-fresh verification.'
        }
        $sourceHeadAfter = Assert-CleanGitSource
        if ($sourceHeadAfter -ne $sourceHeadBefore) {
            throw "Source HEAD changed during isolated source-fresh build: $sourceHeadBefore -> $sourceHeadAfter"
        }
        $sourceFreshHash = Get-Sha256 -Path $sourceFreshBuild.Artifact
        if ($cliHash -ne $sourceFreshHash) {
            throw "CLI SHA-256 differs from a locked fresh-source rebuild: $cliHash != $sourceFreshHash"
        }
        if ($resolvedReference) {
            $referenceHash = Get-Sha256 -Path $resolvedReference
            if ($referenceHash -ne $sourceFreshHash) {
                throw "Reference CLI SHA-256 differs from a locked fresh-source rebuild: $referenceHash != $sourceFreshHash"
            }
        }
        $sourceHeadFinal = Assert-CleanGitSource
        if ($sourceHeadFinal -ne $sourceHeadBefore) {
            throw "Source HEAD changed while verifying the fresh artifact: $sourceHeadBefore -> $sourceHeadFinal"
        }
    } finally {
        if ($sourceFreshBuild) {
            Remove-SourceFreshBuild -TargetDir $sourceFreshBuild.TargetDir
        }
    }
}

if ($RequirePath) {
    # Check PowerShell's effective command first. Looking only for ApplicationInfo
    # silently skips higher-precedence aliases/functions and can certify a binary
    # that `rayman` will not actually invoke in this shell.
    $pathCli = Resolve-EffectiveRaymanApplication
    $pathHash = Get-Sha256 -Path $pathCli
    if ($pathHash -ne $cliHash) {
        throw "PATH rayman SHA-256 differs from -CliPath: $pathHash != $cliHash"
    }
}

if ($VerifyGitTag) {
    $tagHead = Assert-CleanGitSource
    if ($tagHead -ne $sourceHeadBefore) {
        throw "Source HEAD changed before Git tag verification: $sourceHeadBefore -> $tagHead"
    }
    $expectedTag = "v$expectedVersion"
    $tagOutput = & $script:gitApplication -C $repoRoot describe --exact-match --tags HEAD 2>$null
    if ($LASTEXITCODE -ne 0) {
        throw "HEAD is not tagged; expected $expectedTag."
    }
    $headTag = ($tagOutput | Out-String).Trim()
    if ($headTag -ne $expectedTag) {
        throw "Git tag '$headTag' does not match expected release tag '$expectedTag'."
    }
    Assert-GitHubTagContext -SourceHead $tagHead -ExactHeadTag $headTag
}

# Terminal identity check: all earlier hashes are claims about mutable paths. Re-resolve and
# re-hash every supplied/deployed/PATH identity immediately before success output so a
# concurrent replacement cannot inherit an already-validated digest.
$finalCliPath = Resolve-RequiredPath -Path $CliPath -Label 'CLI artifact (final check)'
if ($finalCliPath -ne $script:resolvedCli -or (Get-Sha256 -Path $finalCliPath) -ne $cliHash) {
    throw 'CLI artifact identity changed during release-contract verification.'
}
if ($resolvedReference) {
    $finalReferencePath = Resolve-RequiredPath -Path $ReferenceCliPath -Label 'Reference CLI artifact (final check)'
    if ($finalReferencePath -ne $resolvedReference -or
        (Get-Sha256 -Path $finalReferencePath) -ne $referenceHash) {
        throw 'Reference CLI artifact identity changed during release-contract verification.'
    }
}
$finalSkillPath = Resolve-RequiredPath -Path $SkillPath -Label 'Deployed canonical skill (final check)'
$finalCanonicalSkill = Resolve-RequiredPath -Path $canonicalSkill -Label 'Repository canonical skill (final check)'
if ($finalSkillPath -ne $resolvedSkill -or
    (Get-Sha256 -Path $finalSkillPath) -ne $deployedSkillHash -or
    (Get-Sha256 -Path $finalCanonicalSkill) -ne $canonicalSkillHash) {
    throw 'Deployed or repository canonical SKILL.md identity changed during verification.'
}
if ($RequirePath) {
    $finalPathCli = Resolve-EffectiveRaymanApplication
    if ($finalPathCli -ne $pathCli -or (Get-Sha256 -Path $finalPathCli) -ne $pathHash) {
        throw 'Effective PATH rayman identity changed during release-contract verification.'
    }
}
if ($RequireSourceFresh) {
    Assert-NoBuildShapingEnvironment
    if ((Get-RustcIdentity) -ne $rustcIdentityBefore) {
        throw 'The active Rust compiler changed before final release-contract output.'
    }
    $terminalHead = Assert-CleanGitSource
    if ($terminalHead -ne $sourceHeadBefore) {
        throw "Source HEAD changed before final release-contract output: $sourceHeadBefore -> $terminalHead"
    }
}
foreach ($name in @($toolIdentity.Keys | Sort-Object)) {
    $finalTool = Resolve-RequiredApplication -Name $name -Label "$name executable (final check)"
    $initialTool = $toolIdentity[$name]
    if ($finalTool -ne $initialTool.Path -or (Get-Sha256 $finalTool) -ne $initialTool.Hash) {
        throw "$name executable identity changed during release-contract verification."
    }
}

if ($RequirePath) {
    Write-Host "Installed release identity verified: rayman $expectedVersion (MSRV $expectedMsrv)"
} else {
    Write-Host "Release artifact contract verified: rayman $expectedVersion (MSRV $expectedMsrv)"
}
Write-Host "  CLI SHA-256: $cliHash"
if ($resolvedReference) {
    Write-Host "  Reference artifact: $resolvedReference"
}
Write-Host "  Canonical SKILL.md: $resolvedSkill"
if ($RequirePath) {
    Write-Host '  PATH identity: verified'
}
if ($VerifyGitTag) {
    Write-Host "  Git tag: v$expectedVersion"
}
if ($RequireSourceFresh) {
    Write-Host "  Source freshness: verified by locked isolated rebuild ($sourceFreshHash)"
    Write-Host "  Compiler identity: $($rustcIdentityBefore.Split([Environment]::NewLine)[0]) (active compiler consistency only; not toolchain provenance)"
} else {
    Write-Warning 'Source freshness was not checked. Release handoff/CI must pass -RequireSourceFresh.'
}
