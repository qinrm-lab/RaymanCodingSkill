[CmdletBinding(DefaultParameterSetName = 'Check')]
param(
    [Parameter(Mandatory = $true, ParameterSetName = 'SelfTest')]
    [switch]$SelfTest,

    [Parameter(Mandatory = $true, ParameterSetName = 'Check')]
    [switch]$Check,

    [Parameter(Mandatory = $true, ParameterSetName = 'Apply')]
    [switch]$Yes
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

if ($PSVersionTable.PSVersion.Major -lt 7) {
    throw 'configure-codex-validation-temp.ps1 requires PowerShell 7+. Run it with pwsh, not Windows PowerShell.'
}

$script:DataRoot = [IO.Path]::GetFullPath('E:\codex-sandbox')
$script:ProcessTempRoot = [IO.Path]::GetFullPath(
    'E:\codex-sandbox\temp'
)
$script:ValidationTempRoot = [IO.Path]::GetFullPath(
    'E:\codex-sandbox\rayman-validation'
)
$script:ManagedEnvironment = [ordered]@{
    TEMP = $script:ProcessTempRoot
    TMP = $script:ProcessTempRoot
    TMPDIR = $script:ProcessTempRoot
    RAYMAN_VALIDATION_TEMP_ROOT = $script:ValidationTempRoot
}

function Get-BytesSha256 {
    param([Parameter(Mandatory = $true)][byte[]]$Bytes)

    return [Convert]::ToHexString(
        [Security.Cryptography.SHA256]::HashData($Bytes)
    ).ToLowerInvariant()
}

function Get-FileSha256 {
    param([Parameter(Mandatory = $true)][string]$Path)

    return Get-BytesSha256 -Bytes ([IO.File]::ReadAllBytes($Path))
}

function Read-StrictUtf8Document {
    param([Parameter(Mandatory = $true)][string]$Path)

    $resolved = [IO.Path]::GetFullPath($Path)
    if (-not (Test-Path -LiteralPath $resolved -PathType Leaf)) {
        throw "Codex config is missing: $resolved"
    }

    $item = Get-Item -LiteralPath $resolved -Force
    if (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
        throw "Codex config must not be a reparse point: $resolved"
    }

    $bytes = [IO.File]::ReadAllBytes($resolved)
    $hasBom = $bytes.Length -ge 3 -and
        $bytes[0] -eq 0xEF -and
        $bytes[1] -eq 0xBB -and
        $bytes[2] -eq 0xBF
    $offset = if ($hasBom) { 3 } else { 0 }
    $payload = [byte[]]::new($bytes.Length - $offset)
    if ($payload.Length -gt 0) {
        [Array]::Copy($bytes, $offset, $payload, 0, $payload.Length)
    }
    try {
        $text = [Text.UTF8Encoding]::new($false, $true).GetString($payload)
    } catch {
        throw "Codex config must be strict UTF-8: $resolved ($($_.Exception.Message))"
    }
    if ($text.Contains([char]0)) {
        throw "Codex config contains a NUL byte: $resolved"
    }

    $withoutCrLf = $text.Replace("`r`n", '')
    if ($withoutCrLf.Contains("`r")) {
        throw "Codex config contains a bare carriage return: $resolved"
    }
    if ($text.Contains("`r`n") -and $withoutCrLf.Contains("`n")) {
        throw "Codex config mixes CRLF and LF line endings: $resolved"
    }
    $newLine = if ($text.Contains("`r`n")) {
        "`r`n"
    } elseif ($text.Contains("`n")) {
        "`n"
    } else {
        "`r`n"
    }

    return [pscustomobject]@{
        Path = $resolved
        Bytes = $bytes
        HasBom = $hasBom
        Text = $text
        NewLine = $newLine
        Sha256 = Get-BytesSha256 -Bytes $bytes
    }
}

function ConvertTo-StrictUtf8Bytes {
    param(
        [Parameter(Mandatory = $true)][AllowEmptyString()][string]$Text,
        [Parameter(Mandatory = $true)][bool]$HasBom
    )

    $payload = [Text.UTF8Encoding]::new($false, $true).GetBytes($Text)
    if (-not $HasBom) {
        return $payload
    }
    $bytes = [byte[]]::new($payload.Length + 3)
    $bytes[0] = 0xEF
    $bytes[1] = 0xBB
    $bytes[2] = 0xBF
    if ($payload.Length -gt 0) {
        [Array]::Copy($payload, 0, $bytes, 3, $payload.Length)
    }
    return $bytes
}

function ConvertTo-TomlLines {
    param([Parameter(Mandatory = $true)][AllowEmptyString()][string]$Text)

    $lines = [Collections.Generic.List[object]]::new()
    $cursor = 0
    while ($cursor -lt $Text.Length) {
        $lf = $Text.IndexOf("`n", $cursor, [StringComparison]::Ordinal)
        if ($lf -lt 0) {
            $lines.Add([pscustomobject]@{
                Content = $Text.Substring($cursor)
                Ending = ''
            })
            $cursor = $Text.Length
            continue
        }
        $contentEnd = $lf
        $ending = "`n"
        if ($lf -gt $cursor -and $Text[$lf - 1] -eq "`r") {
            $contentEnd = $lf - 1
            $ending = "`r`n"
        }
        $lines.Add([pscustomobject]@{
            Content = $Text.Substring($cursor, $contentEnd - $cursor)
            Ending = $ending
        })
        $cursor = $lf + 1
    }
    return ,$lines
}

function ConvertFrom-TomlLines {
    param([Parameter(Mandatory = $true)]$Lines)

    $builder = [Text.StringBuilder]::new()
    foreach ($line in $Lines) {
        [void]$builder.Append([string]$line.Content)
        [void]$builder.Append([string]$line.Ending)
    }
    return $builder.ToString()
}

function Split-TomlComment {
    param([Parameter(Mandatory = $true)][AllowEmptyString()][string]$Line)

    $state = 'plain'
    $escaped = $false
    for ($index = 0; $index -lt $Line.Length; $index++) {
        $character = $Line[$index]
        if ($state -eq 'basic') {
            if ($escaped) {
                $escaped = $false
            } elseif ($character -eq '\') {
                $escaped = $true
            } elseif ($character -eq '"') {
                $state = 'plain'
            }
            continue
        }
        if ($state -eq 'literal') {
            if ($character -eq "'") {
                $state = 'plain'
            }
            continue
        }
        if ($character -eq '"') {
            $state = 'basic'
        } elseif ($character -eq "'") {
            $state = 'literal'
        } elseif ($character -eq '#') {
            return [pscustomobject]@{
                Code = $Line.Substring(0, $index)
                CommentIndex = $index
            }
        }
    }
    if ($state -ne 'plain' -or $escaped) {
        throw "Unsupported or unterminated TOML string: $Line"
    }
    return [pscustomobject]@{
        Code = $Line
        CommentIndex = $Line.Length
    }
}

function Read-TomlQuotedToken {
    param(
        [Parameter(Mandatory = $true)][string]$Text,
        [Parameter(Mandatory = $true)][int]$Start
    )

    if ($Start -ge $Text.Length -or
        ($Text[$Start] -ne '"' -and $Text[$Start] -ne "'")) {
        throw 'Expected a quoted TOML token.'
    }
    $quote = $Text[$Start]
    $builder = [Text.StringBuilder]::new()
    $index = $Start + 1
    while ($index -lt $Text.Length) {
        $character = $Text[$index]
        if ($character -eq $quote) {
            return [pscustomobject]@{
                Value = $builder.ToString()
                End = $index + 1
                Quote = [string]$quote
            }
        }
        if ($quote -eq "'" -or $character -ne '\') {
            [void]$builder.Append($character)
            $index++
            continue
        }

        $index++
        if ($index -ge $Text.Length) {
            throw 'TOML basic string ends with an incomplete escape.'
        }
        $escape = $Text[$index]
        switch ($escape) {
            '"' { [void]$builder.Append('"') }
            '\' { [void]$builder.Append('\') }
            'b' { [void]$builder.Append("`b") }
            't' { [void]$builder.Append("`t") }
            'n' { [void]$builder.Append("`n") }
            'f' { [void]$builder.Append("`f") }
            'r' { [void]$builder.Append("`r") }
            'u' {
                if ($index + 4 -ge $Text.Length) {
                    throw 'TOML basic string has an incomplete \u escape.'
                }
                $hex = $Text.Substring($index + 1, 4)
                if ($hex -notmatch '^[0-9A-Fa-f]{4}$') {
                    throw "Invalid TOML Unicode escape: \u$hex"
                }
                $codePoint = [Convert]::ToInt32($hex, 16)
                if ($codePoint -ge 0xD800 -and $codePoint -le 0xDFFF) {
                    throw "TOML Unicode escape is a surrogate: \u$hex"
                }
                [void]$builder.Append([char]::ConvertFromUtf32($codePoint))
                $index += 4
            }
            'U' {
                if ($index + 8 -ge $Text.Length) {
                    throw 'TOML basic string has an incomplete \U escape.'
                }
                $hex = $Text.Substring($index + 1, 8)
                if ($hex -notmatch '^[0-9A-Fa-f]{8}$') {
                    throw "Invalid TOML Unicode escape: \U$hex"
                }
                $codePoint = [Convert]::ToInt32($hex, 16)
                if ($codePoint -gt 0x10FFFF -or
                    ($codePoint -ge 0xD800 -and $codePoint -le 0xDFFF)) {
                    throw "Invalid TOML Unicode scalar: \U$hex"
                }
                [void]$builder.Append([char]::ConvertFromUtf32($codePoint))
                $index += 8
            }
            default {
                throw "Unsupported TOML escape: \$escape"
            }
        }
        $index++
    }
    throw 'Unterminated quoted TOML token.'
}

function ConvertFrom-TomlKeyPath {
    param([Parameter(Mandatory = $true)][string]$Text)

    $segments = [Collections.Generic.List[string]]::new()
    $index = 0
    while ($true) {
        while ($index -lt $Text.Length -and [char]::IsWhiteSpace($Text[$index])) {
            $index++
        }
        if ($index -ge $Text.Length) {
            throw "Empty TOML key segment: $Text"
        }

        if ($Text[$index] -eq '"' -or $Text[$index] -eq "'") {
            $token = Read-TomlQuotedToken -Text $Text -Start $index
            $segments.Add([string]$token.Value)
            $index = $token.End
        } else {
            $start = $index
            while ($index -lt $Text.Length -and
                $Text[$index] -match '[A-Za-z0-9_-]') {
                $index++
            }
            if ($start -eq $index) {
                throw "Invalid bare TOML key: $Text"
            }
            $segments.Add($Text.Substring($start, $index - $start))
        }

        while ($index -lt $Text.Length -and [char]::IsWhiteSpace($Text[$index])) {
            $index++
        }
        if ($index -eq $Text.Length) {
            break
        }
        if ($Text[$index] -ne '.') {
            throw "Invalid TOML dotted key: $Text"
        }
        $index++
    }
    return $segments.ToArray()
}

function Find-TomlEquals {
    param([Parameter(Mandatory = $true)][string]$Code)

    $state = 'plain'
    $escaped = $false
    for ($index = 0; $index -lt $Code.Length; $index++) {
        $character = $Code[$index]
        if ($state -eq 'basic') {
            if ($escaped) {
                $escaped = $false
            } elseif ($character -eq '\') {
                $escaped = $true
            } elseif ($character -eq '"') {
                $state = 'plain'
            }
            continue
        }
        if ($state -eq 'literal') {
            if ($character -eq "'") {
                $state = 'plain'
            }
            continue
        }
        if ($character -eq '"') {
            $state = 'basic'
        } elseif ($character -eq "'") {
            $state = 'literal'
        } elseif ($character -eq '=') {
            return $index
        }
    }
    return -1
}

function ConvertFrom-TomlStringValue {
    param([Parameter(Mandatory = $true)][string]$Text)

    $trimmed = $Text.Trim()
    if ($trimmed.Length -lt 2 -or
        ($trimmed[0] -ne '"' -and $trimmed[0] -ne "'")) {
        throw "Expected a TOML string, found: $Text"
    }
    $token = Read-TomlQuotedToken -Text $trimmed -Start 0
    if ($token.End -ne $trimmed.Length) {
        throw "Unexpected content after TOML string: $Text"
    }
    return [string]$token.Value
}

function ConvertFrom-TomlBooleanValue {
    param([Parameter(Mandatory = $true)][string]$Text)

    $trimmed = $Text.Trim()
    if ($trimmed -ceq 'true') {
        return $true
    }
    if ($trimmed -ceq 'false') {
        return $false
    }
    throw "Expected a TOML boolean, found: $Text"
}

function New-TomlBasicString {
    param([Parameter(Mandatory = $true)][AllowEmptyString()][string]$Value)

    $escaped = $Value.Replace('\', '\\').Replace('"', '\"')
    $escaped = $escaped.Replace("`b", '\b').Replace("`t", '\t')
    $escaped = $escaped.Replace("`n", '\n').Replace("`f", '\f')
    $escaped = $escaped.Replace("`r", '\r')
    return '"' + $escaped + '"'
}

function Test-PathSegmentsEqual {
    param(
        [Parameter(Mandatory = $true)][AllowEmptyCollection()][string[]]$Actual,
        [Parameter(Mandatory = $true)][AllowEmptyCollection()][string[]]$Expected
    )

    if ($Actual.Count -ne $Expected.Count) {
        return $false
    }
    for ($index = 0; $index -lt $Actual.Count; $index++) {
        if ($Actual[$index] -cne $Expected[$index]) {
            return $false
        }
    }
    return $true
}

function Get-TomlStructure {
    param([Parameter(Mandatory = $true)]$Lines)

    $headers = [Collections.Generic.List[object]]::new()
    $assignments = [Collections.Generic.List[object]]::new()
    [string[]]$currentTable = @()

    for ($lineIndex = 0; $lineIndex -lt $Lines.Count; $lineIndex++) {
        $line = [string]$Lines[$lineIndex].Content
        $split = Split-TomlComment -Line $line
        $code = $split.Code
        $trimmed = $code.Trim()
        if ([string]::IsNullOrWhiteSpace($trimmed)) {
            continue
        }

        if ($trimmed.StartsWith('[')) {
            $isArray = $trimmed.StartsWith('[[')
            $openLength = if ($isArray) { 2 } else { 1 }
            $closeText = if ($isArray) { ']]' } else { ']' }
            if (-not $trimmed.EndsWith($closeText) -or
                $trimmed.Length -le ($openLength + $closeText.Length)) {
                throw "Malformed TOML table header at line $($lineIndex + 1): $line"
            }
            $innerLength = $trimmed.Length - $openLength - $closeText.Length
            $inner = $trimmed.Substring($openLength, $innerLength)
            [string[]]$path = @(ConvertFrom-TomlKeyPath -Text $inner)
            $strictHeader = $path.Count -gt 0 -and
                ($path[0] -ceq 'windows' -or
                 $path[0] -ceq 'permissions' -or
                 $path[0] -ceq 'shell_environment_policy')
            if ($strictHeader -and -not $isArray -and
                @($headers | Where-Object {
                    -not $_.IsArray -and
                    (Test-PathSegmentsEqual -Actual $_.Path -Expected $path)
                }).Count -gt 0) {
                throw "Duplicate TOML table at line $($lineIndex + 1): $trimmed"
            }
            $currentTable = $path
            $headers.Add([pscustomobject]@{
                LineIndex = $lineIndex
                Path = $path
                IsArray = $isArray
            })
            continue
        }

        $isRelevant = $currentTable.Count -eq 0 -or
            (Test-PathSegmentsEqual -Actual $currentTable -Expected @('windows')) -or
            ($currentTable.Count -gt 0 -and
                ($currentTable[0] -ceq 'permissions' -or
                 $currentTable[0] -ceq 'shell_environment_policy'))
        if (-not $isRelevant) {
            continue
        }

        $equals = Find-TomlEquals -Code $code
        if ($equals -lt 1) {
            throw "Expected a single-line TOML assignment at line $($lineIndex + 1): $line"
        }
        [string[]]$keyPath = @(
            ConvertFrom-TomlKeyPath -Text $code.Substring(0, $equals).Trim()
        )
        $valueStart = $equals + 1
        while ($valueStart -lt $code.Length -and
            [char]::IsWhiteSpace($code[$valueStart])) {
            $valueStart++
        }
        $valueEnd = $code.Length
        while ($valueEnd -gt $valueStart -and
            [char]::IsWhiteSpace($code[$valueEnd - 1])) {
            $valueEnd--
        }
        if ($valueStart -eq $valueEnd) {
            throw "Empty TOML value at line $($lineIndex + 1): $line"
        }
        $assignments.Add([pscustomobject]@{
            LineIndex = $lineIndex
            TablePath = [string[]]$currentTable.Clone()
            KeyPath = $keyPath
            ValueText = $code.Substring($valueStart, $valueEnd - $valueStart)
            ValueStart = $valueStart
            ValueEnd = $valueEnd
        })
    }

    return [pscustomobject]@{
        Headers = $headers
        Assignments = $assignments
    }
}

function Get-SingleAssignment {
    param(
        [Parameter(Mandatory = $true)]$Structure,
        [Parameter(Mandatory = $true)][AllowEmptyCollection()][string[]]$TablePath,
        [Parameter(Mandatory = $true)][string[]]$KeyPath,
        [Parameter(Mandatory = $true)][string]$Label,
        [switch]$Optional
    )

    $matches = @($Structure.Assignments | Where-Object {
        (Test-PathSegmentsEqual -Actual $_.TablePath -Expected $TablePath) -and
        (Test-PathSegmentsEqual -Actual $_.KeyPath -Expected $KeyPath)
    })
    if ($matches.Count -gt 1) {
        throw "Duplicate TOML assignment for $Label."
    }
    if ($matches.Count -eq 0) {
        if ($Optional) {
            return $null
        }
        throw "Missing TOML assignment for $Label."
    }
    return $matches[0]
}

function Test-FullAccessName {
    param([Parameter(Mandatory = $true)][string]$Value)

    $normalized = $Value.ToLowerInvariant() -replace '[^a-z0-9]', ''
    return $normalized.Contains('fullaccess') -or
        $normalized.Contains('dangerfullaccess')
}

function Test-PathContains {
    param(
        [Parameter(Mandatory = $true)][string]$Parent,
        [Parameter(Mandatory = $true)][string]$Child
    )

    $parentFull = [IO.Path]::GetFullPath($Parent).TrimEnd('\', '/')
    $childFull = [IO.Path]::GetFullPath($Child).TrimEnd('\', '/')
    return $childFull.Equals($parentFull, [StringComparison]::OrdinalIgnoreCase) -or
        $childFull.StartsWith(
            $parentFull + [IO.Path]::DirectorySeparatorChar,
            [StringComparison]::OrdinalIgnoreCase
        )
}

function Assert-ConfigSecurityContract {
    param([Parameter(Mandatory = $true)]$Structure)

    $defaultAssignment = Get-SingleAssignment -Structure $Structure `
        -TablePath @() -KeyPath @('default_permissions') `
        -Label 'default_permissions'
    $profile = ConvertFrom-TomlStringValue -Text $defaultAssignment.ValueText
    if ([string]::IsNullOrWhiteSpace($profile) -or
        (Test-FullAccessName -Value $profile)) {
        throw "Refusing Full access default permission profile: $profile"
    }

    $sandboxAssignment = Get-SingleAssignment -Structure $Structure `
        -TablePath @('windows') -KeyPath @('sandbox') `
        -Label 'windows.sandbox'
    $sandbox = ConvertFrom-TomlStringValue -Text $sandboxAssignment.ValueText
    if ($sandbox -cne 'elevated') {
        throw "Windows sandbox must remain narrowly elevated; found: $sandbox"
    }

    $profilePath = @('permissions', $profile)
    $extendsAssignment = Get-SingleAssignment -Structure $Structure `
        -TablePath $profilePath -KeyPath @('extends') `
        -Label "permissions.$profile.extends"
    $extends = ConvertFrom-TomlStringValue -Text $extendsAssignment.ValueText
    if (Test-FullAccessName -Value $extends) {
        throw "Refusing Full access permission inheritance: $extends"
    }
    if ($extends -cne ':workspace') {
        throw "Default permission profile must extend :workspace; found: $extends"
    }

    $rootTable = @('permissions', $profile, 'workspace_roots')
    $rootAssignments = @($Structure.Assignments | Where-Object {
        Test-PathSegmentsEqual -Actual $_.TablePath -Expected $rootTable
    })
    if ($rootAssignments.Count -eq 0) {
        throw "Default permission profile has no workspace_roots: $profile"
    }

    $enabledRoots = [Collections.Generic.List[string]]::new()
    foreach ($assignment in $rootAssignments) {
        if ($assignment.KeyPath.Count -ne 1) {
            throw "Default permission profile has a dotted workspace root key at line $($assignment.LineIndex + 1)."
        }
        if (-not (ConvertFrom-TomlBooleanValue -Text $assignment.ValueText)) {
            continue
        }
        $rootText = $assignment.KeyPath[0]
        if (-not [IO.Path]::IsPathFullyQualified($rootText)) {
            throw "Default permission profile has a relative or drive-relative writable root: $rootText"
        }
        $root = [IO.Path]::GetFullPath($rootText)
        $driveRoot = [IO.Path]::GetPathRoot($root)
        if ($root.TrimEnd('\', '/').Equals(
                $driveRoot.TrimEnd('\', '/'),
                [StringComparison]::OrdinalIgnoreCase
            )) {
            throw "Refusing a whole-drive writable root: $root"
        }
        $enabledRoots.Add($root)
    }
    $managedRoots = @(
        $script:ProcessTempRoot,
        $script:ValidationTempRoot
    )
    foreach ($managedRoot in $managedRoots) {
        if (-not (Test-PathContains -Parent $script:DataRoot `
                -Child $managedRoot)) {
            throw "Internal managed root escaped the fixed data root: $managedRoot"
        }
        $coversManagedRoot = @($enabledRoots | Where-Object {
            Test-PathContains -Parent $_ -Child $managedRoot
        }).Count -gt 0
        if (-not $coversManagedRoot) {
            throw "Default permission profile does not cover managed root: $managedRoot"
        }
    }
    if ((Test-PathContains -Parent $script:ProcessTempRoot `
            -Child $script:ValidationTempRoot) -or
        (Test-PathContains -Parent $script:ValidationTempRoot `
            -Child $script:ProcessTempRoot)) {
        throw 'Process TEMP and Rayman validation roots must be disjoint siblings.'
    }
    if (-not [IO.Path]::GetDirectoryName($script:ProcessTempRoot).Equals(
            [IO.Path]::GetDirectoryName($script:ValidationTempRoot),
            [StringComparison]::OrdinalIgnoreCase
        )) {
        throw 'Process TEMP and Rayman validation roots must share one data-root parent.'
    }

    return [pscustomobject]@{
        Profile = $profile
        WritableRoots = $enabledRoots.ToArray()
    }
}

function Get-ManagedAssignments {
    param([Parameter(Mandatory = $true)]$Structure)

    $tablePath = @('shell_environment_policy', 'set')
    $headers = @($Structure.Headers | Where-Object {
        Test-PathSegmentsEqual -Actual $_.Path -Expected $tablePath
    })
    if ($headers.Count -gt 1) {
        throw 'Duplicate [shell_environment_policy.set] tables.'
    }
    if ($headers.Count -eq 1 -and $headers[0].IsArray) {
        throw '[shell_environment_policy.set] must not be an array of tables.'
    }

    $inlineSet = Get-SingleAssignment -Structure $Structure `
        -TablePath @('shell_environment_policy') -KeyPath @('set') `
        -Label 'shell_environment_policy.set' -Optional
    if ($null -ne $inlineSet) {
        throw 'Inline shell_environment_policy.set is unsupported; use the lossless [shell_environment_policy.set] table form.'
    }
    $dottedSet = @($Structure.Assignments | Where-Object {
        $_.TablePath.Count -eq 0 -and $_.KeyPath.Count -ge 2 -and
        $_.KeyPath[0] -ceq 'shell_environment_policy' -and
        $_.KeyPath[1] -ceq 'set'
    })
    if ($dottedSet.Count -gt 0) {
        throw 'Dotted shell_environment_policy.set assignments are unsupported; use [shell_environment_policy.set].'
    }

    $assignments = @{}
    foreach ($name in $script:ManagedEnvironment.Keys) {
        $matches = @($Structure.Assignments | Where-Object {
            (Test-PathSegmentsEqual -Actual $_.TablePath -Expected $tablePath) -and
            $_.KeyPath.Count -eq 1 -and $_.KeyPath[0] -ceq $name
        })
        if ($matches.Count -gt 1) {
            throw "Duplicate shell_environment_policy.set.$name assignment."
        }
        if ($matches.Count -eq 1) {
            [void](ConvertFrom-TomlStringValue -Text $matches[0].ValueText)
            $assignments[$name] = $matches[0]
        }
    }
    return [pscustomobject]@{
        Header = if ($headers.Count -eq 1) { $headers[0] } else { $null }
        Assignments = $assignments
    }
}

function Add-TomlLines {
    param(
        [Parameter(Mandatory = $true)]$Lines,
        [Parameter(Mandatory = $true)][int]$Index,
        [Parameter(Mandatory = $true)][AllowEmptyString()][string[]]$Content,
        [Parameter(Mandatory = $true)][string]$NewLine
    )

    if ($Content.Count -eq 0) {
        return
    }
    $atEnd = $Index -eq $Lines.Count
    $hadTerminalNewLine = $Lines.Count -gt 0 -and
        -not [string]::IsNullOrEmpty([string]$Lines[$Lines.Count - 1].Ending)
    if ($atEnd -and $Lines.Count -gt 0 -and -not $hadTerminalNewLine) {
        $Lines[$Lines.Count - 1].Ending = $NewLine
    }

    for ($offset = 0; $offset -lt $Content.Count; $offset++) {
        $ending = $NewLine
        if ($atEnd -and -not $hadTerminalNewLine -and
            $offset -eq $Content.Count - 1) {
            $ending = ''
        }
        $Lines.Insert($Index + $offset, [pscustomobject]@{
            Content = $Content[$offset]
            Ending = $ending
        })
    }
}

function Get-UpdatedConfigText {
    param(
        [Parameter(Mandatory = $true)][AllowEmptyString()][string]$Text,
        [Parameter(Mandatory = $true)][string]$NewLine
    )

    $sourceLines = ConvertTo-TomlLines -Text $Text
    $lines = [Collections.Generic.List[object]]::new()
    foreach ($line in $sourceLines) {
        $lines.Add([pscustomobject]@{
            Content = [string]$line.Content
            Ending = [string]$line.Ending
        })
    }
    $structure = Get-TomlStructure -Lines $lines
    [void](Assert-ConfigSecurityContract -Structure $structure)
    $managed = Get-ManagedAssignments -Structure $structure

    foreach ($name in $script:ManagedEnvironment.Keys) {
        if (-not $managed.Assignments.ContainsKey($name)) {
            continue
        }
        $assignment = $managed.Assignments[$name]
        $currentValue = ConvertFrom-TomlStringValue -Text $assignment.ValueText
        if ($currentValue -ceq $script:ManagedEnvironment[$name]) {
            continue
        }
        $replacement = if ($assignment.ValueText.Trim().StartsWith("'")) {
            "'" + $script:ManagedEnvironment[$name] + "'"
        } else {
            New-TomlBasicString -Value $script:ManagedEnvironment[$name]
        }
        $line = [string]$lines[$assignment.LineIndex].Content
        $lines[$assignment.LineIndex].Content =
            $line.Substring(0, $assignment.ValueStart) +
            $replacement +
            $line.Substring($assignment.ValueEnd)
    }

    $missing = @($script:ManagedEnvironment.Keys | Where-Object {
        -not $managed.Assignments.ContainsKey($_)
    })
    if ($missing.Count -gt 0) {
        $content = @($missing | ForEach-Object {
            "$_ = $(New-TomlBasicString -Value $script:ManagedEnvironment[$_])"
        })
        if ($null -eq $managed.Header) {
            if ($lines.Count -gt 0 -and
                -not [string]::IsNullOrWhiteSpace(
                    [string]$lines[$lines.Count - 1].Content
                )) {
                $content = @('') + @('[shell_environment_policy.set]') + $content
            } else {
                $content = @('[shell_environment_policy.set]') + $content
            }
            Add-TomlLines -Lines $lines -Index $lines.Count `
                -Content $content -NewLine $NewLine
        } else {
            $nextHeader = @($structure.Headers | Where-Object {
                $_.LineIndex -gt $managed.Header.LineIndex
            } | Sort-Object LineIndex | Select-Object -First 1)
            $insertAt = if ($nextHeader.Count -eq 1) {
                [int]$nextHeader[0].LineIndex
            } else {
                $lines.Count
            }
            Add-TomlLines -Lines $lines -Index $insertAt `
                -Content $content -NewLine $NewLine
        }
    }

    $updated = ConvertFrom-TomlLines -Lines $lines
    $verificationLines = ConvertTo-TomlLines -Text $updated
    $verification = Get-TomlStructure -Lines $verificationLines
    [void](Assert-ConfigSecurityContract -Structure $verification)
    $verifiedManaged = Get-ManagedAssignments -Structure $verification
    foreach ($name in $script:ManagedEnvironment.Keys) {
        if (-not $verifiedManaged.Assignments.ContainsKey($name)) {
            throw "Rendered config is missing shell_environment_policy.set.$name."
        }
        $actual = ConvertFrom-TomlStringValue `
            -Text $verifiedManaged.Assignments[$name].ValueText
        if ($actual -cne $script:ManagedEnvironment[$name]) {
            throw "Rendered config has the wrong value for shell_environment_policy.set.$name."
        }
    }
    return $updated
}

function Assert-NoEveryoneWriteAcl {
    param(
        [Parameter(Mandatory = $true)][string]$Sddl,
        [Parameter(Mandatory = $true)][string]$Label
    )

    $descriptor = [Security.AccessControl.RawSecurityDescriptor]::new($Sddl)
    if ($null -eq $descriptor.DiscretionaryAcl) {
        throw "$Label has no DACL."
    }
    $writeCapableSids = @{
        'S-1-1-0' = 'Everyone'
        'S-1-5-32-545' = 'BUILTIN\Users'
    }
    $writeMask = 0x00000002 -bor 0x00000004 -bor 0x00000010 -bor
        0x00000040 -bor
        0x00000100 -bor 0x00010000 -bor 0x00040000 -bor 0x00080000 -bor
        0x10000000 -bor 0x40000000
    foreach ($ace in $descriptor.DiscretionaryAcl) {
        if ($ace -is [Security.AccessControl.QualifiedAce] -and
            $ace.AceQualifier -eq
                [Security.AccessControl.AceQualifier]::AccessAllowed -and
            $null -ne $ace.SecurityIdentifier -and
            $writeCapableSids.ContainsKey($ace.SecurityIdentifier.Value) -and
            ($ace.AccessMask -band $writeMask) -ne 0) {
            throw "$Label grants write-capable access to $($writeCapableSids[$ace.SecurityIdentifier.Value]); refusing ACL expansion."
        }
    }
}

function Assert-DirectorySecurity {
    param([Parameter(Mandatory = $true)][string]$Path)

    $resolved = [IO.Path]::GetFullPath($Path)
    if (-not (Test-Path -LiteralPath $resolved -PathType Container)) {
        throw "Required directory is missing: $resolved"
    }
    $item = Get-Item -LiteralPath $resolved -Force
    if (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
        throw "Managed validation temp directory must not be a reparse point: $resolved"
    }
    $acl = Get-Acl -LiteralPath $resolved
    Assert-NoEveryoneWriteAcl `
        -Sddl $acl.GetSecurityDescriptorSddlForm(
            [Security.AccessControl.AccessControlSections]::All
        ) `
        -Label $resolved
}

function Assert-ConfigOwnerIsCurrentPrincipal {
    param([Parameter(Mandatory = $true)][string]$Path)

    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    if ($null -eq $identity.User) {
        throw 'Cannot resolve the current Windows principal SID.'
    }
    $acl = Get-Acl -LiteralPath $Path
    $descriptor = [Security.AccessControl.RawSecurityDescriptor]::new(
        $acl.GetSecurityDescriptorSddlForm(
            [Security.AccessControl.AccessControlSections]::All
        )
    )
    if ($null -eq $descriptor.Owner -or
        $descriptor.Owner.Value -cne $identity.User.Value) {
        $owner = if ($null -eq $descriptor.Owner) {
            '<missing>'
        } else {
            $descriptor.Owner.Value
        }
        throw "Refusing to modify user config from a non-owner principal. Current=$($identity.Name) [$($identity.User.Value)] Owner=$owner. Run -Yes from the ordinary interactive owner PowerShell."
    }
}

function Invoke-DirectoryWriteProbe {
    param([Parameter(Mandatory = $true)][string]$Path)

    $probe = Join-Path $Path (
        '.rayman-validation-temp-probe-' + [Guid]::NewGuid().ToString('N')
    )
    $expected = [Security.Cryptography.RandomNumberGenerator]::GetBytes(32)
    try {
        $stream = [IO.File]::Open(
            $probe,
            [IO.FileMode]::CreateNew,
            [IO.FileAccess]::Write,
            [IO.FileShare]::None
        )
        try {
            $stream.Write($expected, 0, $expected.Length)
            $stream.Flush($true)
        } finally {
            $stream.Dispose()
        }
        $actual = [IO.File]::ReadAllBytes($probe)
        if (-not [Linq.Enumerable]::SequenceEqual(
                [byte[]]$expected,
                [byte[]]$actual
            )) {
            throw "Validation temp write probe content mismatch: $probe"
        }
    } finally {
        if (Test-Path -LiteralPath $probe -PathType Leaf) {
            Remove-Item -LiteralPath $probe -Force
        }
    }
    if (Test-Path -LiteralPath $probe) {
        throw "Validation temp write probe was not released: $probe"
    }
}

function Set-ConfigFileTransactional {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][byte[]]$OriginalBytes,
        [Parameter(Mandatory = $true)][byte[]]$CandidateBytes,
        [scriptblock]$BeforeReplace,
        [scriptblock]$AfterReplace
    )

    $resolved = [IO.Path]::GetFullPath($Path)
    $initialHash = Get-BytesSha256 -Bytes $OriginalBytes
    $candidateHash = Get-BytesSha256 -Bytes $CandidateBytes
    if ($initialHash -ceq $candidateHash) {
        return [pscustomobject]@{
            Changed = $false
            BackupPath = $null
            Sha256 = $candidateHash
        }
    }
    if ((Get-FileSha256 -Path $resolved) -cne $initialHash) {
        throw 'Codex config changed after it was read; refusing a stale update.'
    }

    $directory = Split-Path -Parent $resolved
    $leaf = Split-Path -Leaf $resolved
    $nonce = [Guid]::NewGuid().ToString('N')
    $stamp = [DateTime]::UtcNow.ToString('yyyyMMddTHHmmssfffffffZ')
    $candidatePath = Join-Path $directory ".$leaf.rayman-write.$nonce.tmp"
    $backupPath = Join-Path $directory "$leaf.rayman-backup.$stamp.$nonce.bak"
    $rollbackSpare = Join-Path $directory ".$leaf.rayman-rollback.$nonce.tmp"
    $replaced = $false
    $backupHash = $null

    try {
        $stream = [IO.File]::Open(
            $candidatePath,
            [IO.FileMode]::CreateNew,
            [IO.FileAccess]::Write,
            [IO.FileShare]::None
        )
        try {
            $stream.Write($CandidateBytes, 0, $CandidateBytes.Length)
            $stream.Flush($true)
        } finally {
            $stream.Dispose()
        }
        if ((Get-FileSha256 -Path $candidatePath) -cne $candidateHash) {
            throw 'Codex config candidate hash verification failed.'
        }

        if ($null -ne $BeforeReplace) {
            & $BeforeReplace | Out-Null
        }
        if ((Get-FileSha256 -Path $resolved) -cne $initialHash) {
            throw 'Codex config changed concurrently before replacement.'
        }

        [IO.File]::Replace($candidatePath, $resolved, $backupPath, $true)
        $replaced = $true
        $backupHash = Get-FileSha256 -Path $backupPath
        if ($backupHash -cne $initialHash) {
            throw 'Codex config changed concurrently at replacement time.'
        }
        if ((Get-FileSha256 -Path $resolved) -cne $candidateHash) {
            throw 'Codex config replacement hash verification failed.'
        }
        if ($null -ne $AfterReplace) {
            & $AfterReplace | Out-Null
        }
        if ((Get-FileSha256 -Path $resolved) -cne $candidateHash) {
            throw 'Codex config changed concurrently after replacement.'
        }

        return [pscustomobject]@{
            Changed = $true
            BackupPath = $backupPath
            Sha256 = $candidateHash
        }
    } catch {
        $primary = $_.Exception
        $rollbackError = $null
        if ($replaced -and (Test-Path -LiteralPath $backupPath -PathType Leaf)) {
            try {
                $currentHash = Get-FileSha256 -Path $resolved
                if ($currentHash -cne $candidateHash) {
                    throw 'Rollback refused because the replaced config changed again.'
                }
                if ($null -eq $backupHash) {
                    $backupHash = Get-FileSha256 -Path $backupPath
                }
                [IO.File]::Replace(
                    $backupPath,
                    $resolved,
                    $rollbackSpare,
                    $true
                )
                if ((Get-FileSha256 -Path $resolved) -cne $backupHash) {
                    throw 'Rollback hash verification failed.'
                }
                if (Test-Path -LiteralPath $rollbackSpare -PathType Leaf) {
                    Remove-Item -LiteralPath $rollbackSpare -Force
                }
            } catch {
                $rollbackError = $_.Exception
            }
        }
        if (Test-Path -LiteralPath $candidatePath -PathType Leaf) {
            Remove-Item -LiteralPath $candidatePath -Force
        }
        if ($null -ne $rollbackError) {
            throw [InvalidOperationException]::new(
                "Codex config update failed and rollback failed: $($primary.Message); rollback: $($rollbackError.Message)",
                $primary
            )
        }
        throw [InvalidOperationException]::new(
            "Codex config update failed and was rolled back or never applied: $($primary.Message)",
            $primary
        )
    } finally {
        foreach ($temporaryPath in @($candidatePath, $rollbackSpare)) {
            if (Test-Path -LiteralPath $temporaryPath -PathType Leaf) {
                Remove-Item -LiteralPath $temporaryPath -Force
            }
        }
    }
}

function Get-CodexConfigPath {
    $profile = [Environment]::GetFolderPath(
        [Environment+SpecialFolder]::UserProfile
    )
    if ([string]::IsNullOrWhiteSpace($profile)) {
        throw 'Cannot resolve the interactive user profile.'
    }
    return [IO.Path]::GetFullPath(
        (Join-Path $profile '.codex\config.toml')
    )
}

function Invoke-SelfTest {
    $repoRoot = Split-Path -Parent $PSScriptRoot
    $selfTestBase = Join-Path $repoRoot '.RaymanCodingSkill\tmp'
    [void][IO.Directory]::CreateDirectory($selfTestBase)
    $fixtureRoot = Join-Path $selfTestBase (
        'configure-codex-validation-temp-' + [Guid]::NewGuid().ToString('N')
    )
    [void][IO.Directory]::CreateDirectory($fixtureRoot)

    try {
        Invoke-DirectoryWriteProbe -Path $fixtureRoot
        $configPath = Join-Path $fixtureRoot 'config.toml'
        $originalText = @(
            'model = "gpt-test"'
            'default_permissions = "windows-elevated-workspace"'
            ''
            '[windows]'
            'sandbox = "elevated"'
            ''
            '# comment that must survive byte-for-byte'
            '[shell_environment_policy.set]'
            'KEEP = "unchanged" # preserve this comment'
            'TEMP = "F:\\win\\Tmp" # preserve trailing comment'
            "TMP = 'F:\win\Tmp'"
            ''
            '[permissions.windows-elevated-workspace]'
            'description = "narrow fixture"'
            'extends = ":workspace"'
            ''
            '[permissions.windows-elevated-workspace.workspace_roots]'
            '"E:\\codex-sandbox\\temp" = true'
            '"E:\\codex-sandbox\\rayman-validation" = true'
            '"C:\\fixture" = false'
            ''
            '[permissions.windows-elevated-workspace.network]'
            'enabled = true'
        ) -join "`r`n"
        $originalBytes = [Text.UTF8Encoding]::new($false).GetBytes($originalText)
        [IO.File]::WriteAllBytes($configPath, $originalBytes)
        if ($IsWindows) {
            Assert-ConfigOwnerIsCurrentPrincipal -Path $configPath
        }
        $document = Read-StrictUtf8Document -Path $configPath
        $updatedText = Get-UpdatedConfigText `
            -Text $document.Text -NewLine $document.NewLine
        if (-not $updatedText.Contains(
                '# comment that must survive byte-for-byte'
            ) -or
            -not $updatedText.Contains(
                'KEEP = "unchanged" # preserve this comment'
            ) -or
            -not $updatedText.Contains(
                'TEMP = "E:\\codex-sandbox\\temp" # preserve trailing comment'
            ) -or
            -not $updatedText.Contains(
                'RAYMAN_VALIDATION_TEMP_ROOT = "E:\\codex-sandbox\\rayman-validation"'
            ) -or
            $updatedText.Contains("`n") -and
                $updatedText.Replace("`r`n", '').Contains("`n")) {
            throw 'CRLF/comment/structure preservation self-test failed.'
        }

        $withoutEnvironmentTable = @(
            'model = "gpt-test"'
            'default_permissions = "windows-elevated-workspace"'
            ''
            '[windows]'
            'sandbox = "elevated"'
            ''
            '[permissions.windows-elevated-workspace]'
            'extends = ":workspace"'
            ''
            '[permissions.windows-elevated-workspace.workspace_roots]'
            '"E:\\codex-sandbox\\temp" = true'
            '"E:\\codex-sandbox\\rayman-validation" = true'
        ) -join "`r`n"
        $insertedEnvironmentTable = Get-UpdatedConfigText `
            -Text $withoutEnvironmentTable -NewLine "`r`n"
        if (-not $insertedEnvironmentTable.Contains(
                "`r`n`r`n[shell_environment_policy.set]`r`n"
            ) -or
            -not $insertedEnvironmentTable.EndsWith(
                'RAYMAN_VALIDATION_TEMP_ROOT = "E:\\codex-sandbox\\rayman-validation"'
            )) {
            throw 'Missing environment-table insertion self-test failed.'
        }

        $candidateBytes = ConvertTo-StrictUtf8Bytes `
            -Text $updatedText -HasBom $document.HasBom
        $result = Set-ConfigFileTransactional -Path $configPath `
            -OriginalBytes $document.Bytes -CandidateBytes $candidateBytes
        if (-not $result.Changed -or
            -not (Test-Path -LiteralPath $result.BackupPath -PathType Leaf) -or
            (Get-BytesSha256 -Bytes ([IO.File]::ReadAllBytes(
                $result.BackupPath
            ))) -cne (Get-BytesSha256 -Bytes $originalBytes)) {
            throw 'Same-directory backup self-test failed.'
        }

        $terminal = Read-StrictUtf8Document -Path $configPath
        $terminalText = Get-UpdatedConfigText `
            -Text $terminal.Text -NewLine $terminal.NewLine
        $terminalBytes = ConvertTo-StrictUtf8Bytes `
            -Text $terminalText -HasBom $terminal.HasBom
        $backupCount = @(
            Get-ChildItem -LiteralPath $fixtureRoot -Filter '*.bak' -File
        ).Count
        $second = Set-ConfigFileTransactional -Path $configPath `
            -OriginalBytes $terminal.Bytes -CandidateBytes $terminalBytes
        if ($second.Changed -or
            @(
                Get-ChildItem -LiteralPath $fixtureRoot -Filter '*.bak' -File
            ).Count -ne $backupCount) {
            throw 'Idempotence self-test failed.'
        }

        $bomPath = Join-Path $fixtureRoot 'bom-config.toml'
        $bomText = $originalText.Replace("`r`n", "`n")
        $bomPayload = [Text.UTF8Encoding]::new($false).GetBytes($bomText)
        $bomBytes = [byte[]]::new($bomPayload.Length + 3)
        $bomBytes[0] = 0xEF
        $bomBytes[1] = 0xBB
        $bomBytes[2] = 0xBF
        [Array]::Copy($bomPayload, 0, $bomBytes, 3, $bomPayload.Length)
        [IO.File]::WriteAllBytes($bomPath, $bomBytes)
        $bomDocument = Read-StrictUtf8Document -Path $bomPath
        $bomUpdated = Get-UpdatedConfigText `
            -Text $bomDocument.Text -NewLine $bomDocument.NewLine
        $bomCandidate = ConvertTo-StrictUtf8Bytes `
            -Text $bomUpdated -HasBom $bomDocument.HasBom
        [void](Set-ConfigFileTransactional -Path $bomPath `
            -OriginalBytes $bomDocument.Bytes -CandidateBytes $bomCandidate)
        $bomTerminal = [IO.File]::ReadAllBytes($bomPath)
        if ($bomTerminal.Length -lt 3 -or
            $bomTerminal[0] -ne 0xEF -or
            $bomTerminal[1] -ne 0xBB -or
            $bomTerminal[2] -ne 0xBF -or
            [Text.UTF8Encoding]::new($false, $true).GetString(
                $bomTerminal[3..($bomTerminal.Length - 1)]
            ).Contains("`r`n")) {
            throw 'UTF-8 BOM/LF preservation self-test failed.'
        }

        $concurrentPath = Join-Path $fixtureRoot 'concurrent.toml'
        [IO.File]::WriteAllBytes($concurrentPath, $originalBytes)
        $concurrentBytes = [Text.UTF8Encoding]::new($false).GetBytes(
            $originalText + "`r`n# concurrent owner edit"
        )
        $concurrentFailed = $false
        try {
            [void](Set-ConfigFileTransactional -Path $concurrentPath `
                -OriginalBytes $originalBytes -CandidateBytes $candidateBytes `
                -BeforeReplace {
                    [IO.File]::WriteAllBytes($concurrentPath, $concurrentBytes)
                })
        } catch {
            $concurrentFailed = $_.Exception.Message.Contains('concurrently')
        }
        if (-not $concurrentFailed -or
            (Get-BytesSha256 -Bytes ([IO.File]::ReadAllBytes(
                $concurrentPath
            ))) -cne (Get-BytesSha256 -Bytes $concurrentBytes)) {
            throw 'Concurrent hash refusal self-test failed.'
        }

        $rollbackPath = Join-Path $fixtureRoot 'rollback.toml'
        [IO.File]::WriteAllBytes($rollbackPath, $originalBytes)
        $rollbackFailed = $false
        try {
            [void](Set-ConfigFileTransactional -Path $rollbackPath `
                -OriginalBytes $originalBytes -CandidateBytes $candidateBytes `
                -AfterReplace { throw 'simulated post-replace failure' })
        } catch {
            $rollbackFailed = $_.Exception.Message.Contains(
                'simulated post-replace failure'
            )
        }
        if (-not $rollbackFailed -or
            (Get-BytesSha256 -Bytes ([IO.File]::ReadAllBytes(
                $rollbackPath
            ))) -cne (Get-BytesSha256 -Bytes $originalBytes)) {
            throw 'Failure rollback self-test failed.'
        }

        foreach ($managedRoot in @(
            $script:ProcessTempRoot,
            $script:ValidationTempRoot
        )) {
            $rootAssignment = (New-TomlBasicString -Value $managedRoot) + ' = true'
            $missingCoverage = $originalText.Replace(
                "`r`n$rootAssignment", ''
            )
            $missingCoverageRejected = $false
            try {
                [void](Get-UpdatedConfigText `
                    -Text $missingCoverage -NewLine "`r`n")
            } catch {
                $missingCoverageRejected = $_.Exception.Message.Contains(
                    $managedRoot
                )
            }
            if (-not $missingCoverageRejected) {
                throw "Missing managed-root coverage self-test failed: $managedRoot"
            }
        }

        foreach ($unsafe in @(
            $originalText.Replace(
                'default_permissions = "windows-elevated-workspace"',
                'default_permissions = "danger-full-access"'
            ),
            $originalText.Replace(
                '"E:\\codex-sandbox\\temp" = true',
                (New-TomlBasicString -Value (
                    [IO.Path]::GetPathRoot($script:DataRoot)
                )) + ' = true'
            ),
            $originalText.Replace(
                'TEMP = "F:\\win\\Tmp" # preserve trailing comment',
                'TEMP = "F:\\one"' + "`r`n" + 'TEMP = "F:\\two"'
            ),
            $originalText.Replace(
                '"C:\\fixture" = false',
                '"." = true'
            ),
            $originalText.Replace(
                '"C:\\fixture" = false',
                '"E:drive-relative" = true'
            )
        )) {
            $rejected = $false
            try {
                [void](Get-UpdatedConfigText -Text $unsafe -NewLine "`r`n")
            } catch {
                $rejected = $true
            }
            if (-not $rejected) {
                throw 'Unsafe or ambiguous TOML fixture was not rejected.'
            }
        }

        if ($IsWindows) {
            $everyoneRejected = $false
            try {
                Assert-NoEveryoneWriteAcl `
                    -Sddl 'O:BAG:BAD:(A;;FA;;;WD)(A;;FA;;;BA)' `
                    -Label 'self-test'
            } catch {
                $everyoneRejected = $_.Exception.Message.Contains('Everyone')
            }
            if (-not $everyoneRejected) {
                throw 'Everyone ACL refusal self-test failed.'
            }
            $usersRejected = $false
            try {
                Assert-NoEveryoneWriteAcl `
                    -Sddl 'O:BAG:BAD:(A;;FA;;;BU)(A;;FA;;;BA)' `
                    -Label 'self-test'
            } catch {
                $usersRejected = $_.Exception.Message.Contains('BUILTIN\Users')
            }
            if (-not $usersRejected) {
                throw 'BUILTIN\Users ACL refusal self-test failed.'
            }
            foreach ($deleteChildSid in @('WD', 'BU')) {
                $deleteChildRejected = $false
                try {
                    Assert-NoEveryoneWriteAcl `
                        -Sddl "O:BAG:BAD:(A;;0x00000040;;;$deleteChildSid)(A;;FA;;;BA)" `
                        -Label 'self-test'
                } catch {
                    $deleteChildRejected = $_.Exception.Message.Contains(
                        'refusing ACL expansion'
                    )
                }
                if (-not $deleteChildRejected) {
                    throw "Delete-child ACL refusal self-test failed: $deleteChildSid"
                }
            }
        }

        Write-Output 'configure-codex-validation-temp self-test: PASS'
    } finally {
        $resolvedBase = [IO.Path]::GetFullPath($selfTestBase).TrimEnd('\', '/')
        $resolvedFixture = [IO.Path]::GetFullPath($fixtureRoot)
        if (-not $resolvedFixture.StartsWith(
                $resolvedBase + [IO.Path]::DirectorySeparatorChar,
                [StringComparison]::OrdinalIgnoreCase
            )) {
            throw "Self-test cleanup escaped its managed base: $resolvedFixture"
        }
        if (Test-Path -LiteralPath $resolvedFixture -PathType Container) {
            Remove-Item -LiteralPath $resolvedFixture -Recurse -Force
        }
    }
}

switch ($PSCmdlet.ParameterSetName) {
    'SelfTest' {
        if (-not $SelfTest.IsPresent) {
            throw 'The SelfTest parameter set requires -SelfTest to be present and true.'
        }
        Invoke-SelfTest
        return
    }
    'Check' {
        if (-not $Check.IsPresent) {
            throw 'The Check parameter set requires -Check to be present and true.'
        }
    }
    'Apply' {
        if (-not $Yes.IsPresent) {
            throw 'The Apply parameter set requires -Yes to be present and true.'
        }
    }
    default {
        throw "Unsupported parameter set: $($PSCmdlet.ParameterSetName)"
    }
}

if (-not $IsWindows) {
    throw 'Codex validation temp configuration is supported only on Windows.'
}

$configPath = Get-CodexConfigPath
$document = Read-StrictUtf8Document -Path $configPath
$updatedText = Get-UpdatedConfigText `
    -Text $document.Text -NewLine $document.NewLine
$candidateBytes = ConvertTo-StrictUtf8Bytes `
    -Text $updatedText -HasBom $document.HasBom

if ($PSCmdlet.ParameterSetName -eq 'Check') {
    if ((Get-BytesSha256 -Bytes $candidateBytes) -cne $document.Sha256) {
        throw "Codex validation temp policy is not configured. Run: pwsh -NoProfile -File `"$PSCommandPath`" -Yes"
    }
    Assert-DirectorySecurity -Path $script:DataRoot
    Assert-DirectorySecurity -Path $script:ProcessTempRoot
    Assert-DirectorySecurity -Path $script:ValidationTempRoot
    Write-Output "configure-codex-validation-temp: PASS (TEMP=$($script:ProcessTempRoot); RAYMAN_VALIDATION_TEMP_ROOT=$($script:ValidationTempRoot))"
    return
}

Assert-ConfigOwnerIsCurrentPrincipal -Path $configPath
Assert-DirectorySecurity -Path $script:DataRoot
foreach ($managedRoot in @(
    $script:ProcessTempRoot,
    $script:ValidationTempRoot
)) {
    if (-not (Test-Path -LiteralPath $managedRoot)) {
        [void][IO.Directory]::CreateDirectory($managedRoot)
    }
    Assert-DirectorySecurity -Path $managedRoot
}
Invoke-DirectoryWriteProbe -Path $script:ProcessTempRoot
Invoke-DirectoryWriteProbe -Path $script:ValidationTempRoot

$result = Set-ConfigFileTransactional -Path $configPath `
    -OriginalBytes $document.Bytes -CandidateBytes $candidateBytes
if ($result.Changed) {
    Write-Output 'configure-codex-validation-temp: UPDATED'
    Write-Output "backup=$($result.BackupPath)"
} else {
    Write-Output 'configure-codex-validation-temp: ALREADY_CONFIGURED'
}
Write-Output "process_temp_root=$($script:ProcessTempRoot)"
Write-Output "rayman_validation_temp_root=$($script:ValidationTempRoot)"
Write-Output 'Restart Codex completely before relying on the new environment.'
