[CmdletBinding(DefaultParameterSetName = 'Repair')]
param(
    [Parameter(ParameterSetName = 'Repair')]
    [string]$ProfilePath = $PROFILE.CurrentUserCurrentHost,

    [Parameter(ParameterSetName = 'Repair')]
    [switch]$Check,

    [Parameter(ParameterSetName = 'Repair')]
    [switch]$Yes,

    [Parameter(Mandatory = $true, ParameterSetName = 'SelfTest')]
    [switch]$SelfTest
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

if ($PSVersionTable.PSVersion.Major -lt 7) {
    throw 'repair-rayman-powershell-profile.ps1 requires PowerShell 7+.'
}

$legacyFunction = @"
function rayman { param([Parameter(ValueFromRemainingArguments=`$true)] `$Args); `$cwd = Get-Location; while (`$cwd) { `$candidate = Join-Path `$cwd '.Rayman\rayman.ps1'; if (Test-Path `$candidate) { & `$candidate @Args; return }; `$parent = Split-Path `$cwd -Parent; if (`$parent -eq `$cwd.Path) { break }; `$cwd = Get-Item `$parent } }
"@

function Normalize-FunctionText {
    param([string]$Text)

    return (($Text -replace '\s+', ' ').Trim())
}

function Get-RaymanFunctionExtent {
    param([string]$Text, [string]$Label)

    $tokens = $null
    $parseErrors = $null
    $ast = [Management.Automation.Language.Parser]::ParseInput(
        $Text,
        [ref]$tokens,
        [ref]$parseErrors
    )
    if (@($parseErrors).Count -gt 0) {
        throw "$Label contains PowerShell parse errors; refusing to edit it."
    }
    $functions = @($ast.FindAll({
        param($node)
        $node -is [Management.Automation.Language.FunctionDefinitionAst] -and
            $node.Name -eq 'rayman'
    }, $true))
    if ($functions.Count -eq 0) {
        return $null
    }
    if ($functions.Count -ne 1) {
        throw "$Label defines rayman more than once; refusing ambiguous migration."
    }
    return $functions[0].Extent
}

function Get-ProfileEncoding {
    param([byte[]]$Bytes)

    if ($Bytes.Length -ge 3 -and $Bytes[0] -eq 0xef -and $Bytes[1] -eq 0xbb -and $Bytes[2] -eq 0xbf) {
        # throwOnInvalidBytes here too. A valid BOM says nothing about the bytes
        # that follow it: a BOM-prefixed file later appended to by a legacy
        # code-page tool decodes lossily and gets rewritten as mojibake.
        return [Text.UTF8Encoding]::new($true, $true)
    }
    if ($Bytes.Length -ge 2 -and $Bytes[0] -eq 0xff -and $Bytes[1] -eq 0xfe) {
        return [Text.UnicodeEncoding]::new($false, $true)
    }
    if ($Bytes.Length -ge 2 -and $Bytes[0] -eq 0xfe -and $Bytes[1] -eq 0xff) {
        return [Text.UnicodeEncoding]::new($true, $true)
    }
    # throwOnInvalidBytes, like the UTF-16 branches above. A BOM-less profile in a
    # legacy code page would otherwise decode with U+FFFD replacements and get
    # rewritten as mojibake, after which the backup is deleted and the run reports
    # success. Refusing to touch a file we cannot read losslessly is the only safe
    # outcome for user-authored code.
    return [Text.UTF8Encoding]::new($false, $true)
}

function Assert-LosslessDecode {
    param(
        [byte[]]$Bytes,
        [Text.Encoding]$Encoding,
        [string]$Text,
        [string]$Path
    )

    # ReadAllText consumes the byte-order mark, so compare against the body only.
    $preamble = $Encoding.GetPreamble()
    $body = $Bytes
    if ($preamble.Length -gt 0 -and $Bytes.Length -ge $preamble.Length) {
        $hasPreamble = $true
        for ($i = 0; $i -lt $preamble.Length; $i++) {
            if ($Bytes[$i] -ne $preamble[$i]) { $hasPreamble = $false; break }
        }
        if ($hasPreamble) {
            $body = [byte[]]@($Bytes | Select-Object -Skip $preamble.Length)
        }
    }
    $roundTrip = $Encoding.GetBytes($Text)
    if (-not [Linq.Enumerable]::SequenceEqual([byte[]]$body, [byte[]]$roundTrip)) {
        throw "PowerShell profile is not valid UTF-8 or UTF-16 and cannot be rewritten without corrupting it; remove the legacy rayman function by hand, or re-save the profile as UTF-8 first: $Path"
    }
}

function Get-LegacyProfileMigration {
    param([string]$Path)

    $item = Get-Item -LiteralPath $Path -Force -ErrorAction SilentlyContinue
    if ($null -eq $item) {
        return [pscustomobject]@{ Needed = $false; Path = [IO.Path]::GetFullPath($Path) }
    }
    if ($item.PSIsContainer -or $item.Attributes -band [IO.FileAttributes]::ReparsePoint) {
        throw "PowerShell profile must be a regular file: $Path"
    }
    $resolved = (Resolve-Path -LiteralPath $Path).Path
    $bytes = [IO.File]::ReadAllBytes($resolved)
    $encoding = Get-ProfileEncoding -Bytes $bytes
    try {
        $text = [IO.File]::ReadAllText($resolved, $encoding)
    } catch {
        throw "PowerShell profile is not valid UTF-8 or UTF-16 and cannot be rewritten without corrupting it; remove the legacy rayman function by hand, or re-save the profile as UTF-8 first: $resolved"
    }
    # throwOnInvalidBytes is not sufficient on its own: UnicodeEncoding accepts an odd
    # byte count and lone surrogates without throwing, so the UTF-16 branches would still
    # decode lossily. Re-encoding and comparing is the only check that covers every branch
    # — if the bytes do not round-trip, rewriting this file would silently destroy content.
    Assert-LosslessDecode -Bytes $bytes -Encoding $encoding -Text $text -Path $resolved
    $extent = Get-RaymanFunctionExtent -Text $text -Label $resolved
    if ($null -eq $extent) {
        return [pscustomobject]@{ Needed = $false; Path = $resolved }
    }
    if ((Normalize-FunctionText $extent.Text) -cne (Normalize-FunctionText $legacyFunction)) {
        throw "PowerShell profile defines a non-legacy rayman function; refusing to remove user-authored code: $resolved"
    }
    $updated = $text.Remove($extent.StartOffset, $extent.EndOffset - $extent.StartOffset)
    if ([string]::IsNullOrWhiteSpace($updated)) {
        $updated = ''
    }
    return [pscustomobject]@{
        Needed = $true
        Path = $resolved
        OriginalHash = (Get-FileHash -LiteralPath $resolved -Algorithm SHA256).Hash
        Updated = $updated
        Encoding = $encoding
    }
}

function Invoke-LegacyProfileMigration {
    param([string]$Path, [switch]$ConfirmWrite)

    $migration = Get-LegacyProfileMigration -Path $Path
    if (-not $migration.Needed) {
        Write-Host "PowerShell profile check passed: no legacy rayman function at $($migration.Path)"
        return
    }
    if (-not $ConfirmWrite) {
        throw "Legacy rayman profile function detected at $($migration.Path). Re-run with -Yes to remove only that exact function."
    }

    $nonce = [Guid]::NewGuid().ToString('N')
    $staged = "$($migration.Path).rayman-migrate-$nonce"
    $backup = "$($migration.Path).rayman-backup-$nonce"
    try {
        [IO.File]::WriteAllText($staged, $migration.Updated, $migration.Encoding)
        if ((Get-FileHash -LiteralPath $migration.Path -Algorithm SHA256).Hash -ne $migration.OriginalHash) {
            throw 'PowerShell profile changed after inspection; refusing to overwrite concurrent edits.'
        }
        Move-Item -LiteralPath $migration.Path -Destination $backup
        Move-Item -LiteralPath $staged -Destination $migration.Path

        $verified = Get-LegacyProfileMigration -Path $migration.Path
        if ($verified.Needed) {
            throw 'Legacy rayman function remained after migration.'
        }
        Remove-Item -LiteralPath $backup -Force
    } catch {
        $failure = $_.Exception.Message
        if (Test-Path -LiteralPath $backup -PathType Leaf) {
            Remove-Item -LiteralPath $migration.Path -Force -ErrorAction SilentlyContinue
            Move-Item -LiteralPath $backup -Destination $migration.Path
        }
        Remove-Item -LiteralPath $staged -Force -ErrorAction SilentlyContinue
        throw "PowerShell profile migration failed and was rolled back: $failure"
    }
    Write-Host "Removed the exact legacy rayman profile function: $($migration.Path)"
}

# Create a managed directory one component at a time, refusing any ancestor that
# is a symlink/junction — the same rule the sibling audit script applies to this
# very directory. `New-Item -Force` traverses an unchecked reparse point before
# the caller ever sees the path, which would put self-test writes outside the
# workspace.
function New-RealManagedDirectory {
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
        if (-not (Test-Path -LiteralPath $current)) {
            New-Item -ItemType Directory -Path $current | Out-Null
        }
        $item = Get-Item -LiteralPath $current -Force
        if (-not $item.PSIsContainer -or
            $item.Attributes -band [IO.FileAttributes]::ReparsePoint) {
            throw "$Label ancestor must be a real directory: $current"
        }
    }
    return (Resolve-Path -LiteralPath $fullPath).Path
}

function Invoke-SelfTest {
    $repoRoot = Split-Path -Parent $PSScriptRoot
    $managedTemp = New-RealManagedDirectory `
        -Path (Join-Path $repoRoot '.RaymanCodingSkill/tmp') `
        -Label 'Managed self-test temp'
    $testRoot = New-RealManagedDirectory `
        -Path (Join-Path $managedTemp ("profile-migration-selftest-" + [Guid]::NewGuid().ToString('N'))) `
        -Label 'Managed self-test run'
    try {
        $legacy = Join-Path $testRoot 'legacy.ps1'
        $mixed = Join-Path $testRoot 'mixed.ps1'
        $custom = Join-Path $testRoot 'custom.ps1'
        [IO.File]::WriteAllText($legacy, $legacyFunction, [Text.UTF8Encoding]::new($false))
        [IO.File]::WriteAllText($mixed, "`$global:PreserveMe = 7`r`n$legacyFunction`r`n", [Text.UTF8Encoding]::new($false))
        [IO.File]::WriteAllText($custom, 'function rayman { param($Args) & rayman.exe @Args }', [Text.UTF8Encoding]::new($false))

        Invoke-LegacyProfileMigration -Path $legacy -ConfirmWrite
        Invoke-LegacyProfileMigration -Path $mixed -ConfirmWrite
        if ((Get-Content -Raw -LiteralPath $mixed) -notmatch '\$global:PreserveMe = 7') {
            throw 'Profile migration self-test did not preserve unrelated profile content.'
        }
        $customRejected = $false
        try {
            Invoke-LegacyProfileMigration -Path $custom -ConfirmWrite
        } catch {
            $customRejected = $_.Exception.Message -match 'non-legacy rayman function'
        }
        if (-not $customRejected -or (Get-Content -Raw -LiteralPath $custom) -notmatch 'rayman\.exe') {
            throw 'Profile migration self-test did not preserve a custom rayman function.'
        }

        # A BOM-less profile in a legacy code page must be refused, not decoded
        # lossily and rewritten as mojibake with the backup deleted.
        $legacyCodePage = Join-Path $testRoot 'legacy-codepage.ps1'
        $nonAscii = "# " + [char]0x4E2D + [char]0x6587 + "`r`n" + '$global:Keep = 1' + "`r`n"
        $gbk = [Text.Encoding]::GetEncoding(936)
        [IO.File]::WriteAllBytes($legacyCodePage, $gbk.GetBytes($nonAscii + $legacyFunction))
        $originalBytes = [IO.File]::ReadAllBytes($legacyCodePage)
        $encodingRejected = $false
        try {
            Invoke-LegacyProfileMigration -Path $legacyCodePage -ConfirmWrite
        } catch {
            $encodingRejected = $_.Exception.Message -match 'not valid UTF-8 or UTF-16'
        }
        if (-not $encodingRejected) {
            throw 'Profile migration self-test did not refuse a non-UTF-8 profile.'
        }
        if (-not [Linq.Enumerable]::SequenceEqual($originalBytes, [IO.File]::ReadAllBytes($legacyCodePage))) {
            throw 'Profile migration self-test modified a non-UTF-8 profile instead of leaving it untouched.'
        }

        # A valid BOM says nothing about the bytes after it. Covering only the
        # BOM-less case is how the same fail-open survived one round of fixing.
        $bomCodePage = Join-Path $testRoot 'bom-codepage.ps1'
        $bomBytes = [byte[]](@(0xef, 0xbb, 0xbf) + $gbk.GetBytes($nonAscii + $legacyFunction))
        [IO.File]::WriteAllBytes($bomCodePage, $bomBytes)
        $bomRejected = $false
        try {
            Invoke-LegacyProfileMigration -Path $bomCodePage -ConfirmWrite
        } catch {
            $bomRejected = $_.Exception.Message -match 'not valid UTF-8 or UTF-16'
        }
        if (-not $bomRejected) {
            throw 'Profile migration self-test did not refuse a BOM-prefixed profile with invalid UTF-8 bytes.'
        }
        if (-not [Linq.Enumerable]::SequenceEqual($bomBytes, [IO.File]::ReadAllBytes($bomCodePage))) {
            throw 'Profile migration self-test modified a BOM-prefixed non-UTF-8 profile instead of leaving it untouched.'
        }

        # UnicodeEncoding does not throw on an odd byte count or a lone surrogate, so the
        # UTF-16 branches were fail-open until the round-trip check. Covering only UTF-8 is
        # how the same hole survived a round of fixing.
        $utf16 = [Text.UnicodeEncoding]::new($false, $true)
        foreach ($case in @(
                @{ Name = 'odd-length'; Tail = [byte[]]@(0x41) },
                @{ Name = 'lone-surrogate'; Tail = [byte[]]@(0x00, 0xd8) }
            )) {
            $broken = Join-Path $testRoot ("utf16-" + $case.Name + ".ps1")
            $brokenBytes = [byte[]](@(0xff, 0xfe) +
                $utf16.GetBytes($nonAscii + $legacyFunction) + $case.Tail)
            [IO.File]::WriteAllBytes($broken, $brokenBytes)
            $utf16Rejected = $false
            try {
                Invoke-LegacyProfileMigration -Path $broken -ConfirmWrite
            } catch {
                $utf16Rejected = $_.Exception.Message -match 'not valid UTF-8 or UTF-16'
            }
            if (-not $utf16Rejected) {
                throw "Profile migration self-test did not refuse a malformed UTF-16 profile ($($case.Name))."
            }
            if (-not [Linq.Enumerable]::SequenceEqual($brokenBytes, [IO.File]::ReadAllBytes($broken))) {
                throw "Profile migration self-test modified a malformed UTF-16 profile ($($case.Name))."
            }
        }
    } finally {
        if (Test-Path -LiteralPath $testRoot) {
            $item = Get-Item -LiteralPath $testRoot -Force
            $prefix = [IO.Path]::GetFullPath($managedTemp).TrimEnd([IO.Path]::DirectorySeparatorChar) + [IO.Path]::DirectorySeparatorChar
            if (-not $item.PSIsContainer -or
                $item.Attributes -band [IO.FileAttributes]::ReparsePoint -or
                -not $item.FullName.StartsWith($prefix, $(if ($IsWindows) { [StringComparison]::OrdinalIgnoreCase } else { [StringComparison]::Ordinal }))) {
                throw "Refusing unsafe profile migration self-test cleanup: $testRoot"
            }
            Remove-Item -LiteralPath $testRoot -Recurse -Force
        }
    }
    Write-Host 'PowerShell profile migration self-test passed.'
}

if ($SelfTest) {
    Invoke-SelfTest
    return
}

if ($Check) {
    $migration = Get-LegacyProfileMigration -Path $ProfilePath
    if ($migration.Needed) {
        throw "Legacy rayman profile function detected at $($migration.Path)."
    }
    Write-Host "PowerShell profile check passed: no legacy rayman function at $($migration.Path)"
    return
}

Invoke-LegacyProfileMigration -Path $ProfilePath -ConfirmWrite:$Yes
