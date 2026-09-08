[CmdletBinding()]
param(
    [string]$Root = (Split-Path -Parent $PSScriptRoot),
    [ValidateSet('Check', 'Plan')][string]$Mode = 'Check',
    [string[]]$Paths = @(),
    [string]$CandidateDirectory,
    [switch]$Json,
    [switch]$SelfTest
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if ($PSVersionTable.PSVersion.Major -lt 7) { throw 'This command requires PowerShell 7+' }
if ($PSBoundParameters.ContainsKey('SelfTest') -and -not $SelfTest) { throw 'SelfTest must be present and true' }
$script:Utf8 = [Text.UTF8Encoding]::new($false, $true)

function Get-ByteHash {
    param([AllowEmptyCollection()][byte[]]$Bytes)
    [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData($Bytes)).ToLowerInvariant()
}

function Test-OrdinalMember {
    param([object[]]$Items, [string]$Value)
    foreach ($item in $Items) {
        if ([StringComparer]::Ordinal.Equals([string]$item, $Value)) { return $true }
    }
    return $false
}

function Resolve-OrdinaryPath {
    param([string]$Base, [string]$Relative)
    if ([IO.Path]::IsPathRooted($Relative) -or $Relative.Contains('\') -or
        @($Relative.Split('/') | Where-Object { $_ -in @('', '.', '..') }).Count) {
        throw "SOURCE_BYTES_PATH: unsafe relative path: $Relative"
    }
    $cursor = [IO.Path]::GetFullPath($Base)
    $ancestor = $cursor
    while ($ancestor) {
        $item = Get-Item -LiteralPath $ancestor -Force -ErrorAction Stop
        if ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw "SOURCE_BYTES_REPARSE: $ancestor" }
        $ancestor = [IO.Path]::GetDirectoryName($ancestor)
    }
    foreach ($part in @('') + $Relative.Split('/')) {
        if ($part) { $cursor = Join-Path $cursor $part }
        $item = Get-Item -LiteralPath $cursor -Force -ErrorAction Stop
        if ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) {
            throw "SOURCE_BYTES_REPARSE: $Relative"
        }
    }
    if ((Get-Item -LiteralPath $cursor -Force).PSIsContainer) {
        throw "SOURCE_BYTES_NOT_FILE: $Relative"
    }
    return $cursor
}

function Invoke-SourceGit {
    param([string]$Base, [string[]]$Arguments, [byte[]]$InputBytes)
    $git = @(Get-Command git -All -ErrorAction Stop)[0]
    if ($git.CommandType -ne 'Application') { throw 'SOURCE_BYTES_GIT: git must resolve directly to an application' }
    $start = [Diagnostics.ProcessStartInfo]::new($git.Source)
    $start.WorkingDirectory = $Base
    $start.UseShellExecute = $false
    $start.CreateNoWindow = $true
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    $start.RedirectStandardInput = $true
    foreach ($arg in @('-c', 'core.fsmonitor=false') + $Arguments) { $start.ArgumentList.Add($arg) }
    $process = [Diagnostics.Process]::Start($start)
    $stdout = [IO.MemoryStream]::new()
    $read = $process.StandardOutput.BaseStream.CopyToAsync($stdout)
    $errorRead = $process.StandardError.ReadToEndAsync()
    if ($null -ne $InputBytes) { $process.StandardInput.BaseStream.Write($InputBytes) }
    $process.StandardInput.Close()
    $null = $read.GetAwaiter().GetResult()
    $process.WaitForExit()
    $errorText = $errorRead.GetAwaiter().GetResult()
    $exitCode = $process.ExitCode
    $bytes = $stdout.ToArray()
    $stdout.Dispose()
    $process.Dispose()
    if ($exitCode -ne 0) { throw "SOURCE_BYTES_GIT: git $Arguments exited ${exitCode}: $errorText" }
    return ,$bytes
}

function Get-TextIssue {
    param([AllowEmptyCollection()][byte[]]$Bytes, [bool]$Crlf = $false)
    try { $text = $script:Utf8.GetString($Bytes) }
    catch { return 'invalid_utf8' }
    if ($text.StartsWith([string][char]0xfeff, [StringComparison]::Ordinal)) { return 'utf8_bom' }
    if ($text.Contains([char]0)) { return 'nul_in_text' }
    if ($Crlf) {
        if ($text.Replace("`r`n", '').Contains("`n") -or $text.Replace("`r`n", '').Contains("`r")) {
            return 'expected_crlf'
        }
    } elseif ($text.Contains("`r")) {
        if ($text.Replace("`r`n", '').Contains("`r")) { return 'bare_cr' }
        if ($text.Replace("`r`n", '').Contains("`n")) { return 'mixed_eol' }
        return 'crlf'
    }
    return $null
}

function Get-CanonicalSourceBytes {
    param([AllowEmptyCollection()][byte[]]$Bytes)
    $text = $script:Utf8.GetString($Bytes)
    if ($text.Contains([char]0) -or $text.Replace("`r`n", '').Contains("`r")) {
        throw 'SOURCE_BYTES_UNSAFE_NORMALIZATION: NUL or bare CR requires an explicit encoding/content decision'
    }
    if ($text.StartsWith([string][char]0xfeff, [StringComparison]::Ordinal)) { $text = $text.Substring(1) }
    if ($text.StartsWith([string][char]0xfeff, [StringComparison]::Ordinal)) {
        throw 'SOURCE_BYTES_UNSAFE_NORMALIZATION: repeated BOM requires an explicit content decision'
    }
    return ,$script:Utf8.GetBytes($text.Replace("`r`n", "`n"))
}

function Read-SourcePolicy {
    param([string]$Base)
    $path = Resolve-OrdinaryPath $Base 'governance/source-bytes-policy.json'
    $bytes = [IO.File]::ReadAllBytes($path)
    if (Get-TextIssue $bytes) { throw 'SOURCE_BYTES_POLICY: policy bytes must be UTF-8, no BOM, LF' }
    $document = [Text.Json.JsonDocument]::Parse([IO.MemoryStream]::new($bytes))
    try {
        $names = @($document.RootElement.EnumerateObject() | ForEach-Object Name)
        $expected = @('schema', 'text_extensions', 'text_names', 'binary_extensions', 'binary_paths', 'crlf_paths')
        if ($names.Count -ne $expected.Count -or @($names | Select-Object -Unique).Count -ne $names.Count -or
            @($names | Where-Object { -not (Test-OrdinalMember $expected $_) }).Count) { throw 'SOURCE_BYTES_POLICY: unknown or duplicate fields' }
    } finally { $document.Dispose() }
    $policy = $script:Utf8.GetString($bytes) | ConvertFrom-Json -Depth 8
    if (-not [StringComparer]::Ordinal.Equals($policy.schema, 'rayman.source-bytes.policy.v1')) { throw 'SOURCE_BYTES_POLICY: unknown schema' }
    foreach ($key in @('text_extensions', 'text_names', 'binary_extensions', 'binary_paths', 'crlf_paths')) {
        if ($policy.$key -isnot [array] -or @($policy.$key | Where-Object { $_ -isnot [string] -or -not $_ }).Count -or
            @($policy.$key | Select-Object -Unique).Count -ne $policy.$key.Count) {
            throw "SOURCE_BYTES_POLICY: invalid $key"
        }
    }
    if (@($policy.binary_extensions | Where-Object { $_ -in $policy.text_extensions }).Count) {
        throw 'SOURCE_BYTES_POLICY: binary/text overlap'
    }
    if (@($policy.binary_paths | Where-Object { $_ -cin $policy.crlf_paths }).Count) { throw 'SOURCE_BYTES_POLICY: binary/CRLF exception overlap' }
    foreach ($path in @($policy.binary_paths) + @($policy.crlf_paths)) {
        if ([IO.Path]::IsPathRooted($path) -or $path -match '[\\*?\[\]{}]' -or
            @($path.Split('/') | Where-Object { $_ -in @('', '.', '..') }).Count) {
            throw 'SOURCE_BYTES_POLICY: exceptions require exact ordinary relative paths without glob metacharacters'
        }
    }
    return $policy
}

function Assert-SourceEditorPolicy {
    param([string]$Text, $Policy)
    $section = ''
    $rootSeen = $false
    $defaultSeen = $false
    $defaultCharset = $false
    $defaultEol = $false
    $exceptions = [Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
    foreach ($line in $Text.Split("`n")) {
        $line = $line.Trim()
        if (-not $line -or $line.StartsWith('#') -or $line.StartsWith(';')) { continue }
        if ($line -match '^\[([^\]]+)\]$') {
            $section = $Matches[1]
            if ($section -ceq '*') {
                if ($defaultSeen -or $exceptions.Count) { throw 'SOURCE_BYTES_EDITOR: one default section must precede exact exceptions' }
                $defaultSeen = $true
            }
            continue
        }
        if ($line -notmatch '^([^=]+)\s*=\s*(.*?)\s*$') { throw 'SOURCE_BYTES_EDITOR: malformed setting' }
        $key = $Matches[1].Trim().ToLowerInvariant()
        $value = $Matches[2].ToLowerInvariant()
        if ($key -eq 'root') {
            if ($section -or $rootSeen -or $value -ne 'true') { throw 'SOURCE_BYTES_EDITOR: root=true is required once before sections' }
            $rootSeen = $true
        } elseif ($key -eq 'charset') {
            if ($value -ne 'utf-8') { throw 'SOURCE_BYTES_EDITOR: governed text must use UTF-8' }
            if ($section -ceq '*') { $defaultCharset = $true }
        } elseif ($key -eq 'end_of_line') {
            if ($section -ceq '*') {
                if ($defaultEol -or $value -ne 'lf') { throw 'SOURCE_BYTES_EDITOR: default EOL must be LF' }
                $defaultEol = $true
            } elseif ((Test-OrdinalMember $Policy.crlf_paths $section) -and $value -eq 'crlf' -and $defaultEol) {
                if (-not $exceptions.Add($section)) { throw 'SOURCE_BYTES_EDITOR: duplicate CRLF override' }
            } else {
                throw 'SOURCE_BYTES_EDITOR: EOL overrides require an exact declared CRLF path'
            }
        }
    }
    if (-not $rootSeen -or -not $defaultCharset -or -not $defaultEol -or $exceptions.Count -ne $Policy.crlf_paths.Count) {
        throw 'SOURCE_BYTES_EDITOR: UTF-8/LF defaults and every exact CRLF override must match policy'
    }
}

function Get-GitAttributes {
    param([string]$Base, [string[]]$Names, [switch]$Cached)
    $args = @('check-attr', '-z')
    if ($Cached) { $args += '--cached' }
    $args += @('--stdin', 'text', 'eol')
    $inputBytes = $script:Utf8.GetBytes(($Names -join [char]0) + [char]0)
    $parts = $script:Utf8.GetString((Invoke-SourceGit $Base $args $inputBytes)).Split([char]0)
    $result = @{}
    for ($i = 0; $i -lt $parts.Length - 1; $i += 3) {
        if (-not $result.ContainsKey($parts[$i])) { $result[$parts[$i]] = @{} }
        $result[$parts[$i]][$parts[$i + 1]] = $parts[$i + 2]
    }
    return $result
}

function Get-IndexBlobs {
    param([string]$Base, $Entries)
    $oids = @($Entries.Values | Sort-Object -Unique)
    $result = @{}
    if (-not $oids.Count) { return $result }
    $batch = Invoke-SourceGit $Base @('cat-file', '--batch') ($script:Utf8.GetBytes(($oids -join "`n") + "`n"))
    $position = 0
    foreach ($oid in $oids) {
        $end = [Array]::IndexOf($batch, [byte]10, $position)
        if ($end -lt 0) { throw 'SOURCE_BYTES_INDEX: incomplete batch header' }
        $header = [Text.Encoding]::ASCII.GetString($batch, $position, $end - $position)
        if ($header -notmatch '^([0-9a-f]+) blob ([0-9]+)$' -or $Matches[1] -cne $oid) { throw 'SOURCE_BYTES_INDEX: unexpected batch object' }
        $length = [int]::Parse($Matches[2], [Globalization.CultureInfo]::InvariantCulture)
        $position = $end + 1
        if ($length -gt $batch.Length - $position - 1 -or $batch[$position + $length] -ne 10) { throw 'SOURCE_BYTES_INDEX: incomplete batch bytes' }
        $bytes = [byte[]]::new($length)
        [Array]::Copy($batch, $position, $bytes, 0, $length)
        $result[$oid] = $bytes
        $position += $length + 1
    }
    if ($position -ne $batch.Length) { throw 'SOURCE_BYTES_INDEX: trailing batch data' }
    return $result
}

function Get-SourceReport {
    param([string]$Base, [string[]]$Selected = @())
    $Base = [IO.Path]::GetFullPath($Base)
    $actualRoot = $script:Utf8.GetString((Invoke-SourceGit $Base @('rev-parse', '--show-toplevel'))).Trim()
    if (-not [IO.Path]::GetFullPath($actualRoot).Equals($Base, $(if ($IsWindows) { [StringComparison]::OrdinalIgnoreCase } else { [StringComparison]::Ordinal }))) {
        throw 'SOURCE_BYTES_ROOT: root must be the exact Git worktree'
    }
    $policy = Read-SourcePolicy $Base
    $editor = $script:Utf8.GetString([IO.File]::ReadAllBytes((Resolve-OrdinaryPath $Base '.editorconfig')))
    Assert-SourceEditorPolicy $editor $policy
    $entries = @{}
    $records = $script:Utf8.GetString((Invoke-SourceGit $Base @('ls-files', '--stage', '-z'))).Split([char]0)
    foreach ($record in $records) {
        if (-not $record) { continue }
        if ($record -notmatch '^(\d+) ([0-9a-f]+) (\d)\t([\s\S]+)$') { throw 'SOURCE_BYTES_INDEX: malformed index entry' }
        if ($Matches[3] -ne '0' -or $Matches[1] -notin @('100644', '100755')) {
            throw "SOURCE_BYTES_INDEX: conflict, symlink or gitlink: $($Matches[4])"
        }
        if ($entries.ContainsKey($Matches[4])) { throw "SOURCE_BYTES_INDEX: duplicate or case-colliding path: $($Matches[4])" }
        $entries[$Matches[4]] = $Matches[2]
    }
    $untracked = $script:Utf8.GetString((Invoke-SourceGit $Base @('ls-files', '--others', '--exclude-standard', '-z'))).Split([char]0)
    $allNames = @($entries.Keys) + @($untracked | Where-Object { $_ })
    $uniqueNames = [Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
    foreach ($name in $allNames) {
        if (-not $uniqueNames.Add($name)) { throw "SOURCE_BYTES_INDEX: duplicate or case-colliding path: $name" }
    }
    $names = @($allNames | Sort-Object)
    foreach ($exception in @($policy.binary_paths) + @($policy.crlf_paths)) {
        if (-not (Test-OrdinalMember $names $exception)) { throw "SOURCE_BYTES_POLICY: exception does not name a tracked/unignored file: $exception" }
    }
    foreach ($path in $Selected) { if (-not (Test-OrdinalMember $names $path)) { throw "SOURCE_BYTES_PATH: path is not a tracked or unignored source: $path" } }
    if ($Selected.Count) { $names = @($names | Where-Object { Test-OrdinalMember $Selected $_ }) }
    $blobs = Get-IndexBlobs $Base $entries
    $workAttrs = Get-GitAttributes $Base $names
    $indexAttrs = Get-GitAttributes $Base $names -Cached
    $findings = [Collections.Generic.List[object]]::new()
    $files = [Collections.Generic.List[object]]::new()
    foreach ($name in $names) {
        $extension = [IO.Path]::GetExtension($name).ToLowerInvariant()
        $binary = (Test-OrdinalMember $policy.binary_extensions $extension) -or (Test-OrdinalMember $policy.binary_paths $name)
        $known = $binary -or (Test-OrdinalMember $policy.text_extensions $extension) -or (Test-OrdinalMember $policy.text_names ([IO.Path]::GetFileName($name)))
        if (-not $known) { throw "SOURCE_BYTES_UNCLASSIFIED: declare source type explicitly: $name" }
        $crlf = Test-OrdinalMember $policy.crlf_paths $name
        $absolute = Join-Path $Base $name
        $present = Test-Path -LiteralPath $absolute
        $workBytes = if ($present) { ,[IO.File]::ReadAllBytes((Resolve-OrdinaryPath $Base $name)) } else { $null }
        $indexBytes = if ($entries.ContainsKey($name)) { ,$blobs[$entries[$name]] } else { $null }
        foreach ($view in @('worktree', 'index')) {
            $bytes = if ($view -eq 'worktree') { $workBytes } else { $indexBytes }
            if ($null -eq $bytes) { continue }
            $attrs = if ($view -eq 'worktree') { $workAttrs[$name] } else { $indexAttrs[$name] }
            $expectedEol = if ($crlf -and $view -eq 'worktree') { 'crlf' } else { 'lf' }
            $issue = if ($binary) { $null } else { Get-TextIssue $bytes ($crlf -and $view -eq 'worktree') }
            if (($binary -and $attrs.text -ne 'unset') -or
                (-not $binary -and ($attrs.text -notin @('set', 'auto') -or $attrs.eol -ne $(if ($crlf) { 'crlf' } else { 'lf' })))) {
                $findings.Add([ordered]@{ path = $name; view = $view; issue = 'git_attribute_conflict'; expected_eol = $expectedEol })
            }
            if ($issue) {
                $offset = if ($issue -eq 'nul_in_text') { [Array]::IndexOf($bytes, [byte]0) } elseif ($issue -in @('crlf', 'mixed_eol', 'bare_cr')) { [Array]::IndexOf($bytes, [byte]13) } else { 0 }
                $findings.Add([ordered]@{ path = $name; view = $view; issue = $issue; byte_offset = $offset; sha256 = Get-ByteHash $bytes })
            }
        }
        $files.Add([ordered]@{
            path = $name; binary = $binary; crlf_exception = $crlf
            worktree_sha256 = if ($null -ne $workBytes) { Get-ByteHash $workBytes } else { $null }
            index_sha256 = if ($null -ne $indexBytes) { Get-ByteHash $indexBytes } else { $null }
            index_blob_oid = if ($entries.ContainsKey($name)) { $entries[$name] } else { $null }
        })
    }
    [ordered]@{ schema = 'rayman.source-bytes.check.v1'; status = $(if ($findings.Count) { 'fail' } else { 'pass' }); file_count = $files.Count; findings = @($findings.ToArray()); files = @($files.ToArray()) }
}

function New-RepairCandidates {
    param([string]$Base, $Report, [string]$Destination)
    $changes = [Collections.Generic.List[object]]::new()
    foreach ($entry in $Report.files) {
        if ($entry.binary -or $entry.crlf_exception -or $null -eq $entry.worktree_sha256) { continue }
        $bytes = [IO.File]::ReadAllBytes((Resolve-OrdinaryPath $Base $entry.path))
        if ((Get-ByteHash $bytes) -ne $entry.worktree_sha256) { throw 'SOURCE_BYTES_DRIFT: source changed after inspection' }
        $canonical = Get-CanonicalSourceBytes $bytes
        $after = Get-ByteHash $canonical
        if ($after -eq $entry.worktree_sha256) { continue }
        $changes.Add([ordered]@{ path = $entry.path; before_sha256 = $entry.worktree_sha256; after_sha256 = $after })
    }
    if ($Destination) {
        # Candidate generation never overwrites source, the index or an existing
        # output directory. Publication remains the normal reviewed source edit.
        $destinationFull = [IO.Path]::GetFullPath($Destination)
        if (Test-Path -LiteralPath $destinationFull) { throw 'SOURCE_BYTES_DESTINATION: candidate directory must be new' }
        $parent = Get-Item -LiteralPath (Split-Path -Parent $destinationFull) -Force
        if (-not $parent.PSIsContainer -or ($parent.Attributes -band [IO.FileAttributes]::ReparsePoint)) { throw 'SOURCE_BYTES_DESTINATION: ordinary parent required' }
        [IO.Directory]::CreateDirectory($destinationFull) | Out-Null
        foreach ($change in $changes) {
            $source = Resolve-OrdinaryPath $Base $change.path
            $bytes = [IO.File]::ReadAllBytes($source)
            if ((Get-ByteHash $bytes) -ne $change.before_sha256) { throw 'SOURCE_BYTES_DRIFT: source changed while preparing candidates' }
            $target = Join-Path $destinationFull $change.path
            [IO.Directory]::CreateDirectory((Split-Path -Parent $target)) | Out-Null
            $canonical = Get-CanonicalSourceBytes $bytes
            $file = [IO.File]::Open($target, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write, [IO.FileShare]::None)
            try { $file.Write($canonical); $file.Flush($true) } finally { $file.Dispose() }
            if ((Get-ByteHash ([IO.File]::ReadAllBytes($target))) -ne $change.after_sha256) { throw 'SOURCE_BYTES_READBACK: candidate hash mismatch' }
        }
    }
    [ordered]@{ schema = 'rayman.source-bytes.plan.v1'; source_modified = $false; index_modified = $false; candidate_directory = $Destination; changes = @($changes.ToArray()); diagnostics = $Report.findings }
}

function Invoke-SelfTest {
    $cases = @(
        @('LF', "a`nb`n", $null), @('CRLF', "a`r`nb`r`n", 'crlf'),
        @('mixed', "a`nb`r`n", 'mixed_eol'), @('bare CR', "a`rb", 'bare_cr'),
        @('BOM', ([string][char]0xfeff + "a`n"), 'utf8_bom'),
        @('NUL', "a`0b", 'nul_in_text'), @('empty', '', $null),
        @('no trailing newline', 'a', $null), @('Unicode', "中文`n", $null)
    )
    foreach ($case in $cases) {
        $actual = Get-TextIssue ($script:Utf8.GetBytes($case[1]))
        if ($actual -cne $case[2]) { throw "source-bytes test failed: $($case[0]): $actual" }
    }
    if ((Get-TextIssue ([byte[]]@(0xff, 0xfe))) -ne 'invalid_utf8') { throw 'invalid UTF-8 accepted' }
    if (Get-TextIssue ($script:Utf8.GetBytes("a`r`n")) $true) { throw 'CRLF exception rejected' }
    $normal = Get-CanonicalSourceBytes ($script:Utf8.GetBytes("a`r`nb`n"))
    if ((Get-ByteHash $normal) -ne (Get-ByteHash (Get-CanonicalSourceBytes $normal))) { throw 'normalization not idempotent' }
    if ((Get-ByteHash $normal) -eq (Get-ByteHash ($script:Utf8.GetBytes("a`r`nb`n")))) { throw 'raw integrity hash was normalized' }
    $singleBom = Get-CanonicalSourceBytes ($script:Utf8.GetBytes([string][char]0xfeff + "a`r`n"))
    if ((Get-ByteHash $singleBom) -ne (Get-ByteHash (Get-CanonicalSourceBytes $singleBom))) { throw 'BOM normalization not idempotent' }
    $repeatedBomRejected = $false
    try { $null = Get-CanonicalSourceBytes ($script:Utf8.GetBytes([string][char]0xfeff + [string][char]0xfeff + 'a')) }
    catch { $repeatedBomRejected = $_.Exception.Message.Contains('repeated BOM') }
    if (-not $repeatedBomRejected) { throw 'ambiguous repeated BOM produced a noncanonical candidate' }
    $testParent = if ($IsWindows) { [IO.Path]::GetTempPath() } else { Join-Path $Root '.RaymanCodingSkill/tmp' }
    [IO.Directory]::CreateDirectory($testParent) | Out-Null
    $temp = Join-Path $testParent ('source-bytes-' + [Guid]::NewGuid().ToString('N'))
    [IO.Directory]::CreateDirectory($temp) | Out-Null
    try {
        Invoke-SourceGit $temp @('init', '--quiet') | Out-Null
        [IO.Directory]::CreateDirectory((Join-Path $temp 'governance')) | Out-Null
        [IO.File]::Copy((Join-Path $Root 'governance/source-bytes-policy.json'), (Join-Path $temp 'governance/source-bytes-policy.json'))
        [IO.File]::Copy((Join-Path $Root '.editorconfig'), (Join-Path $temp '.editorconfig'))
        $fixturePolicy = Read-SourcePolicy $temp
        $fixturePolicy.binary_paths = @('raw-fixture.txt')
        $fixturePolicy.crlf_paths = @('crlf-fixture.txt')
        [IO.File]::WriteAllText((Join-Path $temp 'governance/source-bytes-policy.json'), (($fixturePolicy | ConvertTo-Json -Depth 8).Replace("`r`n", "`n") + "`n"), $script:Utf8)
        $fixtureEditor = [IO.File]::ReadAllText((Join-Path $temp '.editorconfig')) + "`n[crlf-fixture.txt]`nend_of_line = crlf`n"
        [IO.File]::WriteAllText((Join-Path $temp '.editorconfig'), $fixtureEditor, $script:Utf8)
        [IO.File]::WriteAllText((Join-Path $temp '.gitattributes'), "* text=auto eol=lf`n*.png binary`nraw-fixture.txt binary`ncrlf-fixture.txt text eol=crlf`n", $script:Utf8)
        [IO.File]::WriteAllBytes((Join-Path $temp 'raw-fixture.txt'), [byte[]]@(0xff, 0xfe, 0, 13, 10))
        [IO.File]::WriteAllText((Join-Path $temp 'crlf-fixture.txt'), "fixture`r`n", $script:Utf8)
        [IO.File]::WriteAllBytes((Join-Path $temp 'empty.txt'), [byte[]]@())
        [IO.File]::WriteAllBytes((Join-Path $temp 'binary.png'), [byte[]]@(0, 255, 13, 10))
        $source = Join-Path $temp '中文 file.txt'
        [IO.File]::WriteAllText($source, "a`nb`n", $script:Utf8)
        Invoke-SourceGit $temp @('add', '--all') | Out-Null
        $initial = Get-SourceReport $temp
        if ($initial.status -ne 'pass') { throw 'exact binary/CRLF exceptions failed full worktree/index inspection' }
        $badEditorRejected = $false
        try { Assert-SourceEditorPolicy ($fixtureEditor + "`n[unknown.txt]`nend_of_line = crlf`n") $fixturePolicy }
        catch { $badEditorRejected = $_.Exception.Message.Contains('SOURCE_BYTES_EDITOR') }
        if (-not $badEditorRejected) { throw 'undeclared editor CRLF override accepted' }
        [IO.File]::WriteAllText($source, "a`r`nb`r`n", $script:Utf8)
        foreach ($autocrlf in @('true', 'false', 'input')) {
            foreach ($eol in @('lf', 'crlf', 'native')) {
                Invoke-SourceGit $temp @('config', 'core.autocrlf', $autocrlf) | Out-Null
                Invoke-SourceGit $temp @('config', 'core.eol', $eol) | Out-Null
                $diff = Invoke-SourceGit $temp @('diff', '--name-only')
                $report = Get-SourceReport $temp
                if ($diff.Length -ne 0 -or $report.status -ne 'fail' -or $report.findings[0].issue -ne 'crlf') { throw 'Git-clean CRLF escaped byte preflight' }
            }
        }
        $beforeIndex = Get-ByteHash ([IO.File]::ReadAllBytes((Join-Path $temp '.git/index')))
        $plan = New-RepairCandidates $temp $report (Join-Path $temp 'candidates')
        if ($plan.changes.Count -ne 1 -or $plan.source_modified -or (Get-ByteHash ([IO.File]::ReadAllBytes((Join-Path $temp '.git/index')))) -ne $beforeIndex) { throw 'candidate plan changed source/index' }
        # Raw index bytes must be checked even when the working file is valid LF.
        [IO.File]::WriteAllText($source, "a`r`n", $script:Utf8)
        $oid = $script:Utf8.GetString((Invoke-SourceGit $temp @('hash-object', '-w', '--no-filters', '--', '中文 file.txt'))).Trim()
        Invoke-SourceGit $temp @('update-index', '--cacheinfo', "100644,$oid,中文 file.txt") | Out-Null
        [IO.File]::WriteAllText($source, "a`n", $script:Utf8)
        $report = Get-SourceReport $temp @('中文 file.txt')
        if (@($report.findings | Where-Object { $_.view -eq 'index' -and $_.issue -eq 'crlf' }).Count -ne 1) { throw 'CRLF index accepted' }
        if (Test-OrdinalMember @('crlf-fixture.txt') ('crlf-fixture.txt' + [char]0xfeff)) { throw 'invisible path alias accepted' }
        Invoke-SourceGit $temp @('update-index', '--add', '--cacheinfo', "100644,$oid,Case.txt") | Out-Null
        Invoke-SourceGit $temp @('update-index', '--add', '--cacheinfo', "100644,$oid,case.txt") | Out-Null
        $caseCollisionRejected = $false
        try { $null = Get-SourceReport $temp }
        catch { $caseCollisionRejected = $_.Exception.Message.Contains('case-colliding') }
        if (-not $caseCollisionRejected) { throw 'case-colliding index entries were silently omitted' }
        Write-Output 'source-bytes self-test: PASS (encoding, exceptions, Git matrix, index, candidate preservation)'
    } finally {
        # Only this fresh fixture is owned here; never follow a reparse entry.
        $expectedPrefix = [IO.Path]::GetFullPath($testParent).TrimEnd([IO.Path]::DirectorySeparatorChar) + [IO.Path]::DirectorySeparatorChar
        if (-not [IO.Path]::GetFullPath($temp).StartsWith($expectedPrefix, [StringComparison]::Ordinal)) { throw 'unsafe source-byte fixture cleanup target' }
        $items = @(Get-ChildItem -LiteralPath $temp -Recurse -Force)
        if (@($items | Where-Object { $_.Attributes -band [IO.FileAttributes]::ReparsePoint }).Count) { throw "self-test retained unsafe fixture: $temp" }
        Remove-Item -LiteralPath $temp -Recurse -Force
    }
}

if ($SelfTest) { Invoke-SelfTest; return }
if ($CandidateDirectory -and $Mode -ne 'Plan') { throw 'CandidateDirectory is only valid with -Mode Plan' }
$report = Get-SourceReport $Root $Paths
if ($Mode -eq 'Plan') {
    New-RepairCandidates $Root $report $CandidateDirectory | ConvertTo-Json -Depth 8
} else {
    if ($Json) { $report | ConvertTo-Json -Depth 8 }
    else {
        foreach ($finding in $report.findings) { Write-Output "$($finding.path) [$($finding.view)]: $($finding.issue)" }
        Write-Output "source-bytes: $($report.status.ToUpperInvariant()) ($($report.file_count) files; raw worktree and index bytes)"
    }
    if ($report.status -ne 'pass') { throw 'SOURCE_BYTES_FAILED: repair declared source bytes before hashing or validation; use -Mode Plan for a zero-source-write preview' }
}
