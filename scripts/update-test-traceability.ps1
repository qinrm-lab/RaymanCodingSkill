[CmdletBinding()]
param(
    [string]$DraftInventoryPath,
    [string]$DraftManifestPath,
    [string]$OutputDirectory,
    [switch]$Publish,
    [switch]$Recover,
    [switch]$Yes,
    [switch]$Summary,
    [switch]$SelfTest
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if ($PSVersionTable.PSVersion.Major -lt 7) { throw 'This command requires PowerShell 7+' }
if ($PSBoundParameters.ContainsKey('SelfTest') -and -not $SelfTest) { throw 'SelfTest must be present and true' }
$script:Root = [IO.Path]::GetFullPath((Split-Path -Parent $PSScriptRoot))
$script:Utf8 = [Text.UTF8Encoding]::new($false, $true)
$script:Names = @('first-party-test-inventory.json', 'test-traceability.json')

function Get-PublicationHash {
    param([string]$Path)
    (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}

function Assert-PublicationPath {
    param([string]$Path)
    $cursor = [IO.Path]::GetFullPath($Path)
    while ($cursor) {
        if (Test-Path -LiteralPath $cursor) {
            $item = Get-Item -LiteralPath $cursor -Force
            if ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw "TRACE_PUBLICATION_REPARSE: $cursor" }
        }
        $parent = [IO.Path]::GetDirectoryName($cursor)
        if ($parent -eq $cursor) { break }
        $cursor = $parent
    }
}

function Write-NewPublicationFile {
    param([string]$Path, [byte[]]$Bytes)
    Assert-PublicationPath $Path
    $stream = [IO.File]::Open($Path, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write, [IO.FileShare]::None)
    try { $stream.Write($Bytes); $stream.Flush($true) } finally { $stream.Dispose() }
}

function Get-PublicationTemp {
    param([string]$Base)
    $path = [IO.Path]::GetFullPath((Join-Path $Base '.RaymanCodingSkill/tmp'))
    Assert-PublicationPath $path
    [IO.Directory]::CreateDirectory($path) | Out-Null
    return $path
}

function Replace-PublicationFile {
    param([string]$Stage, [string]$Target)
    Assert-PublicationPath $Stage
    Assert-PublicationPath $Target
    if (-not $IsWindows) { [IO.File]::SetUnixFileMode($Stage, [IO.File]::GetUnixFileMode($Target)) }
    $previous = $Stage + '.previous'
    if (Test-Path -LiteralPath $previous) { throw 'TRACE_PUBLICATION_STAGE: backup name already exists' }
    [IO.File]::Replace($Stage, $Target, $previous)
}

function Restore-PublicationJournal {
    param([string]$Base, [string]$JournalPath)
    Assert-PublicationPath $JournalPath
    $journal = [IO.File]::ReadAllText($JournalPath, $script:Utf8) | ConvertFrom-Json -Depth 8
    $temp = Get-PublicationTemp $Base
    $comparison = if ($IsWindows) { [StringComparison]::OrdinalIgnoreCase } else { [StringComparison]::Ordinal }
    if ($journal.schema -cne 'rayman.traceability.publication.v1' -or $journal.root -cne $Base -or
        -not [IO.Path]::GetFullPath([string]$journal.directory).StartsWith($temp + [IO.Path]::DirectorySeparatorChar, $comparison) -or
        $journal.files.Count -ne 2) { throw 'TRACE_PUBLICATION_JOURNAL: invalid root or file contract' }
    # Validate the complete old/new tuple before restoring either file. An
    # unrelated edit never becomes an authorized rollback target.
    for ($i = 0; $i -lt 2; $i++) {
        $entry = $journal.files[$i]
        if ($entry.name -cne $script:Names[$i] -or $entry.before -cnotmatch '^[a-f0-9]{64}$' -or $entry.after -cnotmatch '^[a-f0-9]{64}$') { throw 'TRACE_PUBLICATION_JOURNAL: invalid file binding' }
        $target = Join-Path $Base "governance/$($entry.name)"
        $backup = Join-Path $journal.directory "before-$($entry.name)"
        Assert-PublicationPath $target
        Assert-PublicationPath $backup
        if ((Get-PublicationHash $backup) -cne $entry.before -or (Get-PublicationHash $target) -cnotin @($entry.before, $entry.after)) {
            throw 'TRACE_PUBLICATION_RECOVERY_DRIFT: retain journal and files for review'
        }
    }
    foreach ($entry in $journal.files) {
        $target = Join-Path $Base "governance/$($entry.name)"
        if ((Get-PublicationHash $target) -ceq $entry.before) { continue }
        $stage = Join-Path $journal.directory ('restore-' + [Guid]::NewGuid().ToString('N'))
        Write-NewPublicationFile $stage ([IO.File]::ReadAllBytes((Join-Path $journal.directory "before-$($entry.name)")))
        Replace-PublicationFile $stage $target
        if ((Get-PublicationHash $target) -cne $entry.before) { throw 'TRACE_PUBLICATION_ROLLBACK: restored hash differs' }
    }
    [IO.File]::Delete($JournalPath)
}

function Publish-TraceabilityCandidate {
    param([string]$Base, $Candidate, [switch]$FailAfterFirst, [scriptblock]$VerifyPublication)
    if ($Candidate.schema -cne 'rayman.traceability.candidate.v1' -or $Candidate.status -cne 'validated') { throw 'TRACE_PUBLICATION_CANDIDATE: validated generation contract required' }
    $temp = Get-PublicationTemp $Base
    $directory = [IO.Path]::GetFullPath([string]$Candidate.output_directory)
    $comparison = if ($IsWindows) { [StringComparison]::OrdinalIgnoreCase } else { [StringComparison]::Ordinal }
    if (-not $directory.StartsWith($temp + [IO.Path]::DirectorySeparatorChar, $comparison)) { throw 'TRACE_PUBLICATION_DIRECTORY: candidates must be in workspace managed temp' }
    Assert-PublicationPath $directory
    $lockPath = Join-Path $temp 'traceability-publication.lock'
    $journalPath = Join-Path $temp 'traceability-publication.json'
    Assert-PublicationPath $lockPath
    $lock = [IO.File]::Open($lockPath, [IO.FileMode]::OpenOrCreate, [IO.FileAccess]::ReadWrite, [IO.FileShare]::None)
    try {
        if (Test-Path -LiteralPath $journalPath) { throw 'TRACE_PUBLICATION_RECOVERY_REQUIRED: use -Recover -Yes before a new publication' }
        $files = @()
        foreach ($name in $script:Names) {
            $relative = "governance/$name"
            $target = Join-Path $Base $relative
            $source = Join-Path $directory $name
            Assert-PublicationPath $target
            Assert-PublicationPath $source
            $before = [string]$Candidate.predecessor_worktree_sha256.$relative
            $after = if ($name -ceq $script:Names[0]) { [string]$Candidate.inventory_sha256 } else { [string]$Candidate.manifest_sha256 }
            if ((Get-PublicationHash $target) -cne $before -or (Get-PublicationHash $source) -cne $after) { throw 'TRACE_PUBLICATION_DRIFT: source or candidate bytes changed' }
            $files += [ordered]@{ name = $name; before = $before; after = $after }
        }
        foreach ($entry in $files) {
            Write-NewPublicationFile (Join-Path $directory "before-$($entry.name)") ([IO.File]::ReadAllBytes((Join-Path $Base "governance/$($entry.name)")))
            if ((Get-PublicationHash (Join-Path $directory "before-$($entry.name)")) -cne $entry.before) { throw 'TRACE_PUBLICATION_BACKUP: preimage changed' }
        }
        $journal = [ordered]@{ schema = 'rayman.traceability.publication.v1'; root = $Base; directory = $directory; files = $files }
        Write-NewPublicationFile $journalPath ($script:Utf8.GetBytes(($journal | ConvertTo-Json -Depth 8 -Compress) + "`n"))
        try {
            for ($i = 0; $i -lt $files.Count; $i++) {
                $entry = $files[$i]
                $target = Join-Path $Base "governance/$($entry.name)"
                if ((Get-PublicationHash $target) -cne $entry.before) { throw 'TRACE_PUBLICATION_DRIFT: preimage changed before replacement' }
                $stage = Join-Path $directory ('publish-' + [Guid]::NewGuid().ToString('N'))
                Write-NewPublicationFile $stage ([IO.File]::ReadAllBytes((Join-Path $directory $entry.name)))
                if ((Get-PublicationHash $stage) -cne $entry.after) { throw 'TRACE_PUBLICATION_DRIFT: candidate changed before replacement' }
                Replace-PublicationFile $stage $target
                if ($FailAfterFirst -and $i -eq 0) { throw 'intentional publication failure' }
            }
            if ($null -ne $VerifyPublication) { $null = & $VerifyPublication }
            foreach ($entry in $files) {
                if ((Get-PublicationHash (Join-Path $Base "governance/$($entry.name)")) -cne $entry.after) { throw 'TRACE_PUBLICATION_READBACK: published tuple changed' }
            }
            [IO.File]::Delete($journalPath)
        } catch {
            Restore-PublicationJournal $Base $journalPath
            throw
        }
    } finally { $lock.Dispose() }
}

function Invoke-GenerationSelfTest {
    $testParent = if ($IsWindows) { [IO.Path]::GetTempPath() } else { Get-PublicationTemp $script:Root }
    $base = Join-Path $testParent ('traceability-publish-' + [Guid]::NewGuid().ToString('N'))
    $null = New-Item -ItemType Directory -Path $base
    try {
        $null = New-Item -ItemType Directory -Path (Join-Path $base 'governance')
        $directory = Join-Path (Get-PublicationTemp $base) 'candidate'
        $null = New-Item -ItemType Directory -Path $directory
        $before = @{}
        foreach ($name in $script:Names) {
            Write-NewPublicationFile (Join-Path $base "governance/$name") ($script:Utf8.GetBytes("{}`n"))
            Write-NewPublicationFile (Join-Path $directory $name) ($script:Utf8.GetBytes("{`"new`":true}`n"))
            $before["governance/$name"] = Get-PublicationHash (Join-Path $base "governance/$name")
        }
        $candidate = [pscustomobject]@{ schema = 'rayman.traceability.candidate.v1'; status = 'validated'; output_directory = $directory; predecessor_worktree_sha256 = [pscustomobject]$before; inventory_sha256 = Get-PublicationHash (Join-Path $directory $script:Names[0]); manifest_sha256 = Get-PublicationHash (Join-Path $directory $script:Names[1]) }
        $failed = $false
        $failure = ''
        try { Publish-TraceabilityCandidate $base $candidate -FailAfterFirst } catch { $failure = $_.Exception.Message; $failed = $failure.Contains('intentional publication failure') }
        if (-not $failed) { throw "publication failure injection did not execute: $failure" }
        foreach ($name in $script:Names) {
            if ((Get-PublicationHash (Join-Path $base "governance/$name")) -cne $before["governance/$name"]) { throw 'two-file rollback did not restore original bytes' }
        }
        $directory2 = Join-Path (Get-PublicationTemp $base) 'candidate2'
        $null = New-Item -ItemType Directory -Path $directory2
        foreach ($name in $script:Names) { [IO.File]::Copy((Join-Path $directory $name), (Join-Path $directory2 $name)) }
        $candidate.output_directory = $directory2
        Publish-TraceabilityCandidate $base $candidate
        foreach ($name in $script:Names) {
            if ((Get-PublicationHash (Join-Path $base "governance/$name")) -cne (Get-PublicationHash (Join-Path $directory2 $name))) { throw 'publication readback failed' }
        }
        $rejected = $false
        try { Publish-TraceabilityCandidate $base $candidate } catch { $rejected = $_.Exception.Message.Contains('TRACE_PUBLICATION_DRIFT') }
        if (-not $rejected) { throw 'stale preimage accepted' }
        Write-Output 'update-test-traceability self-test: PASS (publish, rollback, stale preimage, exact tuple)'
    } finally {
        Assert-PublicationPath $base
        if (-not [IO.Path]::GetFullPath($base).StartsWith(([IO.Path]::GetFullPath($testParent).TrimEnd([IO.Path]::DirectorySeparatorChar) + [IO.Path]::DirectorySeparatorChar), [StringComparison]::OrdinalIgnoreCase)) { throw 'unsafe test cleanup target' }
        if (@(Get-ChildItem -LiteralPath $base -Recurse -Force | Where-Object { $_.Attributes -band [IO.FileAttributes]::ReparsePoint }).Count) { throw "retained unsafe fixture: $base" }
        Remove-Item -LiteralPath $base -Recurse -Force
    }
}

if ($SelfTest) { Invoke-GenerationSelfTest; return }
if (($Publish -or $Recover) -and -not $Yes) { throw 'Publication/recovery requires -Yes; generation alone never changes source files' }
if ($Recover) {
    if ($Publish -or $Summary -or $DraftInventoryPath -or $DraftManifestPath -or $OutputDirectory) { throw 'Recovery is a separate operation' }
    $temp = Get-PublicationTemp $script:Root
    $lockPath = Join-Path $temp 'traceability-publication.lock'
    Assert-PublicationPath $lockPath
    $lock = [IO.File]::Open($lockPath, [IO.FileMode]::OpenOrCreate, [IO.FileAccess]::ReadWrite, [IO.FileShare]::None)
    try { Restore-PublicationJournal $script:Root (Join-Path $temp 'traceability-publication.json') }
    finally { $lock.Dispose() }
    Write-Output 'traceability publication recovered'
    return
}
if ($Summary) {
    if ($Publish -or $DraftInventoryPath -or $DraftManifestPath -or $OutputDirectory) { throw 'Summary is read-only and separate from generation' }
    & (Join-Path $PSScriptRoot 'check-test-traceability.ps1') -Summary
    return
}
$json = & (Join-Path $PSScriptRoot 'check-test-traceability.ps1') -Generate `
    -DraftInventoryPath $DraftInventoryPath -DraftManifestPath $DraftManifestPath -OutputDirectory $OutputDirectory
$candidate = ($json -join "`n") | ConvertFrom-Json -Depth 8
if ($Publish) {
    Publish-TraceabilityCandidate $script:Root $candidate -VerifyPublication {
        & (Join-Path $PSScriptRoot 'check-test-traceability.ps1') | Out-Null
    }
    $candidate.source_modified = $true
}
$candidate | Add-Member -NotePropertyName published -NotePropertyValue ([bool]$Publish)
$candidate | ConvertTo-Json -Depth 8
