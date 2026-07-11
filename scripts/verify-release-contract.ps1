[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidateNotNullOrEmpty()]
    [string]$CliPath,

    [ValidateNotNullOrEmpty()]
    [string]$ReferenceCliPath,

    [ValidateNotNullOrEmpty()]
    [string]$SkillPath,

    [switch]$RequirePath,

    [switch]$VerifyGitTag,

    # Rebuild rayman from the current clean checkout in an isolated target directory and
    # require byte identity with -CliPath (and -ReferenceCliPath when supplied).
    [switch]$RequireSourceFresh
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$repoRoot = Split-Path -Parent $PSScriptRoot
$workspaceManifest = Join-Path $repoRoot 'Cargo.toml'
$crateManifest = Join-Path $repoRoot 'crates/rayman/Cargo.toml'
$lockfile = Join-Path $repoRoot 'Cargo.lock'
$canonicalSkill = Join-Path $repoRoot 'SKILL.md'
$expectedContract = 'rayman-cli-contract-v5'
$requiredMsrv = '1.88'
$requiredCommands = @(
    'context',
    'goal',
    'check',
    'map',
    'assets',
    'temp',
    'state',
    'checkpoint',
    'autosave',
    'doctor'
)

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

function Test-WindowsMsvcHost {
    if (-not $IsWindows) {
        return $false
    }
    $version = (& rustc -vV 2>$null | Out-String).Trim()
    if ($LASTEXITCODE -ne 0) {
        throw 'Unable to inspect the Rust host for reproducible release-link verification.'
    }
    return $version -match '(?m)^host:\s+.+-pc-windows-msvc\s*$'
}

function Assert-CleanGitSource {
    $insideWorkTree = (& git -C $repoRoot rev-parse --is-inside-work-tree 2>$null | Out-String).Trim()
    if ($LASTEXITCODE -ne 0 -or $insideWorkTree -ne 'true') {
        throw "Source-fresh verification requires a Git worktree at $repoRoot."
    }
    $status = @(& git -C $repoRoot status --porcelain --untracked-files=all 2>$null)
    if ($LASTEXITCODE -ne 0) {
        throw 'Unable to inspect Git worktree status for source-fresh verification.'
    }
    $statusText = ($status | Out-String).Trim()
    if ($statusText) {
        throw "Source-fresh verification requires a clean Git worktree; found:`n$statusText"
    }
    $head = (& git -C $repoRoot rev-parse --verify HEAD 2>$null | Out-String).Trim()
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
        $env:CARGO_TARGET_DIR = $targetDir
        # Cargo config enables /Brepro for ordinary Windows MSVC builds. Force the
        # same linker contract here even when a caller supplied RUSTFLAGS that would
        # override config, otherwise the isolated PE gets a fresh timestamp/PDB ID
        # and a byte comparison becomes a false failure.
        if (Test-WindowsMsvcHost) {
            $brepro = @('-C', 'link-arg=/Brepro') -join [char]0x1f
            $env:CARGO_ENCODED_RUSTFLAGS = if ([string]::IsNullOrWhiteSpace($oldEncodedRustFlags)) {
                $brepro
            } else {
                "$oldEncodedRustFlags$([char]0x1f)$brepro"
            }
            Remove-Item Env:RUSTFLAGS -ErrorAction SilentlyContinue
        }
        $pushedLocation = $false
        try {
            Push-Location $repoRoot
            $pushedLocation = $true
            $buildOutput = & cargo build --locked --release -p rayman 2>&1
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
if ($RequireSourceFresh -or $VerifyGitTag) {
    $sourceHeadBefore = Assert-CleanGitSource
}

$script:resolvedCli = Resolve-RequiredPath -Path $CliPath -Label 'CLI artifact'
$resolvedReference = if ($ReferenceCliPath) {
    Resolve-RequiredPath -Path $ReferenceCliPath -Label 'Reference CLI artifact'
}
$resolvedSkill = if ($SkillPath) {
    Resolve-RequiredPath -Path $SkillPath -Label 'Deployed canonical skill'
}
$resolvedCanonicalSkill = Resolve-RequiredPath -Path $canonicalSkill -Label 'Repository canonical skill'

Push-Location $repoRoot
try {
    $reportedVersion = Invoke-Rayman -Arguments @('--version')
    if ($reportedVersion -ne "rayman $expectedVersion") {
        throw "CLI reports '$reportedVersion', expected 'rayman $expectedVersion'."
    }

    $help = Invoke-Rayman -Arguments @('--help')
    foreach ($command in $requiredCommands) {
        $commandPattern = "(?m)^\s{2,}$([regex]::Escape($command))\s"
        if ($help -notmatch $commandPattern) {
            throw "CLI help is missing required top-level command '$command'."
        }
    }

    $goalHelp = Invoke-Rayman -Arguments @('goal', '--help')
    if ($goalHelp -notmatch '(?m)^\s{2,}validate\s') {
        throw 'CLI goal help is missing receipt-producing subcommand validate.'
    }

    $checkpointHelp = Invoke-Rayman -Arguments @('checkpoint', '--help')
    if ($checkpointHelp -notmatch '(?m)^\s{2,}verify\s') {
        throw 'CLI checkpoint help is missing integrity-verifying subcommand verify.'
    }

    $stateHelp = Invoke-Rayman -Arguments @('state', '--help')
    if ($stateHelp -notmatch '(?m)^\s{2,}audit\s') {
        throw 'CLI state help is missing read-only subcommand audit.'
    }
    $stateAuditHelp = Invoke-Rayman -Arguments @('state', 'audit', '--help')
    if ($stateAuditHelp -notmatch '(?m)^\s+--check\s') {
        throw 'CLI state audit help is missing fail-closed --check.'
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

if ($resolvedSkill) {
    $deployedSkillHash = Get-Sha256 -Path $resolvedSkill
    $canonicalSkillHash = Get-Sha256 -Path $resolvedCanonicalSkill
    if ($deployedSkillHash -ne $canonicalSkillHash) {
        throw "Deployed SKILL.md SHA-256 differs from the repository canonical skill: $deployedSkillHash != $canonicalSkillHash"
    }
}

$sourceFreshHash = $null
if ($RequireSourceFresh) {
    $sourceFreshBuild = $null
    try {
        $sourceFreshBuild = Build-SourceFreshArtifact
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
    # PowerShell returns every matching ApplicationInfo when multiple rayman
    # binaries exist on PATH.  The release contract is about the command that
    # will actually run, i.e. PATH precedence, so select the first result.
    $pathCommand = @(Get-Command rayman -CommandType Application -ErrorAction Stop | Select-Object -First 1)[0]
    $pathCli = Resolve-RequiredPath -Path $pathCommand.Source -Label 'PATH rayman executable'
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
    $actualTag = if ($env:GITHUB_REF_TYPE -eq 'tag' -and $env:GITHUB_REF_NAME) {
        $env:GITHUB_REF_NAME
    } else {
        $tagOutput = & git -C $repoRoot describe --exact-match --tags HEAD 2>$null
        if ($LASTEXITCODE -ne 0) {
            throw "HEAD is not tagged; expected $expectedTag."
        }
        ($tagOutput | Out-String).Trim()
    }
    if ($actualTag -ne $expectedTag) {
        throw "Git tag '$actualTag' does not match expected release tag '$expectedTag'."
    }
}

Write-Host "Installed release identity verified: rayman $expectedVersion (MSRV $expectedMsrv)"
Write-Host "  CLI SHA-256: $cliHash"
if ($resolvedReference) {
    Write-Host "  Reference artifact: $resolvedReference"
}
if ($resolvedSkill) {
    Write-Host "  Canonical SKILL.md: $resolvedSkill"
}
if ($RequirePath) {
    Write-Host '  PATH identity: verified'
}
if ($VerifyGitTag) {
    Write-Host "  Git tag: v$expectedVersion"
}
if ($RequireSourceFresh) {
    Write-Host "  Source freshness: verified by locked isolated rebuild ($sourceFreshHash)"
} else {
    Write-Warning 'Source freshness was not checked. Release handoff/CI must pass -RequireSourceFresh.'
}
