[CmdletBinding(DefaultParameterSetName = 'Hook')]
param(
    [Parameter(Mandatory = $true, ParameterSetName = 'Hook')]
    [ValidateSet('Observe', 'Enforce')]
    [string]$Mode,

    [Parameter(Mandatory = $true, ParameterSetName = 'Hook')]
    [ValidatePattern('^[0-9a-f]{64}$')]
    [string]$ExpectedScriptSha256,

    [Parameter(Mandatory = $true, ParameterSetName = 'DeterministicTest')]
    [switch]$DeterministicTest,

    [Parameter(ParameterSetName = 'DeterministicTest')]
    [string]$DeterministicTestConfigPath
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

if ($PSVersionTable.PSVersion.Major -lt 7) {
    throw 'codex-wait-route-guard.ps1 requires PowerShell 7+.'
}

$script:Utf8 = [Text.UTF8Encoding]::new($false, $true)
$script:MaximumTranscriptTailBytes = 4MB
$script:MaximumSessionMetadataBytes = 1MB
$script:MaximumYieldAge = [TimeSpan]::FromMinutes(10)
$script:MaximumContinuationAge = [TimeSpan]::FromMinutes(2)
$script:RouteContext = @'
functions.wait routing rule: Call functions.wait only when the immediately preceding functions.exec result in this same session and turn contains the literal `Script running with cell ID <X>`, and pass that exact active X. Never guess, reuse a completed or failed cell, recover a cell from prose, or retry after `exec cell ... not found`. For child-agent coordination, call collaboration.list_agents({}) directly and then collaboration.wait_agent({"timeout_ms":180000}) directly. Never place collaboration tools inside functions.exec.
'@.Trim()

function Get-TextSha256 {
    param([Parameter(Mandatory = $true)][string]$Text)

    $bytes = $script:Utf8.GetBytes($Text)
    $sha = [Security.Cryptography.SHA256]::Create()
    try {
        return [Convert]::ToHexString($sha.ComputeHash($bytes)).ToLowerInvariant()
    } finally {
        $sha.Dispose()
    }
}

function Test-ObjectProperty {
    param(
        [Parameter(Mandatory = $true)]$Object,
        [Parameter(Mandatory = $true)][string]$Name
    )

    return $null -ne $Object -and
        $null -ne $Object.PSObject -and
        $null -ne $Object.PSObject.Properties[$Name]
}

function Get-RequiredString {
    param(
        [Parameter(Mandatory = $true)]$Object,
        [Parameter(Mandatory = $true)][string]$Name,
        [ValidateRange(1, 32768)][int]$MaximumLength = 1024
    )

    if (-not (Test-ObjectProperty -Object $Object -Name $Name)) {
        throw "Missing string field: $Name"
    }
    $value = $Object.PSObject.Properties[$Name].Value
    if ($value -isnot [string] -or [string]::IsNullOrWhiteSpace($value) -or
        $value.Length -gt $MaximumLength -or $value.Contains([char]0)) {
        throw "Invalid string field: $Name"
    }
    return [string]$value
}

function Assert-NoDuplicateJsonProperties {
    param([Parameter(Mandatory = $true)][Text.Json.JsonElement]$Element)

    if ($Element.ValueKind -eq [Text.Json.JsonValueKind]::Object) {
        $names = [Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
        foreach ($property in $Element.EnumerateObject()) {
            if (-not $names.Add($property.Name)) {
                throw "Duplicate JSON property: $($property.Name)"
            }
            Assert-NoDuplicateJsonProperties -Element $property.Value
        }
    } elseif ($Element.ValueKind -eq [Text.Json.JsonValueKind]::Array) {
        foreach ($item in $Element.EnumerateArray()) {
            Assert-NoDuplicateJsonProperties -Element $item
        }
    }
}

function ConvertFrom-StrictJsonText {
    param(
        [Parameter(Mandatory = $true)][string]$Text,
        [Parameter(Mandatory = $true)][string]$Label
    )

    if ([string]::IsNullOrWhiteSpace($Text)) {
        throw "$Label is empty"
    }
    $options = [Text.Json.JsonDocumentOptions]::new()
    $options.AllowTrailingCommas = $false
    $options.CommentHandling = [Text.Json.JsonCommentHandling]::Disallow
    $options.MaxDepth = 64
    try {
        $document = [Text.Json.JsonDocument]::Parse($Text, $options)
    } catch {
        throw "$Label is invalid JSON: $($_.Exception.Message)"
    }
    try {
        if ($document.RootElement.ValueKind -ne [Text.Json.JsonValueKind]::Object) {
            throw "$Label must be a JSON object"
        }
        Assert-NoDuplicateJsonProperties -Element $document.RootElement
    } finally {
        $document.Dispose()
    }
    try {
        return $Text | ConvertFrom-Json -Depth 64 -NoEnumerate -DateKind String -ErrorAction Stop
    } catch {
        throw "$Label could not be materialized: $($_.Exception.Message)"
    }
}

function Read-ExactBytes {
    param(
        [Parameter(Mandatory = $true)][IO.FileStream]$Stream,
        [Parameter(Mandatory = $true)][long]$Offset,
        [Parameter(Mandatory = $true)][int]$Count
    )

    [void]$Stream.Seek($Offset, [IO.SeekOrigin]::Begin)
    $bytes = [byte[]]::new($Count)
    $read = 0
    while ($read -lt $Count) {
        $next = $Stream.Read($bytes, $read, $Count - $read)
        if ($next -eq 0) {
            throw 'Transcript ended before its captured length.'
        }
        $read += $next
    }
    return $bytes
}

function Read-TranscriptSnapshot {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$ExpectedSessionId
    )

    $fullPath = [IO.Path]::GetFullPath($Path)
    if ([IO.Path]::GetExtension($fullPath) -cne '.jsonl') {
        throw 'Transcript path must name a .jsonl file.'
    }
    $item = Get-Item -LiteralPath $fullPath -Force -ErrorAction Stop
    if ($item -isnot [IO.FileInfo] -or
        ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
        throw 'Transcript must be an ordinary non-reparse file.'
    }

    $share = [IO.FileShare]::ReadWrite -bor [IO.FileShare]::Delete
    $stream = [IO.File]::Open($fullPath, [IO.FileMode]::Open, [IO.FileAccess]::Read, $share)
    try {
        $length = $stream.Length
        if ($length -le 0) { throw 'Transcript is empty.' }

        $prefixCount = [int][Math]::Min($length, $script:MaximumSessionMetadataBytes)
        $prefix = Read-ExactBytes -Stream $stream -Offset 0 -Count $prefixCount
        $prefixNewline = [Array]::IndexOf($prefix, [byte]10)
        if ($prefixNewline -lt 0) {
            throw 'Transcript session metadata exceeds the bounded prefix.'
        }
        $firstLine = $script:Utf8.GetString($prefix, 0, $prefixNewline).TrimEnd("`r")
        $sessionMeta = ConvertFrom-StrictJsonText -Text $firstLine -Label 'Transcript session metadata'
        if ((Get-RequiredString -Object $sessionMeta -Name 'type') -cne 'session_meta' -or
            -not (Test-ObjectProperty -Object $sessionMeta -Name 'payload')) {
            throw 'Transcript does not begin with session_meta.'
        }
        $metaSession = if (Test-ObjectProperty -Object $sessionMeta.payload -Name 'session_id') {
            Get-RequiredString -Object $sessionMeta.payload -Name 'session_id'
        } else {
            Get-RequiredString -Object $sessionMeta.payload -Name 'id'
        }
        if ($metaSession -cne $ExpectedSessionId) {
            throw 'Transcript session id differs from the Hook event.'
        }

        $tailStart = [Math]::Max(0L, $length - $script:MaximumTranscriptTailBytes)
        $tailCount = [int]($length - $tailStart)
        $tail = Read-ExactBytes -Stream $stream -Offset $tailStart -Count $tailCount
    } finally {
        $stream.Dispose()
    }

    $tailOffset = 0
    if ($tailStart -gt 0) {
        $firstTailNewline = [Array]::IndexOf($tail, [byte]10)
        if ($firstTailNewline -lt 0) {
            throw 'Transcript tail contains no complete JSONL record.'
        }
        $tailOffset = $firstTailNewline + 1
    }
    $tailText = $script:Utf8.GetString($tail, $tailOffset, $tail.Length - $tailOffset)
    $tailEndsAtRecord = $tail.Length -gt $tailOffset -and $tail[$tail.Length - 1] -eq 10
    $lines = @($tailText -split "`n")
    if (-not $tailEndsAtRecord -and $lines.Count -gt 0) {
        $lines = if ($lines.Count -eq 1) { @() } else { @($lines[0..($lines.Count - 2)]) }
    }

    $records = [Collections.Generic.List[object]]::new()
    $lastOrdinal = [long]::MinValue
    foreach ($rawLine in $lines) {
        $line = $rawLine.TrimEnd("`r")
        if ([string]::IsNullOrWhiteSpace($line)) { continue }
        $record = ConvertFrom-StrictJsonText -Text $line -Label 'Transcript JSONL record'
        if (-not (Test-ObjectProperty -Object $record -Name 'ordinal') -or
            $record.ordinal -isnot [ValueType]) {
            throw 'Transcript record is missing a numeric ordinal.'
        }
        $ordinal = [long]$record.ordinal
        if ($ordinal -le $lastOrdinal) {
            throw 'Transcript ordinals are not strictly increasing in the bounded window.'
        }
        $lastOrdinal = $ordinal
        $records.Add($record)
    }
    if ($records.Count -eq 0) { throw 'Transcript tail has no complete records.' }

    return [pscustomobject]@{
        SessionId = $metaSession
        Path = $fullPath
        PathSha256 = Get-TextSha256 -Text $fullPath
        SnapshotLength = $length
        Records = @($records)
    }
}

function Get-RecordTurnId {
    param([Parameter(Mandatory = $true)]$Record)

    if (-not (Test-ObjectProperty -Object $Record -Name 'payload')) { return $null }
    $payload = $Record.payload
    if (-not (Test-ObjectProperty -Object $payload -Name 'internal_chat_message_metadata_passthrough')) {
        return $null
    }
    $metadata = $payload.internal_chat_message_metadata_passthrough
    if (-not (Test-ObjectProperty -Object $metadata -Name 'turn_id') -or
        $metadata.turn_id -isnot [string]) {
        return $null
    }
    return [string]$metadata.turn_id
}

function Get-RecordTimestamp {
    param([Parameter(Mandatory = $true)]$Record)

    $text = Get-RequiredString -Object $Record -Name 'timestamp'
    $parsed = [DateTimeOffset]::MinValue
    if (-not [DateTimeOffset]::TryParse(
            $text,
            [Globalization.CultureInfo]::InvariantCulture,
            [Globalization.DateTimeStyles]::AssumeUniversal,
            [ref]$parsed)) {
        throw 'Transcript record timestamp is invalid.'
    }
    return $parsed.ToUniversalTime()
}

function Convert-WaitToolInput {
    param([Parameter(Mandatory = $true)]$InputObject)

    if ($InputObject -is [array] -or $InputObject -isnot [pscustomobject]) {
        throw 'wait tool_input must be a JSON object.'
    }
    $allowed = @('cell_id', 'max_tokens', 'yield_time_ms')
    foreach ($name in @($InputObject.PSObject.Properties.Name)) {
        if ($name -cnotin $allowed) { throw "Unexpected wait input field: $name" }
    }
    $cellId = Get-RequiredString -Object $InputObject -Name 'cell_id' -MaximumLength 128
    if ($cellId -notmatch '^[A-Za-z0-9][A-Za-z0-9._:-]{0,127}$') {
        throw 'wait cell_id has an invalid shape.'
    }
    foreach ($name in @('max_tokens', 'yield_time_ms')) {
        if (Test-ObjectProperty -Object $InputObject -Name $name) {
            $value = $InputObject.PSObject.Properties[$name].Value
            if ($value -isnot [ValueType] -or [long]$value -le 0) {
                throw "wait $name must be a positive integer."
            }
        }
    }
    return [pscustomobject]@{ CellId = $cellId }
}

function Convert-WaitArguments {
    param([Parameter(Mandatory = $true)]$Call)

    $arguments = Get-RequiredString -Object $Call -Name 'arguments' -MaximumLength 8192
    return Convert-WaitToolInput -InputObject (
        ConvertFrom-StrictJsonText -Text $arguments -Label 'wait transcript arguments'
    )
}

function New-WaitDecision {
    param(
        [Parameter(Mandatory = $true)][bool]$Allow,
        [Parameter(Mandatory = $true)][string]$Code,
        [Parameter(Mandatory = $true)][string]$Detail,
        [string]$CellId
    )

    $reason = if ($Allow) {
        "$Detail $($script:RouteContext)"
    } else {
        "functions.wait denied before executor dispatch [$Code]: $Detail $($script:RouteContext)"
    }
    return [pscustomobject]@{
        Allow = $Allow
        Code = $Code
        CellId = $CellId
        Reason = $reason
    }
}

function Get-WaitRouteDecision {
    param(
        [Parameter(Mandatory = $true)]$Event,
        $Transcript,
        [Parameter(Mandatory = $true)][DateTimeOffset]$Now,
        $ContinuationState
    )

    try {
        $eventName = Get-RequiredString -Object $Event -Name 'hook_event_name'
        $toolName = Get-RequiredString -Object $Event -Name 'tool_name'
        $sessionId = Get-RequiredString -Object $Event -Name 'session_id'
        $turnId = Get-RequiredString -Object $Event -Name 'turn_id'
        $toolUseId = Get-RequiredString -Object $Event -Name 'tool_use_id'
        if ($eventName -cne 'PreToolUse') {
            return New-WaitDecision -Allow $false -Code 'wrong_event' `
                -Detail "Expected PreToolUse, got $eventName."
        }
        if ($toolName -cne 'wait') {
            return New-WaitDecision -Allow $false -Code 'noncanonical_tool_name' `
                -Detail "Expected canonical tool_name=wait, got $toolName."
        }
        if (-not (Test-ObjectProperty -Object $Event -Name 'tool_input')) {
            throw 'Missing tool_input.'
        }
        $input = Convert-WaitToolInput -InputObject $Event.tool_input
        if ($null -eq $Transcript) {
            return New-WaitDecision -Allow $false -Code 'no_transcript' `
                -Detail 'No current transcript snapshot is available.' -CellId $input.CellId
        }
        if ($Transcript.SessionId -cne $sessionId) {
            return New-WaitDecision -Allow $false -Code 'session_mismatch' `
                -Detail 'Transcript and Hook session ids differ.' -CellId $input.CellId
        }

        $records = @($Transcript.Records | Sort-Object { [long]$_.ordinal })
        $currentCalls = @($records | Where-Object {
                $_.type -ceq 'response_item' -and
                (Get-RecordTurnId -Record $_) -ceq $turnId -and
                $_.payload.type -ceq 'function_call' -and
                $_.payload.name -ceq 'wait' -and
                $_.payload.call_id -ceq $toolUseId
            })
        if ($currentCalls.Count -gt 1) {
            throw 'Current wait call appears more than once in the transcript.'
        }
        $currentOrdinal = if ($currentCalls.Count -eq 1) {
            $callInput = Convert-WaitArguments -Call $currentCalls[0].payload
            if ($callInput.CellId -cne $input.CellId) {
                throw 'Current transcript wait input differs from Hook input.'
            }
            [long]$currentCalls[0].ordinal
        } else {
            [long]::MaxValue
        }

        $execCalls = @{}
        $yield = $null
        foreach ($record in $records) {
            if ([long]$record.ordinal -ge $currentOrdinal -or
                $record.type -cne 'response_item' -or
                (Get-RecordTurnId -Record $record) -cne $turnId) {
                continue
            }
            $payload = $record.payload
            if ($payload.type -ceq 'custom_tool_call' -and
                $payload.name -cin @('exec', 'functions.exec')) {
                $callId = Get-RequiredString -Object $payload -Name 'call_id'
                $execCalls[$callId] = $true
                continue
            }
            if ($payload.type -cne 'custom_tool_call_output' -or
                -not (Test-ObjectProperty -Object $payload -Name 'call_id') -or
                -not $execCalls.ContainsKey([string]$payload.call_id) -or
                -not (Test-ObjectProperty -Object $payload -Name 'output') -or
                $payload.output -isnot [string]) {
                continue
            }
            $match = [regex]::Match(
                [string]$payload.output,
                '\AScript running with cell ID (?<id>[A-Za-z0-9][A-Za-z0-9._:-]{0,127})\r?\n',
                [Text.RegularExpressions.RegexOptions]::CultureInvariant)
            if ($match.Success) {
                $yield = [pscustomobject]@{
                    CellId = $match.Groups['id'].Value
                    CallId = [string]$payload.call_id
                    Ordinal = [long]$record.ordinal
                    Timestamp = Get-RecordTimestamp -Record $record
                }
            }
        }
        if ($null -eq $yield) {
            return New-WaitDecision -Allow $false -Code 'no_structured_yield' `
                -Detail 'No structured functions.exec yielded-cell output exists in the current turn.' `
                -CellId $input.CellId
        }
        if ($yield.CellId -cne $input.CellId) {
            return New-WaitDecision -Allow $false -Code 'cell_id_mismatch' `
                -Detail "Requested cell $($input.CellId), but the latest yielded cell is $($yield.CellId)." `
                -CellId $input.CellId
        }
        $age = $Now.ToUniversalTime() - $yield.Timestamp
        if ($age -lt [TimeSpan]::FromSeconds(-30) -or $age -gt $script:MaximumYieldAge) {
            return New-WaitDecision -Allow $false -Code 'yield_expired' `
                -Detail 'The yielded-cell evidence is outside the allowed current-turn age.' `
                -CellId $input.CellId
        }

        $priorCalls = @($records | Where-Object {
                [long]$_.ordinal -gt $yield.Ordinal -and
                [long]$_.ordinal -lt $currentOrdinal -and
                $_.type -ceq 'response_item' -and
                (Get-RecordTurnId -Record $_) -ceq $turnId -and
                $_.payload.type -cin @('custom_tool_call', 'function_call')
            })
        foreach ($callRecord in $priorCalls) {
            $payload = $callRecord.payload
            if ($payload.type -cne 'function_call' -or $payload.name -cne 'wait') {
                return New-WaitDecision -Allow $false -Code 'intervening_tool_call' `
                    -Detail 'Another tool call occurred after the yielded-cell output.' `
                    -CellId $input.CellId
            }
            $priorInput = Convert-WaitArguments -Call $payload
            if ($priorInput.CellId -cne $input.CellId) {
                return New-WaitDecision -Allow $false -Code 'prior_wait_mismatch' `
                    -Detail 'A different wait request already consumed the yielded-cell frontier.' `
                    -CellId $input.CellId
            }
            $priorOutput = @($records | Where-Object {
                    [long]$_.ordinal -gt [long]$callRecord.ordinal -and
                    [long]$_.ordinal -lt $currentOrdinal -and
                    $_.type -ceq 'response_item' -and
                    $_.payload.type -ceq 'function_call_output' -and
                    $_.payload.call_id -ceq $payload.call_id
                })
            if ($priorOutput.Count -ne 1) {
                return New-WaitDecision -Allow $false -Code 'prior_wait_incomplete' `
                    -Detail 'A prior wait has no unique completed Hook-visible output record.' `
                    -CellId $input.CellId
            }
        }

        if ($priorCalls.Count -eq 0) {
            return New-WaitDecision -Allow $true -Code 'fresh_yield' `
                -Detail "Validated current yielded cell $($input.CellId)." -CellId $input.CellId
        }

        $latestPrior = $priorCalls[-1].payload
        if ($null -eq $ContinuationState) {
            return New-WaitDecision -Allow $false -Code 'cell_consumed_or_completed' `
                -Detail 'A prior wait consumed this cell and no PostToolUse running proof exists.' `
                -CellId $input.CellId
        }
        $stateFields = @(
                'schema', 'session_id', 'turn_id', 'transcript_path_sha256',
                'cell_id', 'tool_use_id', 'status', 'marker', 'response_sha256',
                'observed_at_utc')
        if ((@($ContinuationState.PSObject.Properties.Name | Sort-Object) -join ',') -cne
            (@($stateFields | Sort-Object) -join ',')) {
            throw 'Continuation state has an unexpected field set.'
        }
        foreach ($required in $stateFields) {
            if (-not (Test-ObjectProperty -Object $ContinuationState -Name $required)) {
                throw "Continuation state is missing $required."
            }
        }
        $observedAt = [DateTimeOffset]::MinValue
        if (-not [DateTimeOffset]::TryParse(
                [string]$ContinuationState.observed_at_utc,
                [Globalization.CultureInfo]::InvariantCulture,
                [Globalization.DateTimeStyles]::AssumeUniversal,
                [ref]$observedAt)) {
            throw 'Continuation state timestamp is invalid.'
        }
        $continuationAge = $Now.ToUniversalTime() - $observedAt.ToUniversalTime()
        if ($ContinuationState.schema -cne 'rayman.codex-wait-route-state.v1' -or
            $ContinuationState.session_id -cne $sessionId -or
            $ContinuationState.turn_id -cne $turnId -or
            $ContinuationState.transcript_path_sha256 -cne $Transcript.PathSha256 -or
            $ContinuationState.cell_id -cne $input.CellId -or
            $ContinuationState.tool_use_id -cne $latestPrior.call_id -or
            $ContinuationState.status -cne 'running' -or
            $ContinuationState.marker -cne "Script running with cell ID $($input.CellId)" -or
            $ContinuationState.response_sha256 -notmatch '^[0-9a-f]{64}$' -or
            $continuationAge -lt [TimeSpan]::FromSeconds(-30) -or
            $continuationAge -gt $script:MaximumContinuationAge) {
            return New-WaitDecision -Allow $false -Code 'cell_not_active' `
                -Detail 'The latest prior wait is completed, failed, unknown, stale, or not bound to this cell.' `
                -CellId $input.CellId
        }
        return New-WaitDecision -Allow $true -Code 'continued_yield' `
            -Detail "PostToolUse proves cell $($input.CellId) is still running." -CellId $input.CellId
    } catch {
        return New-WaitDecision -Allow $false -Code 'malformed_evidence' `
            -Detail $_.Exception.Message
    }
}

function Get-StateRoot {
    $repoRoot = Split-Path -Parent $PSScriptRoot
    $stateRoot = [IO.Path]::GetFullPath((Join-Path $repoRoot '.RaymanCodingSkill\tmp\codex-wait-route-guard'))
    foreach ($path in @(
            $repoRoot,
            (Join-Path $repoRoot '.RaymanCodingSkill'),
            (Join-Path $repoRoot '.RaymanCodingSkill\tmp'))) {
        if (Test-Path -LiteralPath $path) {
            $item = Get-Item -LiteralPath $path -Force
            if (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
                throw "Wait-route state ancestor is a reparse point: $path"
            }
        }
    }
    [void][IO.Directory]::CreateDirectory($stateRoot)
    foreach ($path in @(
            $repoRoot,
            (Join-Path $repoRoot '.RaymanCodingSkill'),
            (Join-Path $repoRoot '.RaymanCodingSkill\tmp'),
            $stateRoot)) {
        $item = Get-Item -LiteralPath $path -Force
        if ($item -isnot [IO.DirectoryInfo] -or
            ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
            throw "Wait-route state path is not an ordinary directory: $path"
        }
    }
    return $stateRoot
}

function Get-StatePath {
    param([Parameter(Mandatory = $true)]$Event)

    $sessionId = Get-RequiredString -Object $Event -Name 'session_id'
    $turnId = Get-RequiredString -Object $Event -Name 'turn_id'
    $transcriptPath = Get-RequiredString -Object $Event -Name 'transcript_path' -MaximumLength 32768
    $key = Get-TextSha256 -Text "$sessionId`n$turnId`n$([IO.Path]::GetFullPath($transcriptPath))"
    return Join-Path (Get-StateRoot) "state-$key.json"
}

function Read-ContinuationState {
    param([Parameter(Mandatory = $true)]$Event)

    $path = Get-StatePath -Event $Event
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) { return $null }
    $item = Get-Item -LiteralPath $path -Force
    if (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0 -or $item.Length -gt 16KB) {
        throw 'Continuation state is not an ordinary bounded file.'
    }
    return ConvertFrom-StrictJsonText -Text ([IO.File]::ReadAllText($path, $script:Utf8)) `
        -Label 'Continuation state'
}

function Write-BoundJsonFile {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)]$Document,
        [switch]$CreateOnly
    )

    $json = $Document | ConvertTo-Json -Depth 16 -Compress
    $bytes = $script:Utf8.GetBytes($json + "`n")
    $parent = [IO.Path]::GetDirectoryName([IO.Path]::GetFullPath($Path))
    $parentItem = Get-Item -LiteralPath $parent -Force -ErrorAction Stop
    if ($parentItem -isnot [IO.DirectoryInfo] -or
        ($parentItem.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
        throw 'Bound JSON parent must be an ordinary directory.'
    }
    $writePath = if ($CreateOnly) {
        $Path
    } else {
        Join-Path $parent ('.wait-route-write-' + [Guid]::NewGuid().ToString('N') + '.tmp')
    }
    try {
        $stream = [IO.File]::Open($writePath, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write, [IO.FileShare]::Read)
        try {
            $stream.Write($bytes, 0, $bytes.Length)
            $stream.Flush($true)
        } finally {
            $stream.Dispose()
        }
        if (-not $CreateOnly) {
            if (Test-Path -LiteralPath $Path) {
                $existingItem = Get-Item -LiteralPath $Path -Force
                if ($existingItem -isnot [IO.FileInfo] -or
                    ($existingItem.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0 -or
                    $existingItem.Length -gt 16KB) {
                    throw 'Existing bound JSON destination is not an ordinary bounded file.'
                }
            }
            [IO.File]::Move($writePath, $Path, $true)
        }
    } catch [IO.IOException] {
        if (-not $CreateOnly -or -not (Test-Path -LiteralPath $Path -PathType Leaf)) { throw }
        $existingItem = Get-Item -LiteralPath $Path -Force
        if (($existingItem.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0 -or
            $existingItem.Length -gt 64KB) {
            throw
        }
        $existing = [IO.File]::ReadAllBytes($Path)
        if (-not [Linq.Enumerable]::SequenceEqual[byte]($existing, $bytes)) { throw }
    } finally {
        if (-not $CreateOnly -and (Test-Path -LiteralPath $writePath -PathType Leaf)) {
            Remove-Item -LiteralPath $writePath -Force -ErrorAction SilentlyContinue
        }
    }
}

function Write-Observation {
    param(
        [Parameter(Mandatory = $true)]$Event,
        [Parameter(Mandatory = $true)][string]$CurrentMode,
        [string]$DecisionCode
    )

    $root = Get-StateRoot
    $observationRoot = Join-Path $root 'observations'
    [void][IO.Directory]::CreateDirectory($observationRoot)
    $toolUseId = if (Test-ObjectProperty -Object $Event -Name 'tool_use_id') {
        [string]$Event.tool_use_id
    } else {
        Get-TextSha256 -Text ($Event | ConvertTo-Json -Depth 8 -Compress)
    }
    $identity = Get-TextSha256 -Text (
        "$([string]$Event.session_id)`n$([string]$Event.hook_event_name)`n$toolUseId"
    )
    $transcriptHash = if ((Test-ObjectProperty -Object $Event -Name 'transcript_path') -and
        $Event.transcript_path -is [string] -and
        -not [string]::IsNullOrWhiteSpace($Event.transcript_path)) {
        Get-TextSha256 -Text ([IO.Path]::GetFullPath([string]$Event.transcript_path))
    } else { $null }
    $document = [ordered]@{
        schema = 'rayman.codex-wait-route-observation.v1'
        observed_at_utc = [DateTimeOffset]::UtcNow.ToString('O')
        mode = $CurrentMode
        hook_event_name = [string]$Event.hook_event_name
        tool_name = if (Test-ObjectProperty -Object $Event -Name 'tool_name') { [string]$Event.tool_name } else { $null }
        session_id = [string]$Event.session_id
        turn_id = if (Test-ObjectProperty -Object $Event -Name 'turn_id') { [string]$Event.turn_id } else { $null }
        tool_use_id = if (Test-ObjectProperty -Object $Event -Name 'tool_use_id') { [string]$Event.tool_use_id } else { $null }
        transcript_path_sha256 = $transcriptHash
        decision_code = $DecisionCode
    }
    $observationName = "observation-$identity-$([Guid]::NewGuid().ToString('N')).json"
    Write-BoundJsonFile -Path (Join-Path $observationRoot $observationName) `
        -Document $document -CreateOnly
}

function Get-PostToolResult {
    param(
        [Parameter(Mandatory = $true)]$Response,
        [Parameter(Mandatory = $true)][string]$CellId
    )

    $responseText = if ($Response -is [string]) {
        [string]$Response
    } elseif ($Response -isnot [array] -and $Response -is [pscustomobject] -and
        (Test-ObjectProperty -Object $Response -Name 'output') -and
        $Response.output -is [string]) {
        [string]$Response.output
    } else {
        return [pscustomobject]@{
            Status = 'unknown'
            Marker = $null
            ResponseSha256 = Get-TextSha256 -Text ($Response | ConvertTo-Json -Depth 32 -Compress)
        }
    }
    $marker = "Script running with cell ID $CellId"
    $status = if ($responseText -match (
            '\A' + [regex]::Escape($marker) + '(?:\r?\n|\z)')) {
        'running'
    } elseif ($responseText -match '(?i)exec cell .* not found') {
        'not_found'
    } elseif ($responseText -match '\AScript completed(?:\r?\n|\z)') {
        'completed'
    } elseif ($responseText -match '\AScript (?:failed|terminated)(?:\r?\n|\z)') {
        'failed'
    } else {
        'unknown'
    }
    return [pscustomobject]@{
        Status = $status
        Marker = if ($status -ceq 'running') { $marker } else { $null }
        ResponseSha256 = Get-TextSha256 -Text $responseText
    }
}

function Write-PostToolState {
    param([Parameter(Mandatory = $true)]$Event)

    $input = Convert-WaitToolInput -InputObject $Event.tool_input
    $result = Get-PostToolResult -Response $Event.tool_response -CellId $input.CellId
    $transcriptPath = Get-RequiredString -Object $Event -Name 'transcript_path' -MaximumLength 32768
    $document = [ordered]@{
        schema = 'rayman.codex-wait-route-state.v1'
        session_id = Get-RequiredString -Object $Event -Name 'session_id'
        turn_id = Get-RequiredString -Object $Event -Name 'turn_id'
        transcript_path_sha256 = Get-TextSha256 -Text ([IO.Path]::GetFullPath($transcriptPath))
        cell_id = $input.CellId
        tool_use_id = Get-RequiredString -Object $Event -Name 'tool_use_id'
        status = $result.Status
        marker = $result.Marker
        response_sha256 = $result.ResponseSha256
        observed_at_utc = [DateTimeOffset]::UtcNow.ToString('O')
    }
    Write-BoundJsonFile -Path (Get-StatePath -Event $Event) -Document $document
}

function Get-ContextHookOutput {
    param([Parameter(Mandatory = $true)][string]$EventName)

    if ($EventName -cin @('SessionStart', 'SubagentStart')) {
        return [ordered]@{
            hookSpecificOutput = [ordered]@{
                hookEventName = $EventName
                additionalContext = $script:RouteContext
            }
        }
    }
    if ($EventName -ceq 'PostCompact') {
        return [ordered]@{
            systemMessage = $script:RouteContext
        }
    }
    throw "Unsupported context Hook event: $EventName"
}

function Write-HookOutput {
    param([Parameter(Mandatory = $true)]$Document)

    [Console]::Out.Write(($Document | ConvertTo-Json -Depth 16 -Compress))
}

function Invoke-Hook {
    param(
        [Parameter(Mandatory = $true)][string]$CurrentMode,
        [Parameter(Mandatory = $true)][string]$ExpectedHash
    )

    $actualHash = (Get-FileHash -LiteralPath $PSCommandPath -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($actualHash -cne $ExpectedHash) {
        throw "Wait-route guard source hash mismatch: expected $ExpectedHash, got $actualHash"
    }
    $inputText = [Console]::In.ReadToEnd()
    $event = ConvertFrom-StrictJsonText -Text $inputText -Label 'Hook input'
    $eventName = Get-RequiredString -Object $event -Name 'hook_event_name'

    if ($eventName -cin @('SessionStart', 'PostCompact', 'SubagentStart')) {
        Write-Observation -Event $event -CurrentMode $CurrentMode -DecisionCode 'context_injected'
        Write-HookOutput -Document (Get-ContextHookOutput -EventName $eventName)
        return
    }
    if ($eventName -ceq 'PostToolUse') {
        $toolName = Get-RequiredString -Object $event -Name 'tool_name'
        if ($toolName -ceq 'wait') {
            Write-PostToolState -Event $event
        }
        Write-Observation -Event $event -CurrentMode $CurrentMode -DecisionCode 'post_tool_observed'
        return
    }
    if ($eventName -cne 'PreToolUse') {
        throw "Unsupported Hook event: $eventName"
    }

    $transcript = $null
    $transcriptError = $null
    try {
        $sessionId = Get-RequiredString -Object $event -Name 'session_id'
        $transcriptPath = Get-RequiredString -Object $event -Name 'transcript_path' -MaximumLength 32768
        $transcript = Read-TranscriptSnapshot -Path $transcriptPath -ExpectedSessionId $sessionId
    } catch {
        $transcriptError = $_.Exception.Message
    }
    $state = $null
    if ($null -ne $transcript) {
        try { $state = Read-ContinuationState -Event $event } catch { $state = $null }
    }
    $decision = if ($null -ne $transcriptError) {
        New-WaitDecision -Allow $false -Code 'transcript_unavailable' -Detail $transcriptError
    } else {
        Get-WaitRouteDecision -Event $event -Transcript $transcript `
            -Now ([DateTimeOffset]::UtcNow) -ContinuationState $state
    }
    Write-Observation -Event $event -CurrentMode $CurrentMode -DecisionCode $decision.Code

    if ($CurrentMode -ceq 'Observe') {
        Write-HookOutput -Document ([ordered]@{
                hookSpecificOutput = [ordered]@{
                    hookEventName = 'PreToolUse'
                    additionalContext = "Record-only wait coverage probe observed tool_name=$([string]$event.tool_name); no deny was applied. Candidate result=$($decision.Code). $($script:RouteContext)"
                }
            })
        return
    }
    if ($decision.Allow) {
        Write-HookOutput -Document ([ordered]@{
                hookSpecificOutput = [ordered]@{
                    hookEventName = 'PreToolUse'
                    additionalContext = $decision.Reason
                }
            })
        return
    }
    Write-HookOutput -Document ([ordered]@{
            hookSpecificOutput = [ordered]@{
                hookEventName = 'PreToolUse'
                permissionDecision = 'deny'
                permissionDecisionReason = $decision.Reason
            }
        })
}

function New-TestRecord {
    param(
        [Parameter(Mandatory = $true)][long]$Ordinal,
        [Parameter(Mandatory = $true)][DateTimeOffset]$Timestamp,
        [Parameter(Mandatory = $true)][string]$TurnId,
        [Parameter(Mandatory = $true)]$Payload
    )

    if (-not (Test-ObjectProperty -Object $Payload -Name 'internal_chat_message_metadata_passthrough')) {
        $Payload | Add-Member -NotePropertyName internal_chat_message_metadata_passthrough `
            -NotePropertyValue ([pscustomobject]@{ turn_id = $TurnId })
    }
    return [pscustomobject]@{
        timestamp = $Timestamp.ToUniversalTime().ToString('O')
        ordinal = $Ordinal
        type = 'response_item'
        payload = $Payload
    }
}

function New-TestFixture {
    param(
        [Parameter(Mandatory = $true)][DateTimeOffset]$Now,
        [string]$YieldOutput = "Script running with cell ID 42`nWall time 31.0 seconds`nOutput:`n",
        [DateTimeOffset]$YieldTimestamp = $Now.AddSeconds(-5),
        [string]$RequestedCell = '42',
        [switch]$IncludeCurrentCall
    )

    $sessionId = 'session-test'
    $turnId = 'turn-test'
    $pathHash = Get-TextSha256 -Text 'C:\fixture\rollout.jsonl'
    $records = [Collections.Generic.List[object]]::new()
    $records.Add((New-TestRecord -Ordinal 10 -Timestamp $YieldTimestamp -TurnId $turnId `
                -Payload ([pscustomobject]@{
                    type = 'custom_tool_call'; name = 'exec'; call_id = 'exec-call'; input = 'test'
                })))
    $records.Add((New-TestRecord -Ordinal 11 -Timestamp $YieldTimestamp -TurnId $turnId `
                -Payload ([pscustomobject]@{
                    type = 'custom_tool_call_output'; call_id = 'exec-call'; output = $YieldOutput
                })))
    if ($IncludeCurrentCall) {
        $records.Add((New-TestRecord -Ordinal 12 -Timestamp $Now -TurnId $turnId `
                    -Payload ([pscustomobject]@{
                        type = 'function_call'; name = 'wait'; call_id = 'current-wait'
                        arguments = (@{ cell_id = $RequestedCell } | ConvertTo-Json -Compress)
                    })))
    }
    return [pscustomobject]@{
        Event = [pscustomobject]@{
            hook_event_name = 'PreToolUse'
            tool_name = 'wait'
            session_id = $sessionId
            turn_id = $turnId
            tool_use_id = 'current-wait'
            transcript_path = 'C:\fixture\rollout.jsonl'
            tool_input = [pscustomobject]@{ cell_id = $RequestedCell }
        }
        Transcript = [pscustomobject]@{
            SessionId = $sessionId
            PathSha256 = $pathHash
            Records = @($records)
        }
    }
}

function Add-PriorWaitToFixture {
    param(
        [Parameter(Mandatory = $true)]$Fixture,
        [Parameter(Mandatory = $true)][DateTimeOffset]$Now,
        [string]$CellId = '42'
    )

    $records = [Collections.Generic.List[object]]::new()
    foreach ($record in @($Fixture.Transcript.Records | Where-Object { [long]$_.ordinal -lt 12 })) {
        $records.Add($record)
    }
    $records.Add((New-TestRecord -Ordinal 12 -Timestamp $Now.AddSeconds(-2) -TurnId 'turn-test' `
                -Payload ([pscustomobject]@{
                    type = 'function_call'; name = 'wait'; call_id = 'prior-wait'
                    arguments = (@{ cell_id = $CellId } | ConvertTo-Json -Compress)
                })))
    $records.Add((New-TestRecord -Ordinal 13 -Timestamp $Now.AddSeconds(-1) -TurnId 'turn-test' `
                -Payload ([pscustomobject]@{
                    type = 'function_call_output'; call_id = 'prior-wait'; output = ' '
                })))
    $records.Add((New-TestRecord -Ordinal 14 -Timestamp $Now -TurnId 'turn-test' `
                -Payload ([pscustomobject]@{
                    type = 'function_call'; name = 'wait'; call_id = 'current-wait'
                    arguments = (@{ cell_id = $Fixture.Event.tool_input.cell_id } | ConvertTo-Json -Compress)
                })))
    $Fixture.Transcript.Records = @($records)
    return $Fixture
}

function Assert-Accepted {
    param(
        [Parameter(Mandatory = $true)][string]$Label,
        [Parameter(Mandatory = $true)][scriptblock]$Action
    )

    $decision = & $Action
    if ($decision.Allow -ne $true) {
        throw "Self-test expected acceptance for ${Label}: $($decision.Code) $($decision.Reason)"
    }
}

function Assert-Rejected {
    param(
        [Parameter(Mandatory = $true)][string]$Label,
        [Parameter(Mandatory = $true)][scriptblock]$Action,
        [string]$ExpectedCode
    )

    $decision = & $Action
    if ($decision.Allow -ne $false -or
        (-not [string]::IsNullOrWhiteSpace($ExpectedCode) -and $decision.Code -cne $ExpectedCode) -or
        -not $decision.Reason.Contains('collaboration.list_agents({})', [StringComparison]::Ordinal) -or
        -not $decision.Reason.Contains('collaboration.wait_agent({"timeout_ms":180000})', [StringComparison]::Ordinal)) {
        throw "Self-test expected rejection for ${Label}: $($decision | ConvertTo-Json -Compress)"
    }
}

function Assert-ProjectHookConfig {
    param([string]$ConfigPath)

    $repoRoot = Split-Path -Parent $PSScriptRoot
    if ([string]::IsNullOrWhiteSpace($ConfigPath)) {
        $ConfigPath = Join-Path $repoRoot '.codex\hooks.json'
    }
    $config = ConvertFrom-StrictJsonText -Text ([IO.File]::ReadAllText($configPath, $script:Utf8)) `
        -Label 'Project Hook configuration'
    $expectedEvents = @('PostCompact', 'PostToolUse', 'PreToolUse', 'SessionStart', 'SubagentStart')
    if (-not (Test-ObjectProperty -Object $config -Name 'hooks') -or
        (@($config.hooks.PSObject.Properties.Name | Sort-Object) -join ',') -cne ($expectedEvents -join ',')) {
        throw 'Project Hook configuration has an unexpected event set.'
    }
    $actualHash = (Get-FileHash -LiteralPath $PSCommandPath -Algorithm SHA256).Hash.ToLowerInvariant()
    $modes = [Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
    foreach ($eventProperty in $config.hooks.PSObject.Properties) {
        foreach ($group in @($eventProperty.Value)) {
            foreach ($hook in @($group.hooks)) {
                if ($hook.type -cne 'command' -or
                    $hook.command -isnot [string] -or
                    $hook.commandWindows -isnot [string] -or
                    -not $hook.command.Contains($actualHash, [StringComparison]::Ordinal) -or
                    -not $hook.commandWindows.Contains($actualHash, [StringComparison]::Ordinal) -or
                    -not $hook.command.Contains('scripts/codex-wait-route-guard.ps1', [StringComparison]::Ordinal) -or
                    -not $hook.commandWindows.Contains('scripts\codex-wait-route-guard.ps1', [StringComparison]::Ordinal) -or
                    $hook.command.Contains('--dangerously-bypass-hook-trust', [StringComparison]::Ordinal) -or
                    $hook.commandWindows.Contains('--dangerously-bypass-hook-trust', [StringComparison]::Ordinal)) {
                    throw "Project Hook command is not fixed and source-hash-bound: $($eventProperty.Name)"
                }
                if ($hook.commandWindows -match '-Mode (Observe|Enforce)') {
                    [void]$modes.Add([string]$Matches[1])
                } else {
                    throw 'Project Hook command has no explicit mode.'
                }
            }
        }
    }
    if ($modes.Count -ne 1) { throw 'Project Hook handlers do not share one mode.' }
    if ($null -ne $config.hooks.PSObject.Properties['Stop']) {
        throw 'Project Hook configuration must not shadow or replace the user Stop Hook.'
    }
}

function Invoke-SelfTest {
    param([string]$ConfigPath)

    $now = [DateTimeOffset]::Parse('2026-09-03T10:00:00Z')

    Assert-Accepted -Label 'real yielded cell' -Action {
        $fixture = New-TestFixture -Now $now -IncludeCurrentCall
        Get-WaitRouteDecision -Event $fixture.Event -Transcript $fixture.Transcript `
            -Now $now -ContinuationState $null
    }
    Assert-Rejected -Label 'diagnostic string forgery' -ExpectedCode 'no_structured_yield' -Action {
        $fixture = New-TestFixture -Now $now -YieldOutput 'Script completed'
        $fixture.Transcript.Records += New-TestRecord -Ordinal 12 -Timestamp $now -TurnId 'turn-test' `
            -Payload ([pscustomobject]@{
                type = 'message'; role = 'assistant'
                content = @([pscustomobject]@{ type = 'output_text'; text = 'Script running with cell ID 42' })
            })
        Get-WaitRouteDecision -Event $fixture.Event -Transcript $fixture.Transcript `
            -Now $now -ContinuationState $null
    }
    Assert-Rejected -Label 'wrong numeric cell id' -ExpectedCode 'cell_id_mismatch' -Action {
        $fixture = New-TestFixture -Now $now -RequestedCell '41'
        Get-WaitRouteDecision -Event $fixture.Event -Transcript $fixture.Transcript `
            -Now $now -ContinuationState $null
    }
    Assert-Rejected -Label 'stale yielded cell' -ExpectedCode 'yield_expired' -Action {
        $fixture = New-TestFixture -Now $now -YieldTimestamp $now.AddMinutes(-11)
        Get-WaitRouteDecision -Event $fixture.Event -Transcript $fixture.Transcript `
            -Now $now -ContinuationState $null
    }
    Assert-Rejected -Label 'completed cell reuse' -ExpectedCode 'cell_consumed_or_completed' -Action {
        $fixture = Add-PriorWaitToFixture -Fixture (New-TestFixture -Now $now) -Now $now
        Get-WaitRouteDecision -Event $fixture.Event -Transcript $fixture.Transcript `
            -Now $now -ContinuationState $null
    }
    Assert-Rejected -Label 'failed exec output' -ExpectedCode 'no_structured_yield' -Action {
        $fixture = New-TestFixture -Now $now -YieldOutput "Script failed`nWall time 1.0 seconds"
        Get-WaitRouteDecision -Event $fixture.Event -Transcript $fixture.Transcript `
            -Now $now -ContinuationState $null
    }
    Assert-Rejected -Label 'retry after cell not found' -ExpectedCode 'cell_not_active' -Action {
        $fixture = Add-PriorWaitToFixture -Fixture (New-TestFixture -Now $now) -Now $now
        $state = [pscustomobject]@{
            schema = 'rayman.codex-wait-route-state.v1'; session_id = 'session-test'
            turn_id = 'turn-test'; transcript_path_sha256 = $fixture.Transcript.PathSha256
            cell_id = '42'; tool_use_id = 'prior-wait'; status = 'not_found'; marker = $null
            response_sha256 = ('0' * 64)
            observed_at_utc = $now.AddSeconds(-1).ToString('O')
        }
        Get-WaitRouteDecision -Event $fixture.Event -Transcript $fixture.Transcript `
            -Now $now -ContinuationState $state
    }
    Assert-Rejected -Label 'missing transcript' -ExpectedCode 'no_transcript' -Action {
        $fixture = New-TestFixture -Now $now
        Get-WaitRouteDecision -Event $fixture.Event -Transcript $null `
            -Now $now -ContinuationState $null
    }
    Assert-Rejected -Label 'malformed wait input' -ExpectedCode 'malformed_evidence' -Action {
        $fixture = New-TestFixture -Now $now
        $fixture.Event.tool_input = [pscustomobject]@{ cell_id = 42; intent = 'agent' }
        Get-WaitRouteDecision -Event $fixture.Event -Transcript $fixture.Transcript `
            -Now $now -ContinuationState $null
    }
    Assert-Rejected -Label 'agent intent uses collaboration' -ExpectedCode 'cell_id_mismatch' -Action {
        $fixture = New-TestFixture -Now $now -RequestedCell 'none'
        Get-WaitRouteDecision -Event $fixture.Event -Transcript $fixture.Transcript `
            -Now $now -ContinuationState $null
    }
    Assert-Rejected -Label 'noncanonical wait tool name' -ExpectedCode 'noncanonical_tool_name' -Action {
        $fixture = New-TestFixture -Now $now
        $fixture.Event.tool_name = 'functions.wait'
        Get-WaitRouteDecision -Event $fixture.Event -Transcript $fixture.Transcript `
            -Now $now -ContinuationState $null
    }
    Assert-Accepted -Label 'continued running cell' -Action {
        $fixture = Add-PriorWaitToFixture -Fixture (New-TestFixture -Now $now) -Now $now
        $state = [pscustomobject]@{
            schema = 'rayman.codex-wait-route-state.v1'; session_id = 'session-test'
            turn_id = 'turn-test'; transcript_path_sha256 = $fixture.Transcript.PathSha256
            cell_id = '42'; tool_use_id = 'prior-wait'; status = 'running'
            marker = 'Script running with cell ID 42'
            response_sha256 = ('1' * 64)
            observed_at_utc = $now.AddSeconds(-1).ToString('O')
        }
        Get-WaitRouteDecision -Event $fixture.Event -Transcript $fixture.Transcript `
            -Now $now -ContinuationState $state
    }
    Assert-Rejected -Label 'intervening tool makes cell stale' -ExpectedCode 'intervening_tool_call' -Action {
        $fixture = New-TestFixture -Now $now
        $fixture.Transcript.Records += New-TestRecord -Ordinal 12 -Timestamp $now -TurnId 'turn-test' `
            -Payload ([pscustomobject]@{ type = 'custom_tool_call'; name = 'exec'; call_id = 'other'; input = 'x' })
        Get-WaitRouteDecision -Event $fixture.Event -Transcript $fixture.Transcript `
            -Now $now -ContinuationState $null
    }
    Assert-Accepted -Label 'context reinjection outputs' -Action {
        foreach ($eventName in @('SessionStart', 'SubagentStart')) {
            $output = Get-ContextHookOutput -EventName $eventName
            if ($output.hookSpecificOutput.hookEventName -cne $eventName -or
                -not $output.hookSpecificOutput.additionalContext.Contains('collaboration.wait_agent', [StringComparison]::Ordinal)) {
                return New-WaitDecision -Allow $false -Code 'context_output' -Detail 'Context output mismatch.'
            }
        }
        $postCompact = Get-ContextHookOutput -EventName 'PostCompact'
        if (-not $postCompact.systemMessage.Contains('functions.wait routing rule', [StringComparison]::Ordinal)) {
            return New-WaitDecision -Allow $false -Code 'postcompact_output' -Detail 'PostCompact output mismatch.'
        }
        return New-WaitDecision -Allow $true -Code 'context_outputs' -Detail 'Context outputs are valid.'
    }
    Assert-Accepted -Label 'posttool running output classification' -Action {
        $result = Get-PostToolResult -Response ([pscustomobject]@{
                output = "Script running with cell ID 42`nWall time 10.0 seconds`nOutput:`n"
                unrelated = 'metadata'
            }) -CellId '42'
        if ($result.Status -cne 'running' -or $result.Marker -cne 'Script running with cell ID 42') {
            return New-WaitDecision -Allow $false -Code 'posttool_running' -Detail 'Running response was not recognized.'
        }
        return New-WaitDecision -Allow $true -Code 'posttool_running' -Detail 'Running response was recognized.'
    }
    Assert-Accepted -Label 'posttool diagnostic marker rejected' -Action {
        $result = Get-PostToolResult -Response ([pscustomobject]@{
                message = 'diagnostic says Script running with cell ID 42'
            }) -CellId '42'
        if ($result.Status -cne 'unknown' -or $null -ne $result.Marker) {
            return New-WaitDecision -Allow $false -Code 'posttool_diagnostic' -Detail 'Diagnostic marker was accepted.'
        }
        return New-WaitDecision -Allow $true -Code 'posttool_diagnostic' -Detail 'Diagnostic marker stayed unknown.'
    }
    Assert-Accepted -Label 'bounded JSONL transcript parsing' -Action {
        $fixtureRoot = Join-Path ([IO.Path]::GetTempPath()) (
            'rayman-wait-route-selftest-' + $PID + '-' + [Guid]::NewGuid().ToString('N'))
        $fixturePath = Join-Path $fixtureRoot 'rollout.jsonl'
        try {
            [void][IO.Directory]::CreateDirectory($fixtureRoot)
            $lines = @(
                ([ordered]@{
                        timestamp = $now.ToString('O'); ordinal = 0; type = 'session_meta'
                        payload = [ordered]@{ session_id = 'file-session'; id = 'file-session' }
                    } | ConvertTo-Json -Compress -Depth 8),
                ([ordered]@{
                        timestamp = $now.ToString('O'); ordinal = 1; type = 'response_item'
                        payload = [ordered]@{
                            type = 'custom_tool_call'; name = 'exec'; call_id = 'file-exec'; input = 'fixture'
                            internal_chat_message_metadata_passthrough = [ordered]@{ turn_id = 'file-turn' }
                        }
                    } | ConvertTo-Json -Compress -Depth 8),
                ([ordered]@{
                        timestamp = $now.ToString('O'); ordinal = 2; type = 'response_item'
                        payload = [ordered]@{
                            type = 'custom_tool_call_output'; call_id = 'file-exec'
                            output = "Script running with cell ID 42`nWall time 1.0 seconds`nOutput:`n"
                            internal_chat_message_metadata_passthrough = [ordered]@{ turn_id = 'file-turn' }
                        }
                    } | ConvertTo-Json -Compress -Depth 8)
            )
            [IO.File]::WriteAllText($fixturePath, (($lines -join "`n") + "`n"), $script:Utf8)
            $snapshot = Read-TranscriptSnapshot -Path $fixturePath -ExpectedSessionId 'file-session'
            $event = [pscustomobject]@{
                hook_event_name = 'PreToolUse'; tool_name = 'wait'; session_id = 'file-session'
                turn_id = 'file-turn'; tool_use_id = 'file-current'; transcript_path = $fixturePath
                tool_input = [pscustomobject]@{ cell_id = '42' }
            }
            return Get-WaitRouteDecision -Event $event -Transcript $snapshot -Now $now -ContinuationState $null
        } finally {
            if (Test-Path -LiteralPath $fixturePath -PathType Leaf) { [IO.File]::Delete($fixturePath) }
            if (Test-Path -LiteralPath $fixtureRoot -PathType Container) { [IO.Directory]::Delete($fixtureRoot, $false) }
        }
    }
    Assert-Rejected -Label 'malformed transcript record' -ExpectedCode 'malformed_transcript' -Action {
        $fixtureRoot = Join-Path ([IO.Path]::GetTempPath()) (
            'rayman-wait-route-selftest-' + $PID + '-' + [Guid]::NewGuid().ToString('N'))
        $fixturePath = Join-Path $fixtureRoot 'rollout.jsonl'
        try {
            [void][IO.Directory]::CreateDirectory($fixtureRoot)
            $meta = [ordered]@{
                timestamp = $now.ToString('O'); ordinal = 0; type = 'session_meta'
                payload = [ordered]@{ session_id = 'bad-session'; id = 'bad-session' }
            } | ConvertTo-Json -Compress -Depth 8
            [IO.File]::WriteAllText($fixturePath, ($meta + "`n{" + "`n"), $script:Utf8)
            [void](Read-TranscriptSnapshot -Path $fixturePath -ExpectedSessionId 'bad-session')
            return New-WaitDecision -Allow $true -Code 'unexpected_parse' -Detail 'Malformed transcript was accepted.'
        } catch {
            return New-WaitDecision -Allow $false -Code 'malformed_transcript' -Detail $_.Exception.Message
        } finally {
            if (Test-Path -LiteralPath $fixturePath -PathType Leaf) { [IO.File]::Delete($fixturePath) }
            if (Test-Path -LiteralPath $fixtureRoot -PathType Container) { [IO.Directory]::Delete($fixtureRoot, $false) }
        }
    }
    Assert-Rejected -Label 'malformed duplicate JSON' -ExpectedCode 'malformed_json' -Action {
        try {
            [void](ConvertFrom-StrictJsonText -Text '{"cell_id":"42","cell_id":"43"}' -Label 'fixture')
            return New-WaitDecision -Allow $true -Code 'unexpected_parse' -Detail 'Duplicate JSON was accepted.'
        } catch {
            return New-WaitDecision -Allow $false -Code 'malformed_json' -Detail $_.Exception.Message
        }
    }
    Assert-Accepted -Label 'project hook fixed binding' -Action {
        try {
            Assert-ProjectHookConfig -ConfigPath $ConfigPath
            return New-WaitDecision -Allow $true -Code 'config_bound' -Detail 'Project Hook config is bound.'
        } catch {
            return New-WaitDecision -Allow $false -Code 'config_unbound' -Detail $_.Exception.Message
        }
    }

    Write-Output 'codex-wait-route-guard.ps1 self-test passed.'
}

if ($DeterministicTest) {
    if (-not $DeterministicTest.IsPresent) {
        throw 'The DeterministicTest parameter set requires -DeterministicTest.'
    }
    Invoke-SelfTest -ConfigPath $DeterministicTestConfigPath
    exit 0
}

try {
    Invoke-Hook -CurrentMode $Mode -ExpectedHash $ExpectedScriptSha256
} catch {
    $message = "wait-route guard failed closed: $($_.Exception.Message) $($script:RouteContext)"
    if ($Mode -ceq 'Enforce') {
        [Console]::Error.WriteLine($message)
        exit 2
    }
    Write-HookOutput -Document ([ordered]@{ systemMessage = $message })
}
