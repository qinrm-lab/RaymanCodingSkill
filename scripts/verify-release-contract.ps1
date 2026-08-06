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
    [ValidateNotNullOrEmpty()]
    [string]$DoctorWorkspace,

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
$packagedCanonicalSkill = Join-Path $repoRoot 'crates/rayman/assets/canonical-skill.md'
$expectedContract = 'rayman-cli-contract-v16'
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

function Resolve-RequiredDirectory {
    param([string]$Path, [string]$Label)

    $fullPath = [IO.Path]::GetFullPath($Path)
    $root = [IO.Path]::GetPathRoot($fullPath)
    $comparison = if ($IsWindows) {
        [StringComparison]::OrdinalIgnoreCase
    } else {
        [StringComparison]::Ordinal
    }
    if ($fullPath.Equals($root, $comparison)) {
        throw "$Label cannot be a filesystem root: $fullPath"
    }
    $fullPath = $fullPath.TrimEnd(
        [IO.Path]::DirectorySeparatorChar,
        [IO.Path]::AltDirectorySeparatorChar
    )
    if (-not (Test-Path -LiteralPath $fullPath -PathType Container)) {
        throw "$Label is missing or is not a directory: $fullPath"
    }

    # Validate every named component. Checking only the final directory lets an
    # ancestor junction/symlink keep a lexical path outside the repository while
    # redirecting the doctor into repository state.
    $separators = [char[]]@(
        [IO.Path]::DirectorySeparatorChar,
        [IO.Path]::AltDirectorySeparatorChar
    )
    $relative = $fullPath.Substring($root.Length)
    $segments = @($relative.Split($separators, [StringSplitOptions]::RemoveEmptyEntries))
    $current = $root
    foreach ($segment in $segments) {
        $current = Join-Path $current $segment
        $item = Get-Item -LiteralPath $current -Force
        if (-not $item.PSIsContainer -or
            $item.Attributes -band [IO.FileAttributes]::ReparsePoint) {
            throw "$Label ancestor must be a real non-reparse directory: $current"
        }
    }

    $resolved = (Resolve-Path -LiteralPath $fullPath).Path.TrimEnd(
        [IO.Path]::DirectorySeparatorChar,
        [IO.Path]::AltDirectorySeparatorChar
    )
    if (-not $resolved.Equals($fullPath, $comparison)) {
        throw "$Label canonical path differs from the explicitly named path: $fullPath -> $resolved"
    }
    return $resolved
}

function Resolve-RequiredRegularFile {
    param([string]$Path, [string]$Label)

    $fullPath = [IO.Path]::GetFullPath($Path)
    $resolvedParent = Resolve-RequiredDirectory -Path (Split-Path -Parent $fullPath) -Label "$Label parent"
    if (-not (Test-Path -LiteralPath $fullPath -PathType Leaf)) {
        throw "$Label is missing or is not a regular file: $fullPath"
    }
    $item = Get-Item -LiteralPath $fullPath -Force
    if ($item.PSIsContainer -or
        $item.Attributes -band [IO.FileAttributes]::ReparsePoint) {
        throw "$Label must be a real regular file: $fullPath"
    }
    $resolved = (Resolve-Path -LiteralPath $fullPath).Path
    $comparison = if ($IsWindows) {
        [StringComparison]::OrdinalIgnoreCase
    } else {
        [StringComparison]::Ordinal
    }
    if (-not $resolved.Equals($fullPath, $comparison) -or
        -not (Split-Path -Parent $resolved).Equals($resolvedParent, $comparison)) {
        throw "$Label canonical path differs from the explicitly named path: $fullPath -> $resolved"
    }
    return $resolved
}

function Test-SameOrDescendantPath {
    param([string]$Candidate, [string]$Parent)

    $comparison = if ($IsWindows) {
        [StringComparison]::OrdinalIgnoreCase
    } else {
        [StringComparison]::Ordinal
    }
    if ($Candidate.Equals($Parent, $comparison)) {
        return $true
    }
    $prefix = $Parent.TrimEnd(
        [IO.Path]::DirectorySeparatorChar,
        [IO.Path]::AltDirectorySeparatorChar
    ) + [IO.Path]::DirectorySeparatorChar
    return $Candidate.StartsWith($prefix, $comparison)
}

function Resolve-IsolatedDoctorWorkspace {
    param([string]$Path)

    $resolved = Resolve-RequiredDirectory -Path $Path -Label 'Doctor workspace'
    $resolvedRepo = Resolve-RequiredDirectory -Path $repoRoot -Label 'Repository root'
    if ((Test-SameOrDescendantPath -Candidate $resolved -Parent $resolvedRepo) -or
        (Test-SameOrDescendantPath -Candidate $resolvedRepo -Parent $resolved)) {
        throw "Doctor workspace must be outside and disjoint from the repository tree: $resolved"
    }

    $stateRoot = Resolve-RequiredDirectory `
        -Path (Join-Path $resolved '.RaymanCodingSkill') `
        -Label 'Doctor workspace state root'
    $binding = Resolve-RequiredRegularFile `
        -Path (Join-Path $stateRoot 'workspace_skill.yaml') `
        -Label 'Doctor workspace activation binding'
    return [pscustomobject]@{
        Workspace = $resolved
        StateRoot = $stateRoot
        Binding = $binding
        BindingHash = Get-Sha256 -Path $binding
    }
}

function Assert-IsolatedDoctorWorkspaceSnapshot {
    param(
        [Parameter(Mandatory = $true)]$Expected,
        [Parameter(Mandatory = $true)]$Actual,
        [Parameter(Mandatory = $true)][string]$Label
    )

    $comparison = if ($IsWindows) {
        [StringComparison]::OrdinalIgnoreCase
    } else {
        [StringComparison]::Ordinal
    }
    foreach ($property in @('Workspace', 'StateRoot', 'Binding')) {
        $expectedPath = [string]$Expected.$property
        $actualPath = [string]$Actual.$property
        if (-not $actualPath.Equals($expectedPath, $comparison)) {
            throw "$Label $property identity changed during verification: $expectedPath -> $actualPath"
        }
    }
    if ($Actual.BindingHash -ne $Expected.BindingHash) {
        throw "$Label binding content changed during verification: $($Expected.BindingHash) -> $($Actual.BindingHash)"
    }
}

function Get-Sha256 {
    param([string]$Path)

    return (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}

function Assert-CanonicalSkillAssetBinding {
    param(
        [Parameter(Mandatory = $true)]
        [string]$RepositorySkillPath,
        [Parameter(Mandatory = $true)]
        [string]$PackagedSkillPath
    )

    $repositoryPath = Resolve-RequiredPath -Path $RepositorySkillPath -Label 'Repository canonical skill'
    $packagedPath = Resolve-RequiredPath -Path $PackagedSkillPath -Label 'Packaged canonical skill asset'
    $repositoryHash = Get-Sha256 -Path $repositoryPath
    $packagedHash = Get-Sha256 -Path $packagedPath
    if ($repositoryHash -ne $packagedHash) {
        throw "Packaged canonical skill SHA-256 differs from repository SKILL.md: $packagedHash != $repositoryHash"
    }
    return [pscustomobject]@{
        RepositoryPath = $repositoryPath
        PackagedPath = $packagedPath
        RepositoryHash = $repositoryHash
        PackagedHash = $packagedHash
    }
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

function Invoke-WithDoctorWorkspaceValidation {
    param(
        [Parameter(Mandatory = $true)][string]$Workspace,
        $ExpectedIsolatedSnapshot,
        [Parameter(Mandatory = $true)][scriptblock]$Operation
    )

    if ($null -ne $ExpectedIsolatedSnapshot) {
        $before = Resolve-IsolatedDoctorWorkspace -Path $Workspace
        Assert-IsolatedDoctorWorkspaceSnapshot `
            -Expected $ExpectedIsolatedSnapshot `
            -Actual $before `
            -Label 'Doctor workspace pre-check'
        $validatedWorkspace = $before.Workspace
    } else {
        $validatedWorkspace = Resolve-RequiredDirectory -Path $Workspace -Label 'Repository doctor workspace'
    }

    $result = $null
    $operationFailure = $null
    $postValidationFailure = $null
    try {
        $result = & $Operation $validatedWorkspace
    } catch {
        $operationFailure = $_.Exception
    } finally {
        try {
            if ($null -ne $ExpectedIsolatedSnapshot) {
                $after = Resolve-IsolatedDoctorWorkspace -Path $Workspace
                Assert-IsolatedDoctorWorkspaceSnapshot `
                    -Expected $ExpectedIsolatedSnapshot `
                    -Actual $after `
                    -Label 'Doctor workspace post-check'
            } else {
                $afterWorkspace = Resolve-RequiredDirectory -Path $Workspace -Label 'Repository doctor workspace (post-check)'
                $comparison = if ($IsWindows) {
                    [StringComparison]::OrdinalIgnoreCase
                } else {
                    [StringComparison]::Ordinal
                }
                if (-not $afterWorkspace.Equals($validatedWorkspace, $comparison)) {
                    throw "Repository doctor workspace identity changed during verification: $validatedWorkspace -> $afterWorkspace"
                }
            }
        } catch {
            $postValidationFailure = $_.Exception
        }
    }

    if ($null -ne $operationFailure) {
        if ($null -ne $postValidationFailure) {
            throw "Doctor invocation failed: $($operationFailure.Message)`nDoctor workspace post-check also failed: $($postValidationFailure.Message)"
        }
        throw $operationFailure
    }
    if ($null -ne $postValidationFailure) {
        throw "Doctor workspace post-check failed: $($postValidationFailure.Message)"
    }
    return $result
}

function Assert-DoctorReady {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Workspace,
        [Parameter(Mandatory = $true)]
        [string]$ExpectedSkillPath,
        $ExpectedIsolatedSnapshot
    )

    $invocation = Invoke-WithDoctorWorkspaceValidation `
        -Workspace $Workspace `
        -ExpectedIsolatedSnapshot $ExpectedIsolatedSnapshot `
        -Operation {
            param([string]$ValidatedWorkspace)

            $startingLocation = (Get-Location).Path
            $pushedLocation = $false
            $doctorText = $null
            try {
                Push-Location $ValidatedWorkspace
                $pushedLocation = $true
                $reportedVersion = Invoke-Rayman -Arguments @('--version')
                if ($reportedVersion -ne "rayman $expectedVersion") {
                    throw "CLI reports '$reportedVersion', expected 'rayman $expectedVersion'."
                }
                $doctorText = Invoke-Rayman -Arguments @('--format', 'json', 'doctor', '--check')
            } finally {
                if ($pushedLocation) {
                    Pop-Location
                }
                if (-not (Get-Location).Path.Equals(
                        $startingLocation,
                        $(if ($IsWindows) { [StringComparison]::OrdinalIgnoreCase } else { [StringComparison]::Ordinal })
                    )) {
                    throw 'Doctor verification did not restore the caller working directory.'
                }
            }
            return [pscustomobject]@{
                DoctorText = $doctorText
                Workspace = $ValidatedWorkspace
            }
        }

    $doctorText = $invocation.DoctorText
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
    if (-not $doctor.PSObject.Properties['workspace_skill'] -or
        [string]::IsNullOrWhiteSpace([string]$doctor.workspace_skill.path)) {
        throw 'doctor did not report the workspace skill path.'
    }
    $reportedDoctorSkill = [string]$doctor.workspace_skill.path
    $doctorSkillCandidate = if ([IO.Path]::IsPathRooted($reportedDoctorSkill)) {
        $reportedDoctorSkill
    } else {
        Join-Path $invocation.Workspace $reportedDoctorSkill
    }
    $doctorSkill = Resolve-RequiredPath -Path $doctorSkillCandidate -Label 'Doctor workspace skill'
    if (-not $doctorSkill.Equals(
            $ExpectedSkillPath,
            $(if ($IsWindows) { [StringComparison]::OrdinalIgnoreCase } else { [StringComparison]::Ordinal })
        )) {
        throw "doctor workspace skill path differs from -SkillPath: $doctorSkill != $ExpectedSkillPath"
    }
    return $doctor
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

function Get-ReleaseVerifierSelfTestItem {
    param([Parameter(Mandatory = $true)][string]$Path)

    try {
        return Get-Item -LiteralPath $Path -Force -ErrorAction Stop
    } catch {
        if ($_.CategoryInfo.Category -eq [System.Management.Automation.ErrorCategory]::ObjectNotFound) {
            return $null
        }
        throw "Unable to inspect release verifier self-test path '$Path': $($_.Exception.Message)"
    }
}

function Resolve-ReleaseVerifierSelfTestDirectory {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$TempRoot,
        [Parameter(Mandatory = $true)][string]$NamePattern,
        [switch]$AllowMissing,
        [switch]$RequireNoReparseDescendants
    )

    $comparison = if ($IsWindows) {
        [StringComparison]::OrdinalIgnoreCase
    } else {
        [StringComparison]::Ordinal
    }
    $resolvedTempRoot = Resolve-RequiredDirectory `
        -Path $TempRoot `
        -Label 'Release verifier self-test temp root'
    $fullPath = [IO.Path]::GetFullPath($Path).TrimEnd(
        [IO.Path]::DirectorySeparatorChar,
        [IO.Path]::AltDirectorySeparatorChar
    )
    $parent = Split-Path -Parent $fullPath
    $leaf = Split-Path -Leaf $fullPath
    if (-not $parent.Equals($resolvedTempRoot, $comparison) -or
        $leaf -notmatch $NamePattern) {
        throw "Refusing self-test cleanup outside the exact direct-child GUID scope: $fullPath"
    }

    $item = Get-ReleaseVerifierSelfTestItem -Path $fullPath
    if ($null -eq $item) {
        if ($AllowMissing) {
            return $null
        }
        throw "Release verifier self-test cleanup target is missing: $fullPath"
    }
    $resolved = Resolve-RequiredDirectory -Path $fullPath -Label 'Release verifier self-test cleanup root'
    if (-not $resolved.Equals($fullPath, $comparison)) {
        throw "Release verifier self-test cleanup root did not resolve exactly: $fullPath -> $resolved"
    }
    if ($RequireNoReparseDescendants) {
        $reparseDescendant = @(
            Get-ChildItem -LiteralPath $resolved -Force -Recurse |
                Where-Object { $_.Attributes -band [IO.FileAttributes]::ReparsePoint } |
                Select-Object -First 1
        )
        if ($reparseDescendant.Count -gt 0) {
            throw "Refusing recursive self-test cleanup with a reparse descendant: $($reparseDescendant[0].FullName)"
        }
        $terminalResolved = Resolve-RequiredDirectory `
            -Path $fullPath `
            -Label 'Release verifier self-test cleanup root (terminal check)'
        if (-not $terminalResolved.Equals($resolved, $comparison)) {
            throw "Release verifier self-test cleanup root changed before deletion: $resolved -> $terminalResolved"
        }
    }
    return $resolved
}

function Remove-ReleaseVerifierSelfTestDirectory {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$TempRoot,
        [Parameter(Mandatory = $true)][string]$NamePattern
    )

    $resolved = Resolve-ReleaseVerifierSelfTestDirectory `
        -Path $Path `
        -TempRoot $TempRoot `
        -NamePattern $NamePattern `
        -AllowMissing `
        -RequireNoReparseDescendants
    if ($null -eq $resolved) {
        return
    }
    Remove-Item -LiteralPath $resolved -Recurse -Force
    $remaining = Get-ReleaseVerifierSelfTestItem -Path $resolved
    if ($null -ne $remaining) {
        throw "Release verifier self-test cleanup did not remove its exact target: $resolved"
    }
}

function Remove-ReleaseVerifierSelfTestReparsePoint {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$ExpectedParent,
        [Parameter(Mandatory = $true)][string]$TempRoot,
        [Parameter(Mandatory = $true)][string]$ParentNamePattern
    )

    $comparison = if ($IsWindows) {
        [StringComparison]::OrdinalIgnoreCase
    } else {
        [StringComparison]::Ordinal
    }
    $resolvedParent = Resolve-ReleaseVerifierSelfTestDirectory `
        -Path $ExpectedParent `
        -TempRoot $TempRoot `
        -NamePattern $ParentNamePattern `
        -AllowMissing
    $fullPath = [IO.Path]::GetFullPath($Path)
    if ((Split-Path -Leaf $fullPath) -ne 'linked-ancestor') {
        throw "Refusing to remove an unexpected self-test reparse path: $fullPath"
    }
    if ($null -eq $resolvedParent) {
        $orphan = Get-ReleaseVerifierSelfTestItem -Path $fullPath
        if ($null -ne $orphan) {
            throw "Self-test reparse point exists without its validated parent: $fullPath"
        }
        return
    }
    if (-not (Split-Path -Parent $fullPath).Equals($resolvedParent, $comparison)) {
        throw "Refusing self-test reparse cleanup outside its exact parent: $fullPath"
    }

    $item = Get-ReleaseVerifierSelfTestItem -Path $fullPath
    if ($null -eq $item) {
        return
    }
    $itemPath = [IO.Path]::GetFullPath($item.FullName)
    if (-not $itemPath.Equals($fullPath, $comparison) -or
        -not ($item.Attributes -band [IO.FileAttributes]::ReparsePoint)) {
        throw "Refusing to remove a non-reparse or path-drifted self-test entry: $fullPath"
    }
    if ($item.PSIsContainer) {
        [IO.Directory]::Delete($fullPath)
    } else {
        [IO.File]::Delete($fullPath)
    }
    $remaining = Get-ReleaseVerifierSelfTestItem -Path $fullPath
    if ($null -ne $remaining) {
        throw "Release verifier self-test reparse cleanup did not remove its exact target: $fullPath"
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

    $repositorySkillProbe = [IO.Path]::GetTempFileName()
    $packagedSkillProbe = [IO.Path]::GetTempFileName()
    try {
        [IO.File]::WriteAllBytes($repositorySkillProbe, [byte[]](1, 2, 3))
        [IO.File]::WriteAllBytes($packagedSkillProbe, [byte[]](1, 2, 4))
        $assetMismatchRejected = $false
        try {
            $null = Assert-CanonicalSkillAssetBinding -RepositorySkillPath $repositorySkillProbe -PackagedSkillPath $packagedSkillProbe
        } catch {
            $assetMismatchRejected = $_.Exception.Message -match 'Packaged canonical skill SHA-256 differs'
        }
        if (-not $assetMismatchRejected) {
            throw 'Release verifier self-test failed: a mismatched packaged canonical skill asset was not rejected.'
        }
    } finally {
        Remove-Item -LiteralPath $repositorySkillProbe, $packagedSkillProbe -Force -ErrorAction SilentlyContinue
    }

    $selfTestTempRoot = Resolve-RequiredDirectory `
        -Path ([IO.Path]::GetTempPath()) `
        -Label 'Release verifier self-test temp root'
    $selfTestRootNamePattern = '^' +
        [regex]::Escape("rayman-release-doctor-selftest-$PID-") +
        '[0-9a-f]{32}(?:-(?:missing|reparse-target|reparse-carrier))?$'
    $doctorProbeRoot = Join-Path $selfTestTempRoot (
        "rayman-release-doctor-selftest-$PID-" + [Guid]::NewGuid().ToString('N')
    )
    $missingMarkerRoot = "$doctorProbeRoot-missing"
    $reparseTargetRoot = "$doctorProbeRoot-reparse-target"
    $reparseCarrierRoot = "$doctorProbeRoot-reparse-carrier"
    $reparseLink = Join-Path $reparseCarrierRoot 'linked-ancestor'
    try {
        New-Item -ItemType Directory -Path (Join-Path $doctorProbeRoot '.RaymanCodingSkill') -Force | Out-Null
        $bindingProbe = Join-Path $doctorProbeRoot '.RaymanCodingSkill/workspace_skill.yaml'
        [IO.File]::WriteAllText(
            $bindingProbe,
            ('self-test marker' + [Environment]::NewLine),
            [Text.UTF8Encoding]::new($false)
        )
        $resolvedProbe = Resolve-IsolatedDoctorWorkspace -Path $doctorProbeRoot
        if (-not $resolvedProbe.Workspace.Equals(
                (Resolve-Path -LiteralPath $doctorProbeRoot).Path,
                $(if ($IsWindows) { [StringComparison]::OrdinalIgnoreCase } else { [StringComparison]::Ordinal })
            )) {
            throw 'Release verifier self-test failed: isolated doctor workspace resolution drifted.'
        }

        # Exercise the same wrapper used by each real doctor invocation: a binding
        # mutation after the pre-check must be rejected by the post-check.
        $originalBinding = [IO.File]::ReadAllBytes($bindingProbe)
        $terminalMutationRejected = $false
        try {
            $null = Invoke-WithDoctorWorkspaceValidation `
                -Workspace $resolvedProbe.Workspace `
                -ExpectedIsolatedSnapshot $resolvedProbe `
                -Operation {
                    param([string]$ValidatedWorkspace)
                    [IO.File]::WriteAllText(
                        (Join-Path $ValidatedWorkspace '.RaymanCodingSkill/workspace_skill.yaml'),
                        ('mutated self-test marker' + [Environment]::NewLine),
                        [Text.UTF8Encoding]::new($false)
                    )
                    return 'mutation-complete'
                }
        } catch {
            $terminalMutationRejected = $_.Exception.Message -match 'post-check' -and
                $_.Exception.Message -match 'binding content changed'
        } finally {
            [IO.File]::WriteAllBytes($bindingProbe, $originalBinding)
        }
        if (-not $terminalMutationRejected) {
            throw 'Release verifier self-test failed: terminal doctor workspace mutation was not rejected.'
        }
        $restoredProbe = Resolve-IsolatedDoctorWorkspace -Path $doctorProbeRoot
        Assert-IsolatedDoctorWorkspaceSnapshot `
            -Expected $resolvedProbe `
            -Actual $restoredProbe `
            -Label 'Release verifier self-test restoration'

        New-Item -ItemType Directory -Path $missingMarkerRoot -Force | Out-Null
        foreach ($invalid in @(
            $repoRoot,
            (Join-Path $repoRoot 'scripts'),
            (Split-Path -Parent $repoRoot),
            $missingMarkerRoot
        )) {
            $rejected = $false
            try {
                $null = Resolve-IsolatedDoctorWorkspace -Path $invalid
            } catch {
                $rejected = $true
            }
            if (-not $rejected) {
                throw "Release verifier self-test failed: unsafe doctor workspace was accepted: $invalid"
            }
        }

        $reparseDoctorRoot = Join-Path $reparseTargetRoot 'doctor-workspace'
        New-Item -ItemType Directory -Path (Join-Path $reparseDoctorRoot '.RaymanCodingSkill') -Force | Out-Null
        [IO.File]::WriteAllText(
            (Join-Path $reparseDoctorRoot '.RaymanCodingSkill/workspace_skill.yaml'),
            ('reparse self-test marker' + [Environment]::NewLine),
            [Text.UTF8Encoding]::new($false)
        )
        New-Item -ItemType Directory -Path $reparseCarrierRoot -Force | Out-Null
        if ($IsWindows) {
            New-Item -ItemType Junction -Path $reparseLink -Target $reparseTargetRoot | Out-Null
        } else {
            New-Item -ItemType SymbolicLink -Path $reparseLink -Target $reparseTargetRoot | Out-Null
        }
        $ancestorReparseRejected = $false
        try {
            $null = Resolve-IsolatedDoctorWorkspace -Path (Join-Path $reparseLink 'doctor-workspace')
        } catch {
            $ancestorReparseRejected = $_.Exception.Message -match 'ancestor must be a real non-reparse directory'
        }
        if (-not $ancestorReparseRejected) {
            throw 'Release verifier self-test failed: an ancestor reparse point was not rejected.'
        }
    } finally {
        Remove-ReleaseVerifierSelfTestReparsePoint `
            -Path $reparseLink `
            -ExpectedParent $reparseCarrierRoot `
            -TempRoot $selfTestTempRoot `
            -ParentNamePattern $selfTestRootNamePattern
        foreach ($owned in @(
            $doctorProbeRoot,
            $missingMarkerRoot,
            $reparseCarrierRoot,
            $reparseTargetRoot
        )) {
            Remove-ReleaseVerifierSelfTestDirectory `
                -Path $owned `
                -TempRoot $selfTestTempRoot `
                -NamePattern $selfTestRootNamePattern
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
    $selfTestManifest = Read-RequiredFile -Path $workspaceManifest -Label 'Workspace manifest'
    $selfTestVersion = Get-ManifestValue -Content $selfTestManifest -Key 'version' -Label $workspaceManifest
    $exactTag = "v$selfTestVersion"
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

    Write-Host 'Release verifier self-test passed: native command shadows, packaged canonical skill drift, isolated doctor workspace ancestry/terminal guards, and forged GitHub tag context were verified.'
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
$doctorWorkspaceSnapshot = $null
$resolvedDoctorWorkspace = if ([string]::IsNullOrWhiteSpace($DoctorWorkspace)) {
    Resolve-RequiredDirectory -Path $repoRoot -Label 'Repository doctor workspace'
} else {
    $doctorWorkspaceSnapshot = Resolve-IsolatedDoctorWorkspace -Path $DoctorWorkspace
    $doctorWorkspaceSnapshot.Workspace
}
$canonicalSkillBinding = Assert-CanonicalSkillAssetBinding -RepositorySkillPath $canonicalSkill -PackagedSkillPath $packagedCanonicalSkill
$resolvedCanonicalSkill = $canonicalSkillBinding.RepositoryPath
$resolvedPackagedCanonicalSkill = $canonicalSkillBinding.PackagedPath

$doctor = Assert-DoctorReady `
    -Workspace $resolvedDoctorWorkspace `
    -ExpectedSkillPath $resolvedSkill `
    -ExpectedIsolatedSnapshot $doctorWorkspaceSnapshot
$cliHash = Get-Sha256 -Path $script:resolvedCli
if ($resolvedReference) {
    $referenceHash = Get-Sha256 -Path $resolvedReference
    if ($cliHash -ne $referenceHash) {
        throw "CLI SHA-256 differs from the reference artifact: $cliHash != $referenceHash"
    }
}

$deployedSkillHash = Get-Sha256 -Path $resolvedSkill
$canonicalSkillHash = $canonicalSkillBinding.RepositoryHash
$packagedCanonicalSkillHash = $canonicalSkillBinding.PackagedHash
if ($deployedSkillHash -ne $canonicalSkillHash) {
    throw "Deployed SKILL.md SHA-256 differs from the repository canonical skill: $deployedSkillHash != $canonicalSkillHash"
}

# install-manifest.json deploys three Codex skill resources under one rollback
# transaction, and this script is the artifact-identity primitive the release
# contract names — but it hashed only SKILL.md, so AGENTS.md and the workflow
# contract could be arbitrarily stale in the deployed skill and still certify.
# Verify every resource the manifest actually deploys, relative to the deployed
# SKILL.md's own directory (that is the deployment root).
$manifestPath = Resolve-RequiredPath -Path (Join-Path $repoRoot 'install-manifest.json') -Label 'Install manifest'
$manifest = Get-Content -Raw -LiteralPath $manifestPath | ConvertFrom-Json
$deploymentRoot = Split-Path -Parent $resolvedSkill
$verifiedSkillResources = @()
foreach ($resource in @($manifest.codex_skill_resources)) {
    if ($resource.destination -eq 'SKILL.md') { continue }  # already checked above
    $deployedResource = Resolve-RequiredPath `
        -Path (Join-Path $deploymentRoot $resource.destination) `
        -Label "Deployed skill resource $($resource.destination)"
    $sourceResource = Resolve-RequiredPath `
        -Path (Join-Path $repoRoot $resource.source) `
        -Label "Repository skill resource $($resource.source)"
    $deployedResourceHash = Get-Sha256 -Path $deployedResource
    $sourceResourceHash = Get-Sha256 -Path $sourceResource
    if ($deployedResourceHash -ne $sourceResourceHash) {
        throw "Deployed $($resource.destination) SHA-256 differs from the repository source $($resource.source): $deployedResourceHash != $sourceResourceHash"
    }
    $verifiedSkillResources += [pscustomobject]@{
        Destination = $resource.destination
        Source = $resource.source
        DeployedPath = $deployedResource
        SourcePath = $sourceResource
        DeployedHash = $deployedResourceHash
        SourceHash = $sourceResourceHash
    }
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
$finalPackagedCanonicalSkill = Resolve-RequiredPath -Path $packagedCanonicalSkill -Label 'Packaged canonical skill asset (final check)'
if ($finalSkillPath -ne $resolvedSkill -or
    $finalCanonicalSkill -ne $resolvedCanonicalSkill -or
    $finalPackagedCanonicalSkill -ne $resolvedPackagedCanonicalSkill -or
    (Get-Sha256 -Path $finalSkillPath) -ne $deployedSkillHash -or
    (Get-Sha256 -Path $finalCanonicalSkill) -ne $canonicalSkillHash -or
    (Get-Sha256 -Path $finalPackagedCanonicalSkill) -ne $packagedCanonicalSkillHash) {
    throw 'Deployed, repository, or packaged canonical SKILL.md identity changed during verification.'
}
# Every other supplied/deployed identity is re-resolved and re-hashed here so a
# concurrent replacement cannot inherit an already-validated digest. The other
# manifest-deployed skill resources must obey the same rule as SKILL.md.
foreach ($verified in $verifiedSkillResources) {
    $finalDeployed = Resolve-RequiredPath `
        -Path (Join-Path $deploymentRoot $verified.Destination) `
        -Label "Deployed skill resource $($verified.Destination) (final check)"
    $finalSource = Resolve-RequiredPath `
        -Path (Join-Path $repoRoot $verified.Source) `
        -Label "Repository skill resource $($verified.Source) (final check)"
    if ($finalDeployed -ne $verified.DeployedPath -or
        $finalSource -ne $verified.SourcePath -or
        (Get-Sha256 -Path $finalDeployed) -ne $verified.DeployedHash -or
        (Get-Sha256 -Path $finalSource) -ne $verified.SourceHash) {
        throw "Deployed or repository $($verified.Destination) identity changed during verification."
    }
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
if ($null -ne $doctorWorkspaceSnapshot) {
    $terminalDoctorWorkspaceSnapshot = Resolve-IsolatedDoctorWorkspace -Path $resolvedDoctorWorkspace
    Assert-IsolatedDoctorWorkspaceSnapshot `
        -Expected $doctorWorkspaceSnapshot `
        -Actual $terminalDoctorWorkspaceSnapshot `
        -Label 'Terminal doctor workspace check'
    $resolvedDoctorWorkspace = $terminalDoctorWorkspaceSnapshot.Workspace
} else {
    $resolvedDoctorWorkspace = Resolve-RequiredDirectory `
        -Path $repoRoot `
        -Label 'Repository doctor workspace (terminal check)'
}
$null = Assert-DoctorReady `
    -Workspace $resolvedDoctorWorkspace `
    -ExpectedSkillPath $finalSkillPath `
    -ExpectedIsolatedSnapshot $doctorWorkspaceSnapshot

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
