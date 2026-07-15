[CmdletBinding()]
param(
    [string]$BinDirectory,

    [string]$SkillDirectory,

    [switch]$AddToUserPath,

    [switch]$SelfTest,

    [switch]$Yes
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if ($PSVersionTable.PSVersion.Major -lt 7) {
    throw 'install-rayman.ps1 requires PowerShell 7+. Run it with pwsh, not Windows PowerShell.'
}
if ($AddToUserPath -and -not $IsWindows) {
    throw '-AddToUserPath is supported only on Windows. Configure PATH before installation on this platform.'
}

$repoRoot = Split-Path -Parent $PSScriptRoot
$artifactName = if ($IsWindows) { 'rayman.exe' } else { 'rayman' }
if ([string]::IsNullOrWhiteSpace($BinDirectory)) {
    $BinDirectory = if ($IsWindows) {
        Join-Path $env:LOCALAPPDATA 'Rayman/bin'
    } else {
        Join-Path $HOME '.local/bin'
    }
}
if ([string]::IsNullOrWhiteSpace($SkillDirectory)) {
    $SkillDirectory = Join-Path $HOME '.codex/skills/raymancodingskill'
}

function Invoke-NativeChecked {
    param(
        [Parameter(Mandatory = $true)]
        [string]$FilePath,
        [string[]]$Arguments
    )

    & $FilePath @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "$FilePath $($Arguments -join ' ') failed with exit code $LASTEXITCODE."
    }
}

function Resolve-RequiredApplication {
    param([string]$Name, [string]$Label)

    $commands = @(Get-Command $Name -All -ErrorAction Stop)
    if ($commands.Count -eq 0 -or $commands[0].CommandType -ne 'Application') {
        $kind = if ($commands.Count -eq 0) { 'missing' } else { $commands[0].CommandType }
        throw "$Label must resolve directly to an Application; effective command is $kind."
    }
    $path = $commands[0].Source
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
        throw "$Label is missing: $path"
    }
    return (Resolve-Path -LiteralPath $path).Path
}

function Get-PathComparisonKey {
    param([string]$Entry)

    if ([string]::IsNullOrWhiteSpace($Entry)) {
        return ''
    }
    $candidate = [Environment]::ExpandEnvironmentVariables($Entry.Trim().Trim('"'))
    try {
        $candidate = [IO.Path]::GetFullPath($candidate)
    } catch {
        # Preserve an unusual PATH entry for comparison instead of making path
        # normalization itself an installation side effect.
    }
    $root = [IO.Path]::GetPathRoot($candidate)
    if ($candidate -ne $root) {
        $candidate = $candidate.TrimEnd(
            [IO.Path]::DirectorySeparatorChar,
            [IO.Path]::AltDirectorySeparatorChar
        )
    }
    if ($IsWindows) {
        return $candidate.ToLowerInvariant()
    }
    return $candidate
}

function Get-ProposedUserPath {
    param([AllowNull()][string]$ExistingUserPath, [string]$CliDirectory)

    $targetKey = Get-PathComparisonKey -Entry $CliDirectory
    $entries = @(
        $ExistingUserPath -split [IO.Path]::PathSeparator |
            Where-Object { -not [string]::IsNullOrWhiteSpace($_) }
    )
    $remaining = @($entries | Where-Object {
        (Get-PathComparisonKey -Entry $_) -ne $targetKey
    })
    return (@($CliDirectory) + $remaining) -join [IO.Path]::PathSeparator
}

function Get-ProjectedPersistentPath {
    param(
        [AllowNull()][string]$MachinePath,
        [AllowNull()][string]$UserPath
    )

    $parts = @()
    foreach ($part in @($MachinePath, $UserPath)) {
        if (-not [string]::IsNullOrWhiteSpace($part)) {
            $parts += $part
        }
    }
    return [Environment]::ExpandEnvironmentVariables(
        ($parts -join [IO.Path]::PathSeparator)
    )
}

function Resolve-ManagedDirectory {
    param([string]$Path, [string]$Label)

    $fullPath = [IO.Path]::GetFullPath($Path)
    $root = [IO.Path]::GetPathRoot($fullPath)
    if ($fullPath -eq $root) {
        throw "$Label cannot be a filesystem root: $fullPath"
    }

    # Walk from the filesystem root, validating each existing ancestor before
    # creating the next component. New-Item -Force on the final path alone would
    # follow an ancestor junction/symlink and create files outside the named root.
    $separators = [char[]]@(
        [IO.Path]::DirectorySeparatorChar,
        [IO.Path]::AltDirectorySeparatorChar
    )
    $relative = $fullPath.Substring($root.Length)
    $segments = @($relative.Split($separators, [StringSplitOptions]::RemoveEmptyEntries))
    $current = $root
    foreach ($segment in $segments) {
        $current = Join-Path $current $segment
        if (-not (Test-Path -LiteralPath $current)) {
            New-Item -ItemType Directory -Path $current | Out-Null
        }
        $item = Get-Item -LiteralPath $current -Force
        if (-not $item.PSIsContainer -or
            $item.Attributes -band [IO.FileAttributes]::ReparsePoint) {
            throw "$Label ancestor must be a real directory, not a file or reparse point: $current"
        }
    }

    # Recheck the full chain and canonical equality after creation. This closes
    # ordinary symlink/junction escapes; a hostile same-user TOCTOU race remains
    # outside PowerShell's ability to make a multi-file install fully race-free.
    $current = $root
    foreach ($segment in $segments) {
        $current = Join-Path $current $segment
        $item = Get-Item -LiteralPath $current -Force
        if (-not $item.PSIsContainer -or
            $item.Attributes -band [IO.FileAttributes]::ReparsePoint) {
            throw "$Label ancestor changed into a file or reparse point: $current"
        }
    }
    $resolved = (Resolve-Path -LiteralPath $fullPath).Path
    $comparison = if ($IsWindows) {
        [StringComparison]::OrdinalIgnoreCase
    } else {
        [StringComparison]::Ordinal
    }
    if (-not $resolved.Equals($fullPath, $comparison)) {
        throw "$Label canonical path escaped the explicitly named destination: $fullPath -> $resolved"
    }
    return $resolved
}

function Assert-ReplaceableFile {
    param([string]$Path, [string]$Label)

    $item = Get-Item -LiteralPath $Path -Force -ErrorAction SilentlyContinue
    if ($null -eq $item) {
        return
    }
    if ($item.PSIsContainer -or $item.Attributes -band [IO.FileAttributes]::ReparsePoint) {
        throw "$Label destination must be a regular file: $Path"
    }
}

function Assert-ExpectedFileHash {
    param([string]$Path, [string]$ExpectedHash, [string]$Label)

    Assert-ReplaceableFile -Path $Path -Label $Label
    $actual = (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($actual -ne $ExpectedHash) {
        throw "$Label hash drifted after verification: $actual != $ExpectedHash ($Path)"
    }
}

function Install-FileWithRollback {
    param(
        [string]$Source,
        [string]$Destination,
        [string]$Nonce,
        [string]$ExpectedHash,
        [switch]$TestFailBackupMove
    )

    $destinationDirectory = Split-Path -Parent $Destination
    $staged = "$Destination.install-$Nonce"
    $backup = "$Destination.backup-$Nonce"
    $hadOriginal = $false
    $ownsStagedPath = $false
    $ownsBackupPath = $false
    try {
        $null = Resolve-ManagedDirectory -Path $destinationDirectory -Label 'Managed destination directory'
        if (Test-Path -LiteralPath $staged) {
            throw "Refusing occupied install staging path: $staged"
        }
        if (Test-Path -LiteralPath $backup) {
            throw "Refusing occupied install backup path: $backup"
        }
        Assert-ReplaceableFile -Path $Destination -Label 'Managed file'
        Assert-ExpectedFileHash -Path $Source -ExpectedHash $ExpectedHash -Label 'Verified install source'

        # Mark ownership before Copy-Item so a partially created staging file is
        # still removed when the copy itself fails.
        $ownsStagedPath = $true
        Copy-Item -LiteralPath $Source -Destination $staged
        $stagedHash = (Get-FileHash -LiteralPath $staged -Algorithm SHA256).Hash.ToLowerInvariant()
        if ($stagedHash -ne $ExpectedHash) {
            throw "Staged file hash mismatch against verified identity: $Destination"
        }
        Assert-ExpectedFileHash -Path $Source -ExpectedHash $ExpectedHash -Label 'Verified install source'

        $hadOriginal = Test-Path -LiteralPath $Destination -PathType Leaf
        if ($hadOriginal) {
            if ($TestFailBackupMove) {
                throw 'Injected backup-move failure for installer self-test.'
            }
            # As with staging, claim the previously empty nonce path before the
            # move so a partial move can be detected and restored in the catch.
            $ownsBackupPath = $true
            Move-Item -LiteralPath $Destination -Destination $backup
        }
        Move-Item -LiteralPath $staged -Destination $Destination
        $ownsStagedPath = $false
    } catch {
        $installFailure = $_.Exception.Message
        $recoveryErrors = @()

        if ($ownsBackupPath -and (Test-Path -LiteralPath $backup)) {
            try {
                if (-not (Test-Path -LiteralPath $backup -PathType Leaf)) {
                    throw "backup is not a regular file: $backup"
                }
                if (Test-Path -LiteralPath $Destination) {
                    Assert-ReplaceableFile -Path $Destination -Label 'Failed install destination'
                    Remove-Item -LiteralPath $Destination -Force
                }
                Move-Item -LiteralPath $backup -Destination $Destination
                $ownsBackupPath = $false
            } catch {
                $recoveryErrors += "unable to restore backup '$backup' to '$Destination': $($_.Exception.Message)"
            }
        } elseif ($hadOriginal -and -not (Test-Path -LiteralPath $Destination -PathType Leaf)) {
            $recoveryErrors += "original destination disappeared without a recoverable backup: $Destination"
        }

        if ($ownsStagedPath -and (Test-Path -LiteralPath $staged)) {
            try {
                $stagedItem = Get-Item -LiteralPath $staged -Force
                if ($stagedItem.PSIsContainer -or
                    $stagedItem.Attributes -band [IO.FileAttributes]::ReparsePoint) {
                    throw "staging path is not a regular file: $staged"
                }
                Remove-Item -LiteralPath $staged -Force
                $ownsStagedPath = $false
            } catch {
                $recoveryErrors += "unable to remove staging path '$staged': $($_.Exception.Message)"
            }
        }

        if ($recoveryErrors.Count -gt 0) {
            throw "Install-file transaction failed: $installFailure`nRecovery was incomplete; retained evidence requires review:`n$($recoveryErrors -join "`n")"
        }
        throw $installFailure
    }
    return [pscustomobject]@{
        Destination = $Destination
        Backup = $backup
        HadOriginal = $hadOriginal
    }
}

function Restore-InstalledFile {
    param($InstallRecord)

    if ($InstallRecord.HadOriginal -and
        -not (Test-Path -LiteralPath $InstallRecord.Backup -PathType Leaf)) {
        throw "Rollback backup is missing; preserving current destination: $($InstallRecord.Backup)"
    }
    if (Test-Path -LiteralPath $InstallRecord.Destination -PathType Leaf) {
        Remove-Item -LiteralPath $InstallRecord.Destination -Force
    }
    if ($InstallRecord.HadOriginal -and
        (Test-Path -LiteralPath $InstallRecord.Backup -PathType Leaf)) {
        Move-Item -LiteralPath $InstallRecord.Backup -Destination $InstallRecord.Destination
    }
}

function Invoke-InstallRollback {
    param([array]$InstallRecords)

    $rollback = @($InstallRecords)
    [array]::Reverse($rollback)
    $errors = @()
    foreach ($record in $rollback) {
        try {
            Restore-InstalledFile -InstallRecord $record
        } catch {
            $errors += "destination=$($record.Destination) backup=$($record.Backup): $($_.Exception.Message)"
        }
    }
    return $errors
}

function Remove-CommittedBackup {
    param($InstallRecord)

    if (-not (Test-Path -LiteralPath $InstallRecord.Backup)) {
        return $null
    }
    if (-not (Test-Path -LiteralPath $InstallRecord.Backup -PathType Leaf)) {
        return "Refusing to delete non-file committed backup; retained for review: $($InstallRecord.Backup)"
    }
    try {
        Remove-Item -LiteralPath $InstallRecord.Backup -Force
        return $null
    } catch {
        return "Unable to remove committed backup; retained for review: $($InstallRecord.Backup): $($_.Exception.Message)"
    }
}

function Invoke-InstallPathSelfTest {
    $managedTemp = Resolve-ManagedDirectory `
        -Path (Join-Path $repoRoot '.RaymanCodingSkill/tmp') `
        -Label 'Managed self-test temp'
    $testRoot = Join-Path $managedTemp ("install-path-selftest-" + [Guid]::NewGuid().ToString('N'))
    $inside = Join-Path $testRoot 'inside'
    $outside = Join-Path $testRoot 'outside'
    $link = Join-Path $inside 'escape'
    $null = Resolve-ManagedDirectory -Path $inside -Label 'Self-test inside'
    $null = Resolve-ManagedDirectory -Path $outside -Label 'Self-test outside'
    try {
        $hashProbe = Join-Path $testRoot 'hash-probe.bin'
        Set-Content -LiteralPath $hashProbe -Value 'verified' -NoNewline -Encoding utf8
        $expectedHash = (Get-FileHash -LiteralPath $hashProbe -Algorithm SHA256).Hash.ToLowerInvariant()
        Set-Content -LiteralPath $hashProbe -Value 'drifted' -NoNewline -Encoding utf8
        $hashDriftRejected = $false
        try {
            Assert-ExpectedFileHash -Path $hashProbe -ExpectedHash $expectedHash -Label 'Self-test source'
        } catch {
            $hashDriftRejected = $_.Exception.Message -match 'hash drifted'
        }
        if (-not $hashDriftRejected) {
            throw 'Install path self-test failed: verified source hash drift was not rejected.'
        }

        $transactionSource = Join-Path $testRoot 'transaction-source.bin'
        $transactionDestination = Join-Path $testRoot 'transaction-destination.bin'
        Set-Content -LiteralPath $transactionSource -Value 'new-data' -NoNewline -Encoding utf8
        Set-Content -LiteralPath $transactionDestination -Value 'old-data' -NoNewline -Encoding utf8
        $transactionHash = (Get-FileHash -LiteralPath $transactionSource -Algorithm SHA256).Hash.ToLowerInvariant()
        $transactionNonce = 'forced-backup-move-failure'
        $backupMoveRejected = $false
        try {
            $null = Install-FileWithRollback `
                -Source $transactionSource `
                -Destination $transactionDestination `
                -Nonce $transactionNonce `
                -ExpectedHash $transactionHash `
                -TestFailBackupMove
        } catch {
            $backupMoveRejected = $_.Exception.Message -match 'Injected backup-move failure'
        }
        if (-not $backupMoveRejected -or
            (Get-Content -Raw -LiteralPath $transactionDestination) -ne 'old-data' -or
            (Test-Path -LiteralPath "$transactionDestination.install-$transactionNonce") -or
            (Test-Path -LiteralPath "$transactionDestination.backup-$transactionNonce")) {
            throw 'Install self-test failed: backup-move failure did not preserve the destination and remove staging.'
        }

        $pathDestination = Join-Path $testRoot 'future-bin'
        $oldUserEntry = Join-Path $testRoot 'old-user-bin'
        $proposedUserPath = Get-ProposedUserPath `
            -ExistingUserPath "$oldUserEntry$([IO.Path]::PathSeparator)$pathDestination" `
            -CliDirectory $pathDestination
        $proposedEntries = @($proposedUserPath -split [IO.Path]::PathSeparator)
        if ($proposedEntries.Count -ne 2 -or
            (Get-PathComparisonKey $proposedEntries[0]) -ne (Get-PathComparisonKey $pathDestination) -or
            (Get-PathComparisonKey $proposedEntries[1]) -ne (Get-PathComparisonKey $oldUserEntry)) {
            throw 'Install self-test failed: the destination was not moved to the front of the proposed user PATH.'
        }
        $machineEntry = Join-Path $testRoot 'machine-bin'
        $projectedPath = Get-ProjectedPersistentPath `
            -MachinePath $machineEntry `
            -UserPath $proposedUserPath
        $projectedEntries = @($projectedPath -split [IO.Path]::PathSeparator)
        if ($projectedEntries.Count -ne 3 -or
            (Get-PathComparisonKey $projectedEntries[0]) -ne (Get-PathComparisonKey $machineEntry) -or
            (Get-PathComparisonKey $projectedEntries[1]) -ne (Get-PathComparisonKey $pathDestination)) {
            throw 'Install self-test failed: projected persistent PATH did not preserve Machine + proposed User ordering.'
        }

        $rollbackA = [pscustomobject]@{
            Destination = Join-Path $testRoot 'rollback-a-current'
            Backup = Join-Path $testRoot 'rollback-a-missing-backup'
            HadOriginal = $true
        }
        $rollbackB = [pscustomobject]@{
            Destination = Join-Path $testRoot 'rollback-b-current'
            Backup = Join-Path $testRoot 'rollback-b-backup'
            HadOriginal = $true
        }
        Set-Content -LiteralPath $rollbackA.Destination -Value 'new-a' -NoNewline -Encoding utf8
        Set-Content -LiteralPath $rollbackB.Destination -Value 'new-b' -NoNewline -Encoding utf8
        Set-Content -LiteralPath $rollbackB.Backup -Value 'old-b' -NoNewline -Encoding utf8
        $rollbackErrors = @(Invoke-InstallRollback -InstallRecords @($rollbackB, $rollbackA))
        if ($rollbackErrors.Count -ne 1 -or
            (Get-Content -Raw -LiteralPath $rollbackA.Destination) -ne 'new-a' -or
            (Get-Content -Raw -LiteralPath $rollbackB.Destination) -ne 'old-b') {
            throw 'Install self-test failed: rollback did not preserve missing-backup destination and continue restoring other records.'
        }

        $cleanupDestination = Join-Path $testRoot 'committed-current'
        $cleanupBackup = Join-Path $testRoot 'committed-backup-directory'
        Set-Content -LiteralPath $cleanupDestination -Value 'committed' -NoNewline -Encoding utf8
        New-Item -ItemType Directory -Path $cleanupBackup | Out-Null
        $cleanupWarning = Remove-CommittedBackup -InstallRecord ([pscustomobject]@{
            Destination = $cleanupDestination
            Backup = $cleanupBackup
            HadOriginal = $true
        })
        if ([string]::IsNullOrWhiteSpace($cleanupWarning) -or
            -not (Test-Path -LiteralPath $cleanupDestination -PathType Leaf) -or
            -not (Test-Path -LiteralPath $cleanupBackup -PathType Container)) {
            throw 'Install self-test failed: committed backup cleanup failure affected installed data.'
        }

        $linkType = if ($IsWindows) { 'Junction' } else { 'SymbolicLink' }
        New-Item -ItemType $linkType -Path $link -Target $outside | Out-Null
        $rejected = $false
        try {
            $null = Resolve-ManagedDirectory -Path (Join-Path $link 'child') -Label 'Escaping self-test path'
        } catch {
            $rejected = $_.Exception.Message -match 'reparse point|canonical path escaped'
        }
        if (-not $rejected) {
            throw 'Install path self-test failed: a symlink/junction ancestor was not rejected.'
        }
    } finally {
        if (Test-Path -LiteralPath $link) {
            Remove-Item -LiteralPath $link -Force
        }
        $testItem = Get-Item -LiteralPath $testRoot -Force
        $prefix = $managedTemp.TrimEnd([IO.Path]::DirectorySeparatorChar) + [IO.Path]::DirectorySeparatorChar
        if (-not $testItem.PSIsContainer -or
            $testItem.Attributes -band [IO.FileAttributes]::ReparsePoint -or
            -not $testItem.FullName.StartsWith(
                $prefix,
                $(if ($IsWindows) { [StringComparison]::OrdinalIgnoreCase } else { [StringComparison]::Ordinal })
            )) {
            throw "Refusing unsafe install path self-test cleanup: $testRoot"
        }
        Remove-Item -LiteralPath $testRoot -Recurse -Force
    }
    Write-Host 'Install self-test passed: path/hash drift rejection, full staging rollback, persistent PATH ordering, aggregate rollback, and post-commit backup cleanup isolation.'
}

if ($SelfTest) {
    Invoke-InstallPathSelfTest
    return
}

if (-not $Yes) {
    throw 'Installation replaces the managed rayman executable and canonical SKILL.md. Re-run with -Yes after reviewing the destination paths.'
}

# Fail before any installation write when this shell would invoke an alias or
# function instead of the installed executable.
$existingCommands = @(Get-Command rayman -All -ErrorAction SilentlyContinue)
if ($existingCommands.Count -gt 0 -and $existingCommands[0].CommandType -ne 'Application') {
    throw "PowerShell resolves rayman to $($existingCommands[0].CommandType) '$($existingCommands[0].Name)'. Remove that alias/function before installation so the executable can be verified."
}
$cargoApplication = Resolve-RequiredApplication -Name 'cargo' -Label 'Cargo executable'
$cargoApplicationHash = (Get-FileHash -LiteralPath $cargoApplication -Algorithm SHA256).Hash.ToLowerInvariant()

$resolvedBinDirectory = Resolve-ManagedDirectory -Path $BinDirectory -Label 'CLI directory'
$resolvedSkillDirectory = Resolve-ManagedDirectory -Path $SkillDirectory -Label 'Skill directory'
$destinationCli = Join-Path $resolvedBinDirectory $artifactName
$destinationSkill = Join-Path $resolvedSkillDirectory 'SKILL.md'
Assert-ReplaceableFile -Path $destinationCli -Label 'CLI'
Assert-ReplaceableFile -Path $destinationSkill -Label 'Skill'

Push-Location $repoRoot
try {
    Invoke-NativeChecked -FilePath $cargoApplication -Arguments @('build', '--locked', '--release', '-p', 'rayman')
    $artifact = (Resolve-Path -LiteralPath (Join-Path 'target/release' $artifactName)).Path
    $canonicalSkill = (Resolve-Path -LiteralPath 'SKILL.md').Path

    # doctor verifies the current workspace binding. Installation is the one
    # operation authorized to refresh this ignored operational state.
    New-Item -ItemType Directory -Path '.RaymanCodingSkill' -Force | Out-Null
    $skillHash = (Get-FileHash -LiteralPath $canonicalSkill -Algorithm SHA256).Hash.ToLowerInvariant()
    @(
        'skill: raymancodingskill'
        'enabled: true'
        'skill_file: SKILL.md'
        "skill_sha256: $skillHash"
    ) | Set-Content -LiteralPath '.RaymanCodingSkill/workspace_skill.yaml' -Encoding utf8

    $originalPath = $env:PATH
    try {
        # Pre-install proof: clean source, locked isolated rebuild, canonical skill,
        # and the exact artifact that will be copied.
        $artifactHashBeforeVerification = (Get-FileHash -LiteralPath $artifact -Algorithm SHA256).Hash.ToLowerInvariant()
        $skillHashBeforeVerification = (Get-FileHash -LiteralPath $canonicalSkill -Algorithm SHA256).Hash.ToLowerInvariant()
        $env:PATH = "$(Split-Path -Parent $artifact)$([IO.Path]::PathSeparator)$originalPath"
        & './scripts/verify-release-contract.ps1' `
            -CliPath $artifact `
            -ReferenceCliPath $artifact `
            -SkillPath $canonicalSkill `
            -RequireSourceFresh

        Assert-ExpectedFileHash -Path $artifact -ExpectedHash $artifactHashBeforeVerification -Label 'Source-fresh verified artifact'
        Assert-ExpectedFileHash -Path $canonicalSkill -ExpectedHash $skillHashBeforeVerification -Label 'Source-fresh verified canonical skill'
        $verifiedArtifactHash = $artifactHashBeforeVerification
        $verifiedSkillHash = $skillHashBeforeVerification

        $nonce = [Guid]::NewGuid().ToString('N')
        $installed = @()
        $oldUserPath = $null
        $proposedUserPath = $null
        $pathMutationAttempted = $false
        try {
            $installed += Install-FileWithRollback -Source $artifact -Destination $destinationCli -Nonce $nonce -ExpectedHash $verifiedArtifactHash
            $installed += Install-FileWithRollback -Source $canonicalSkill -Destination $destinationSkill -Nonce $nonce -ExpectedHash $verifiedSkillHash

            Assert-ExpectedFileHash -Path $artifact -ExpectedHash $verifiedArtifactHash -Label 'Verified artifact before post-install check'
            Assert-ExpectedFileHash -Path $canonicalSkill -ExpectedHash $verifiedSkillHash -Label 'Verified skill before post-install check'
            Assert-ExpectedFileHash -Path $destinationCli -ExpectedHash $verifiedArtifactHash -Label 'Installed CLI'
            Assert-ExpectedFileHash -Path $destinationSkill -ExpectedHash $verifiedSkillHash -Label 'Installed skill'

            if ($AddToUserPath) {
                $oldUserPath = [Environment]::GetEnvironmentVariable('Path', 'User')
                $proposedUserPath = Get-ProposedUserPath `
                    -ExistingUserPath $oldUserPath `
                    -CliDirectory $resolvedBinDirectory
                $machinePath = [Environment]::GetEnvironmentVariable('Path', 'Machine')
                $projectedPersistentPath = Get-ProjectedPersistentPath `
                    -MachinePath $machinePath `
                    -UserPath $proposedUserPath

                # Persist inside the transaction, then verify the exact ordering a
                # future Windows process receives: Machine PATH + proposed User PATH.
                # Do not prepend the destination to the process PATH here; doing so
                # would make -RequirePath tautological and miss an older machine CLI.
                $pathMutationAttempted = $true
                [Environment]::SetEnvironmentVariable('Path', $proposedUserPath, 'User')
                $persistedUserPath = [Environment]::GetEnvironmentVariable('Path', 'User')
                if (-not [string]::Equals($persistedUserPath, $proposedUserPath, [StringComparison]::Ordinal)) {
                    throw 'The persisted Windows user PATH differs from the proposed transactional value.'
                }
                $env:PATH = $projectedPersistentPath
            } else {
                # Without an authorized persistent PATH change, installation is valid
                # only when this shell already resolves the destination first.
                $env:PATH = $originalPath
            }

            & './scripts/verify-release-contract.ps1' `
                -CliPath $destinationCli `
                -ReferenceCliPath $artifact `
                -SkillPath $destinationSkill `
                -RequirePath

            Assert-ExpectedFileHash -Path $artifact -ExpectedHash $verifiedArtifactHash -Label 'Verified artifact after post-install check'
            Assert-ExpectedFileHash -Path $canonicalSkill -ExpectedHash $verifiedSkillHash -Label 'Verified skill after post-install check'
            Assert-ExpectedFileHash -Path $destinationCli -ExpectedHash $verifiedArtifactHash -Label 'Installed CLI after post-install check'
            Assert-ExpectedFileHash -Path $destinationSkill -ExpectedHash $verifiedSkillHash -Label 'Installed skill after post-install check'
            if ((Get-FileHash -LiteralPath $cargoApplication -Algorithm SHA256).Hash.ToLowerInvariant() -ne
                $cargoApplicationHash) {
                throw 'Cargo executable identity changed during installation.'
            }
            if ($AddToUserPath) {
                $persistedUserPath = [Environment]::GetEnvironmentVariable('Path', 'User')
                if (-not [string]::Equals($persistedUserPath, $proposedUserPath, [StringComparison]::Ordinal)) {
                    throw 'The persisted Windows user PATH changed during post-install verification.'
                }
            }
        } catch {
            $installFailure = $_.Exception.Message
            $recoveryErrors = @()
            if ($pathMutationAttempted) {
                try {
                    [Environment]::SetEnvironmentVariable('Path', $oldUserPath, 'User')
                    $restoredUserPath = [Environment]::GetEnvironmentVariable('Path', 'User')
                    if (-not [string]::Equals($restoredUserPath, $oldUserPath, [StringComparison]::Ordinal)) {
                        throw 'restored user PATH does not equal its pre-install value'
                    }
                } catch {
                    $recoveryErrors += "unable to restore Windows user PATH: $($_.Exception.Message)"
                }
            }
            $recoveryErrors += @(Invoke-InstallRollback -InstallRecords $installed)
            if ($recoveryErrors.Count -gt 0) {
                throw "Installation failed: $installFailure`nRollback was incomplete; retained backup/current/PATH state requires review:`n$($recoveryErrors -join "`n")"
            }
            throw $installFailure
        }

        # Verification success is the commit point. Backup cleanup is deliberately
        # outside the rollback catch: once any old backup has been destroyed, a
        # later cleanup failure must never delete the committed new installation.
        foreach ($record in $installed) {
            $cleanupWarning = Remove-CommittedBackup -InstallRecord $record
            if (-not [string]::IsNullOrWhiteSpace($cleanupWarning)) {
                Write-Warning $cleanupWarning
            }
        }
    } finally {
        $env:PATH = $originalPath
    }
} finally {
    Pop-Location
}

Write-Host 'RaymanCodingSkill installation verified.'
Write-Host "  CLI: $destinationCli"
Write-Host "  Skill: $destinationSkill"
if ($AddToUserPath) {
    Write-Host "  Persistent user PATH: verified with '$resolvedBinDirectory' first in the user segment"
} else {
    Write-Host '  Persistent PATH: unchanged; current effective PATH identity was verified'
}
