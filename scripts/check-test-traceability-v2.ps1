[CmdletBinding()]
param(
    [string]$ManifestPath = (Join-Path (Split-Path -Parent $PSScriptRoot) 'governance/test-traceability.json'),
    [string]$InventoryPath = (Join-Path (Split-Path -Parent $PSScriptRoot) 'governance/first-party-test-inventory.json'),
    [switch]$SelfTest,
    [switch]$RuntimeInventory,
    [switch]$ListInventory,
    [switch]$Generate,
    [string]$DraftInventoryPath,
    [string]$DraftManifestPath,
    [string]$OutputDirectory,
    [switch]$Summary
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if ($PSVersionTable.PSVersion.Major -lt 7) {
    throw 'check-test-traceability.ps1 requires PowerShell 7+.'
}

$script:RepoRoot = [IO.Path]::GetFullPath((Split-Path -Parent $PSScriptRoot))
$script:ManifestRelativePath = 'governance/test-traceability.json'
$script:InventoryRelativePath = 'governance/first-party-test-inventory.json'
$script:Utf8 = [Text.UTF8Encoding]::new($false, $true)
$script:RustRoots = @(
    'crates/codex-global-execution/src',
    'crates/rayman/src',
    'crates/rayman/tests',
    'xtask/src',
    'evals/src',
    'evals/tasks'
)
$script:ExcludedPrefixes = @('.git/', '.RaymanCodingSkill/', 'target/', 'evals/.runs')
$script:RustTextCache = @{}
$script:RustCodeCache = @{}
$script:PowerShellAstCache = @{}

function Throw-TraceError {
    param(
        [Parameter(Mandatory = $true)][string]$Code,
        [Parameter(Mandatory = $true)][string]$Message
    )
    throw "[$Code] $Message"
}

function Get-Sha256Bytes {
    param([Parameter(Mandatory = $true)][byte[]]$Bytes)
    return [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData($Bytes)).ToLowerInvariant()
}

function Get-Sha256Text {
    param([Parameter(Mandatory = $true)][string]$Text)
    return Get-Sha256Bytes -Bytes $script:Utf8.GetBytes($Text)
}

function Get-FileBytes {
    param([Parameter(Mandatory = $true)][string]$Path)
    return [IO.File]::ReadAllBytes($Path)
}

function Assert-NoDuplicateJsonProperties {
    param(
        [Parameter(Mandatory = $true)][Text.Json.JsonElement]$Element,
        [Parameter(Mandatory = $true)][string]$JsonPath
    )
    if ($Element.ValueKind -eq [Text.Json.JsonValueKind]::Object) {
        $seen = [Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
        foreach ($property in $Element.EnumerateObject()) {
            if (-not $seen.Add($property.Name)) {
                Throw-TraceError 'TRACE_SCHEMA_DUPLICATE_KEY' "duplicate JSON property at $JsonPath.$($property.Name)"
            }
            Assert-NoDuplicateJsonProperties -Element $property.Value -JsonPath "$JsonPath.$($property.Name)"
        }
    } elseif ($Element.ValueKind -eq [Text.Json.JsonValueKind]::Array) {
        $index = 0
        foreach ($item in $Element.EnumerateArray()) {
            Assert-NoDuplicateJsonProperties -Element $item -JsonPath "$JsonPath[$index]"
            $index++
        }
    }
}

function ConvertFrom-StrictJsonBytes {
    param(
        [Parameter(Mandatory = $true)][byte[]]$Bytes,
        [Parameter(Mandatory = $true)][string]$Label
    )
    if ($Bytes.Length -ge 3 -and $Bytes[0] -eq 0xef -and $Bytes[1] -eq 0xbb -and $Bytes[2] -eq 0xbf) {
        Throw-TraceError 'TRACE_SCHEMA_UTF8' "$Label must be UTF-8 without BOM"
    }
    try {
        $text = $script:Utf8.GetString($Bytes)
        $document = [Text.Json.JsonDocument]::Parse($text)
    } catch {
        Throw-TraceError 'TRACE_SCHEMA_JSON' "$Label is not strict UTF-8 JSON: $($_.Exception.Message)"
    }
    try {
        Assert-NoDuplicateJsonProperties -Element $document.RootElement -JsonPath '$'
    } finally {
        $document.Dispose()
    }
    try {
        return $text | ConvertFrom-Json -Depth 100 -NoEnumerate -DateKind String -ErrorAction Stop
    } catch {
        Throw-TraceError 'TRACE_SCHEMA_JSON' "$Label cannot be decoded: $($_.Exception.Message)"
    }
}

function Assert-ExactProperties {
    param(
        [Parameter(Mandatory = $true)]$Value,
        [Parameter(Mandatory = $true)][string[]]$Expected,
        [Parameter(Mandatory = $true)][string]$Label
    )
    if ($Value -isnot [pscustomobject]) {
        Throw-TraceError 'TRACE_SCHEMA_TYPE' "$Label must be an object"
    }
    $actual = @($Value.PSObject.Properties.Name | Sort-Object)
    if (($actual -join ',') -cne (($Expected | Sort-Object) -join ',')) {
        Throw-TraceError 'TRACE_SCHEMA_UNKNOWN_FIELD' "$Label has unknown or missing fields"
    }
}

function Assert-TraceId {
    param(
        [Parameter(Mandatory = $true)][string]$Value,
        [Parameter(Mandatory = $true)][string]$Prefix,
        [Parameter(Mandatory = $true)][string]$Label
    )
    if ($Value -cnotmatch "^$Prefix-[A-Z0-9]+(?:-[A-Z0-9]+)*$") {
        Throw-TraceError 'TRACE_ID_INVALID' "$Label has an invalid stable ID: $Value"
    }
}

function ConvertFrom-TraceDate {
    param(
        [Parameter(Mandatory = $true)][string]$Value,
        [Parameter(Mandatory = $true)][string]$Label
    )
    $parsed = [DateTime]::MinValue
    if (-not [DateTime]::TryParseExact(
            $Value,
            'yyyy-MM-dd',
            [Globalization.CultureInfo]::InvariantCulture,
            [Globalization.DateTimeStyles]::None,
            [ref]$parsed
        )) {
        Throw-TraceError 'TRACE_DATE' "$Label must be strict yyyy-MM-dd"
    }
    return $parsed.Date
}

function ConvertTo-RelativeTracePath {
    param([Parameter(Mandatory = $true)][string]$Path)
    return [IO.Path]::GetRelativePath($script:RepoRoot, [IO.Path]::GetFullPath($Path)).Replace('\', '/')
}

function Resolve-TracePath {
    param(
        [Parameter(Mandatory = $true)][string]$RelativePath,
        [switch]$AllowMissing,
        [switch]$AllowDirectory
    )
    if ([string]::IsNullOrWhiteSpace($RelativePath) -or
        $RelativePath.Contains('\') -or
        [IO.Path]::IsPathRooted($RelativePath) -or
        @($RelativePath.Split('/') | Where-Object { $_ -in @('', '.', '..') }).Count -ne 0) {
        Throw-TraceError 'TRACE_PATH_ESCAPE' "traceability path is not canonical repository-relative: $RelativePath"
    }
    $full = [IO.Path]::GetFullPath((Join-Path $script:RepoRoot $RelativePath))
    $prefix = $script:RepoRoot.TrimEnd([IO.Path]::DirectorySeparatorChar) + [IO.Path]::DirectorySeparatorChar
    if (-not $full.StartsWith($prefix, [StringComparison]::OrdinalIgnoreCase)) {
        Throw-TraceError 'TRACE_PATH_ESCAPE' "traceability path escaped the repository: $RelativePath"
    }
    $current = $script:RepoRoot
    foreach ($segment in $RelativePath.Split('/')) {
        $current = Join-Path $current $segment
        if (Test-Path -LiteralPath $current) {
            $item = Get-Item -LiteralPath $current -Force
            if ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) {
                Throw-TraceError 'TRACE_PATH_REPARSE' "traceability path crosses a reparse point: $RelativePath"
            }
        }
    }
    if (-not $AllowMissing) {
        $pathType = if ($AllowDirectory) { 'Container' } else { 'Leaf' }
        if (-not (Test-Path -LiteralPath $full -PathType $pathType)) {
            Throw-TraceError 'TRACE_ARTIFACT_MISSING' "active traceability artifact is missing: $RelativePath"
        }
    }
    return $full
}

function Test-InventoryPathExcluded {
    param([Parameter(Mandatory = $true)][string]$RelativePath)
    $path = $RelativePath.Replace('\\', '/').Trim('/')
    $segments = @($path.Split('/'))
    if (@($segments | Where-Object { $_ -in @('.git', '.RaymanCodingSkill', 'target') }).Count -ne 0) {
        return $true
    }
    return $path -match '^evals/\.runs[^/]*(?:/|$)'
}

function Assert-OrdinaryInventoryEntry {
    param(
        [Parameter(Mandatory = $true)]$Entry,
        [Parameter(Mandatory = $true)][string]$RelativePath
    )
    if ($Entry.Attributes -band [IO.FileAttributes]::ReparsePoint) {
        Throw-TraceError 'TRACE_PATH_REPARSE' "inventory discovery encountered a reparse entry: $RelativePath"
    }
}

function Get-OrdinaryInventoryFiles {
    param(
        [Parameter(Mandatory = $true)][string]$RootRelativePath,
        [Parameter(Mandatory = $true)][string]$Extension
    )
    $root = Resolve-TracePath -RelativePath $RootRelativePath -AllowDirectory
    $rootItem = Get-Item -LiteralPath $root -Force
    Assert-OrdinaryInventoryEntry -Entry $rootItem -RelativePath $RootRelativePath
    $pending = [Collections.Generic.Stack[string]]::new()
    $pending.Push($root)
    $files = [Collections.Generic.List[string]]::new()
    while ($pending.Count -ne 0) {
        $directory = $pending.Pop()
        foreach ($entry in @(Get-ChildItem -LiteralPath $directory -Force | Sort-Object Name)) {
            $relative = ConvertTo-RelativeTracePath -Path $entry.FullName
            if ($entry.PSIsContainer -and (Test-InventoryPathExcluded -RelativePath $relative)) {
                continue
            }
            Assert-OrdinaryInventoryEntry -Entry $entry -RelativePath $relative
            if ($entry.PSIsContainer) {
                $pending.Push($entry.FullName)
            } elseif ([string]$entry.Extension -ceq $Extension) {
                $files.Add($entry.FullName)
            }
        }
    }
    $result = $files.ToArray()
    [Array]::Sort($result, [StringComparer]::OrdinalIgnoreCase)
    return $result
}

function Assert-FileHash {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$Sha256,
        [Parameter(Mandatory = $true)][string]$Label,
        [switch]$AllowMissing
    )
    if ($Sha256 -cnotmatch '^[0-9a-f]{64}$') {
        Throw-TraceError 'TRACE_HASH_FORMAT' "$Label has an invalid lowercase SHA-256"
    }
    $full = Resolve-TracePath -RelativePath $Path -AllowMissing:$AllowMissing
    if (Test-Path -LiteralPath $full -PathType Leaf) {
        $actual = (Get-FileHash -LiteralPath $full -Algorithm SHA256).Hash.ToLowerInvariant()
        if ($actual -cne $Sha256) {
            Throw-TraceError 'TRACE_ARTIFACT_HASH' "$Label byte identity drifted: $Path"
        }
    }
    return $full
}

function Clear-CharacterRange {
    param(
        [Parameter(Mandatory = $true)][char[]]$Characters,
        [Parameter(Mandatory = $true)][int]$Start,
        [Parameter(Mandatory = $true)][int]$End
    )
    for ($index = $Start; $index -le $End; $index++) {
        if ($Characters[$index] -notin @("`r", "`n")) { $Characters[$index] = ' ' }
    }
}

function Remove-RustNonCode {
    param([Parameter(Mandatory = $true)][string]$Text)
    $input = $Text.ToCharArray()
    $output = $Text.ToCharArray()
    $length = $input.Length
    $index = 0
    while ($index -lt $length) {
        if ($index + 1 -lt $length -and $input[$index] -eq '/' -and $input[$index + 1] -eq '/') {
            $end = $index + 2
            while ($end -lt $length -and $input[$end] -notin @("`r", "`n")) { $end++ }
            Clear-CharacterRange -Characters $output -Start $index -End ($end - 1)
            $index = $end
            continue
        }
        if ($index + 1 -lt $length -and $input[$index] -eq '/' -and $input[$index + 1] -eq '*') {
            $depth = 1
            $end = $index + 2
            while ($end -lt $length -and $depth -gt 0) {
                if ($end + 1 -lt $length -and $input[$end] -eq '/' -and $input[$end + 1] -eq '*') {
                    $depth++
                    $end += 2
                } elseif ($end + 1 -lt $length -and $input[$end] -eq '*' -and $input[$end + 1] -eq '/') {
                    $depth--
                    $end += 2
                } else {
                    $end++
                }
            }
            if ($depth -ne 0) { Throw-TraceError 'TRACE_RUST_LEXER' 'unterminated Rust block comment' }
            Clear-CharacterRange -Characters $output -Start $index -End ($end - 1)
            $index = $end
            continue
        }
        $rawCursor = -1
        if ($input[$index] -eq 'r') {
            $rawCursor = $index + 1
        } elseif ($index + 1 -lt $length -and $input[$index] -in @('b', 'c') -and $input[$index + 1] -eq 'r') {
            $rawCursor = $index + 2
        }
        if ($rawCursor -ge 0) {
            $hashes = 0
            while ($rawCursor + $hashes -lt $length -and $input[$rawCursor + $hashes] -eq '#') { $hashes++ }
            $quote = $rawCursor + $hashes
            if ($quote -lt $length -and $input[$quote] -eq '"') {
                $end = $quote + 1
                $closed = $false
                while ($end -lt $length) {
                    if ($input[$end] -eq '"') {
                        $matches = $true
                        for ($hash = 0; $hash -lt $hashes; $hash++) {
                            if ($end + 1 + $hash -ge $length -or $input[$end + 1 + $hash] -ne '#') {
                                $matches = $false
                                break
                            }
                        }
                        if ($matches) {
                            $end += 1 + $hashes
                            $closed = $true
                            break
                        }
                    }
                    $end++
                }
                if (-not $closed) { Throw-TraceError 'TRACE_RUST_LEXER' 'unterminated Rust raw string' }
                Clear-CharacterRange -Characters $output -Start $index -End ($end - 1)
                $index = $end
                continue
            }
        }
        $stringStart = $index
        $quoteIndex = -1
        if ($input[$index] -eq '"') {
            $quoteIndex = $index
        } elseif ($index + 1 -lt $length -and $input[$index] -in @('b', 'c') -and $input[$index + 1] -eq '"') {
            $quoteIndex = $index + 1
        }
        if ($quoteIndex -ge 0) {
            $end = $quoteIndex + 1
            $closed = $false
            while ($end -lt $length) {
                if ($input[$end] -eq '\') { $end += 2; continue }
                if ($input[$end] -eq '"') { $end++; $closed = $true; break }
                $end++
            }
            if (-not $closed) { Throw-TraceError 'TRACE_RUST_LEXER' 'unterminated Rust string' }
            Clear-CharacterRange -Characters $output -Start $stringStart -End ($end - 1)
            $index = $end
            continue
        }
        if ($input[$index] -eq "'") {
            $end = $index + 1
            if ($end -lt $length -and $input[$end] -eq '\') { $end += 2 } else { $end++ }
            if ($end -lt $length -and $input[$end] -eq "'") {
                Clear-CharacterRange -Characters $output -Start $index -End $end
                $index = $end + 1
                continue
            }
        }
        $index++
    }
    return [string]::new($output)
}

function Get-RustText {
    param([Parameter(Mandatory = $true)][string]$Path)
    if (-not $script:RustTextCache.ContainsKey($Path)) {
        $script:RustTextCache[$Path] = $script:Utf8.GetString((Get-FileBytes -Path $Path))
    }
    return [string]$script:RustTextCache[$Path]
}

function Get-RustCode {
    param([Parameter(Mandatory = $true)][string]$Path)
    if (-not $script:RustCodeCache.ContainsKey($Path)) {
        $script:RustCodeCache[$Path] = Remove-RustNonCode -Text (Get-RustText -Path $Path)
    }
    return [string]$script:RustCodeCache[$Path]
}

function Get-PowerShellAst {
    param([Parameter(Mandatory = $true)][string]$Path)
    if (-not $script:PowerShellAstCache.ContainsKey($Path)) {
        $tokens = $null
        $errors = $null
        $ast = [Management.Automation.Language.Parser]::ParseFile($Path, [ref]$tokens, [ref]$errors)
        if ($errors.Count -ne 0) {
            Throw-TraceError 'TRACE_POWERSHELL_AST' "PowerShell file does not parse: $Path"
        }
        $script:PowerShellAstCache[$Path] = $ast
    }
    return $script:PowerShellAstCache[$Path]
}

function Get-PowerShellAstFromText {
    param([Parameter(Mandatory = $true)][string]$Text)
    $tokens = $null
    $errors = $null
    $ast = [Management.Automation.Language.Parser]::ParseInput($Text, [ref]$tokens, [ref]$errors)
    if ($errors.Count -ne 0) { Throw-TraceError 'TRACE_POWERSHELL_AST' 'PowerShell self-test text does not parse' }
    return $ast
}

function Get-CommandLiteralParameter {
    param(
        [Parameter(Mandatory = $true)][Management.Automation.Language.CommandAst]$Command,
        [Parameter(Mandatory = $true)][string]$Name
    )
    for ($index = 0; $index -lt $Command.CommandElements.Count - 1; $index++) {
        $parameter = $Command.CommandElements[$index]
        $value = $Command.CommandElements[$index + 1]
        if ($parameter -is [Management.Automation.Language.CommandParameterAst] -and
            $parameter.ParameterName -ceq $Name -and
            $value -is [Management.Automation.Language.StringConstantExpressionAst]) {
            return [string]$value.Value
        }
    }
    return $null
}

function Get-SuggestedTestId {
    param(
        [Parameter(Mandatory = $true)][string]$Identity,
        [Parameter(Mandatory = $true)][string]$SelectorKind,
        [Parameter(Mandatory = $true)][string]$Selector
    )
    $kind = if ($SelectorKind.StartsWith('rust_', [StringComparison]::Ordinal)) { 'RUST' } else { 'PS' }
    $name = ($Selector.ToUpperInvariant() -replace '[^A-Z0-9]+', '-').Trim('-')
    if ($name.Length -gt 48) { $name = $name.Substring(0, 48).TrimEnd('-') }
    if ([string]::IsNullOrWhiteSpace($name)) { $name = 'CASE' }
    $suffix = (Get-Sha256Text -Text $Identity).Substring(0, 12).ToUpperInvariant()
    return "TEST-$kind-$name-$suffix"
}

function Get-RustRole {
    param([Parameter(Mandatory = $true)][string]$Path)
    if ($Path.StartsWith('crates/rayman/', [StringComparison]::Ordinal) -or
        $Path.StartsWith('crates/codex-global-execution/', [StringComparison]::Ordinal) -or
        $Path.StartsWith('xtask/', [StringComparison]::Ordinal)) { return 'root_cargo' }
    if ($Path.StartsWith('evals/src/', [StringComparison]::Ordinal)) { return 'eval_cargo' }
    if ($Path -match '^evals/tasks/[^/]+/fixture/') { return 'eval_fixture' }
    if ($Path -match '^evals/tasks/[^/]+/oracle/') { return 'eval_oracle' }
    Throw-TraceError 'TRACE_TEST_INVENTORY_UNCLASSIFIED' "Rust test is outside a supported first-party role: $Path"
}

function Get-EvalOwner {
    param([Parameter(Mandatory = $true)][string]$Path)
    $match = [regex]::Match($Path, '^evals/tasks/(?<owner>[^/]+)/')
    if ($match.Success) { return $match.Groups['owner'].Value }
    return $null
}

function Get-RustCfgExpression {
    param([Parameter(Mandatory = $true)][string]$Attributes)
    $values = @()
    foreach ($match in [regex]::Matches($Attributes, '(?ms)#\s*\[\s*cfg\s*\((?<cfg>.*?)\)\s*\]')) {
        $values += ($match.Groups['cfg'].Value -replace '\s+', '')
    }
    if ($values.Count -eq 0) { return 'all' }
    if ($values.Count -eq 1) { return [string]$values[0] }
    return "all($($values -join ','))"
}

function Get-RustTestInventory {
    $rows = @()
    $pattern = '(?ms)#\s*\[\s*test\s*\]\s*(?:#\s*\[[^\]]+\]\s*)*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?fn\s+(?<name>[A-Za-z_][A-Za-z0-9_]*)\s*\('
    foreach ($root in $script:RustRoots) {
        foreach ($file in @(Get-OrdinaryInventoryFiles -RootRelativePath $root -Extension '.rs')) {
            $path = ConvertTo-RelativeTracePath -Path $file
            $text = Get-RustText -Path $file
            $code = Get-RustCode -Path $file
            foreach ($match in [regex]::Matches($code, $pattern)) {
                $selector = $match.Groups['name'].Value
                $start = $match.Index
                $lineStart = $code.LastIndexOf("`n", [Math]::Max(0, $start - 1)) + 1
                $candidateStart = $lineStart
                while ($candidateStart -gt 0) {
                    $previousEnd = $candidateStart - 1
                    if ($previousEnd -gt 0 -and $code[$previousEnd - 1] -eq "`r") { $previousEnd-- }
                    $previousStart = $code.LastIndexOf("`n", [Math]::Max(0, $previousEnd - 1)) + 1
                    $previousLine = $code.Substring($previousStart, $previousEnd - $previousStart).Trim()
                    if ($previousLine -notmatch '^#\s*\[.*\]$') { break }
                    $candidateStart = $previousStart
                }
                $open = $code.IndexOf('{', $match.Index + $match.Length)
                if ($open -lt 0) { Throw-TraceError 'TRACE_RUST_LEXER' "test function has no body: $path::$selector" }
                $depth = 1
                $cursor = $open + 1
                while ($cursor -lt $code.Length -and $depth -gt 0) {
                    if ($code[$cursor] -eq '{') { $depth++ }
                    elseif ($code[$cursor] -eq '}') { $depth-- }
                    $cursor++
                }
                if ($depth -ne 0) { Throw-TraceError 'TRACE_RUST_LEXER' "test function body is unbalanced: $path::$selector" }
                $fragment = $text.Substring($candidateStart, $cursor - $candidateStart)
                $attributes = $text.Substring($candidateStart, $open - $candidateStart)
                $role = Get-RustRole -Path $path
                $identity = "rust::$role::$path::$selector"
                $rows += [pscustomobject]@{
                    identity = $identity
                    role = $role
                    path = $path
                    selector_kind = 'rust_test_fn'
                    selector = $selector
                    selector_sha256 = Get-Sha256Text -Text $fragment
                    cfg = Get-RustCfgExpression -Attributes $attributes
                    owner = Get-EvalOwner -Path $path
                    suggested_test_id = Get-SuggestedTestId -Identity $identity -SelectorKind 'rust_test_fn' -Selector $selector
                }
            }
        }
    }
    return @($rows)
}

function Test-HasSelfTestParameter {
    param([Parameter(Mandatory = $true)][Management.Automation.Language.ScriptBlockAst]$Ast)
    if ($null -eq $Ast.ParamBlock) { return $false }
    return @($Ast.ParamBlock.Parameters | Where-Object { $_.Name.VariablePath.UserPath -ceq 'SelfTest' }).Count -eq 1
}

function Get-EnclosingPowerShellFunctionName {
    param([Parameter(Mandatory = $true)][Management.Automation.Language.Ast]$Node)
    $current = $Node.Parent
    while ($null -ne $current) {
        if ($current -is [Management.Automation.Language.FunctionDefinitionAst]) {
            return [string]$current.Name
        }
        $current = $current.Parent
    }
    return $null
}

function Test-PowerShellAstUnderSelfTestGuard {
    param([Parameter(Mandatory = $true)][Management.Automation.Language.Ast]$Node)
    $current = $Node.Parent
    while ($null -ne $current) {
        if ($current -is [Management.Automation.Language.IfStatementAst]) {
            foreach ($clause in @($current.Clauses)) {
                $condition = $clause.Item1
                $body = $clause.Item2
                if ($Node.Extent.StartOffset -ge $body.Extent.StartOffset -and
                    $Node.Extent.EndOffset -le $body.Extent.EndOffset -and
                    @($condition.FindAll({
                                param($candidate)
                                $candidate -is [Management.Automation.Language.VariableExpressionAst] -and
                                [string]$candidate.VariablePath.UserPath -ceq 'SelfTest'
                            }, $true)).Count -ne 0) {
                    return $true
                }
            }
        }
        $current = $current.Parent
    }
    return $false
}

function Get-SelfTestReachablePowerShellFunctions {
    param([Parameter(Mandatory = $true)][Management.Automation.Language.ScriptBlockAst]$Ast)
    $definitions = @{}
    foreach ($definition in @($Ast.FindAll({
                    param($node) $node -is [Management.Automation.Language.FunctionDefinitionAst]
                }, $true))) {
        $definitions[[string]$definition.Name] = $definition
    }
    $reachable = [Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
    $pending = [Collections.Generic.Queue[string]]::new()
    foreach ($command in @($Ast.FindAll({
                    param($node) $node -is [Management.Automation.Language.CommandAst]
                }, $true))) {
        if ($null -ne (Get-EnclosingPowerShellFunctionName -Node $command) -or
            -not (Test-PowerShellAstUnderSelfTestGuard -Node $command)) { continue }
        $name = [string]$command.GetCommandName()
        if (-not [string]::IsNullOrWhiteSpace($name) -and $reachable.Add($name)) { $pending.Enqueue($name) }
    }
    while ($pending.Count -ne 0) {
        $name = $pending.Dequeue()
        if (-not $definitions.ContainsKey($name)) { continue }
        foreach ($command in @($definitions[$name].Body.FindAll({
                        param($node) $node -is [Management.Automation.Language.CommandAst]
                    }, $true))) {
            $enclosing = Get-EnclosingPowerShellFunctionName -Node $command
            if (-not [string]::Equals($enclosing, $name, [StringComparison]::OrdinalIgnoreCase)) { continue }
            $child = [string]$command.GetCommandName()
            if (-not [string]::IsNullOrWhiteSpace($child) -and $reachable.Add($child)) { $pending.Enqueue($child) }
        }
    }
    return ,$reachable
}

function Assert-PowerShellNamedCaseReachable {
    param(
        [Parameter(Mandatory = $true)][Management.Automation.Language.CommandAst]$Command,
        [Parameter(Mandatory = $true)]$ReachableFunctions,
        [Parameter(Mandatory = $true)][string]$Path
    )
    $enclosing = Get-EnclosingPowerShellFunctionName -Node $Command
    if (($null -eq $enclosing -and (Test-PowerShellAstUnderSelfTestGuard -Node $Command)) -or
        ($null -ne $enclosing -and $ReachableFunctions.Contains($enclosing))) {
        return
    }
    Throw-TraceError 'TRACE_POWERSHELL_CASE_UNREACHABLE' "PowerShell named test case is not reachable from -SelfTest: $Path::$($Command.Extent.Text)"
}

function New-PowerShellInventoryRow {
    param(
        [Parameter(Mandatory = $true)][string]$Role,
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$Kind,
        [Parameter(Mandatory = $true)][string]$Selector,
        [Parameter(Mandatory = $true)][string]$ExtentText,
        [AllowNull()][string]$Owner
    )
    $identity = "powershell::$Role::$Path::$Kind::$Selector"
    return [pscustomobject]@{
        identity = $identity
        role = $Role
        path = $Path
        selector_kind = $Kind
        selector = $Selector
        selector_sha256 = Get-Sha256Text -Text $ExtentText
        # These native backend suites require Windows token/ACL/task APIs.
        # Their inventory must not claim Linux or other-Unix runtime evidence.
        cfg = if ($Role -ceq 'powershell_self_test' -and $Path -cin @(
            'scripts/configure-global-codex-execution.ps1',
            'scripts/install-global-codex-execution.ps1',
            'scripts/enroll-global-codex-projects.ps1',
            'scripts/repair-codex-workspace-acl.ps1'
        )) { 'windows' } else { 'all' }
        owner = $Owner
        suggested_test_id = Get-SuggestedTestId -Identity $identity -SelectorKind $Kind -Selector $Selector
    }
}

function Get-PowerShellTestInventory {
    $rows = @()
    foreach ($file in @(Get-OrdinaryInventoryFiles -RootRelativePath 'scripts' -Extension '.ps1')) {
        $path = ConvertTo-RelativeTracePath -Path $file
        $ast = Get-PowerShellAst -Path $file
        $isDelegate = $path -ceq 'scripts/check-test-traceability-v2.ps1'
        $hasSelfTest = Test-HasSelfTestParameter -Ast $ast
        if (-not $hasSelfTest) { continue }
        $reachableFunctions = Get-SelfTestReachablePowerShellFunctions -Ast $ast
        if (-not $isDelegate) {
            $rows += New-PowerShellInventoryRow -Role 'powershell_self_test' -Path $path `
                -Kind 'powershell_self_test_suite' -Selector '-SelfTest' `
                -ExtentText $ast.Extent.Text -Owner $null
        }
        foreach ($command in @($ast.FindAll({ param($node) $node -is [Management.Automation.Language.CommandAst] }, $true))) {
            $commandName = [string]$command.GetCommandName()
            $parameter = $null
            if ($commandName -in @('Assert-Rejected', 'Assert-Accepted')) { $parameter = 'Label' }
            elseif ($commandName -ceq 'Invoke-InstallNamedSelfTest') { $parameter = 'Name' }
            elseif ($commandName -ceq 'Assert-SelfTestRejected') { $parameter = 'Code' }
            if ($null -eq $parameter) { continue }
            $value = Get-CommandLiteralParameter -Command $command -Name $parameter
            if ($null -eq $value) { continue }
            Assert-PowerShellNamedCaseReachable -Command $command -ReachableFunctions $reachableFunctions -Path $path
            $selector = "$commandName::$value"
            $rows += New-PowerShellInventoryRow -Role 'powershell_named_case' -Path $path `
                -Kind 'powershell_named_case' -Selector $selector `
                -ExtentText $command.Extent.Text -Owner $null
        }
    }

    $tasksRoot = Resolve-TracePath -RelativePath 'evals/tasks' -AllowDirectory
    foreach ($task in @(Get-ChildItem -LiteralPath $tasksRoot -Directory -Force | Sort-Object Name)) {
        $contractPath = Join-Path $task.FullName 'task.json'
        $contract = ConvertFrom-StrictJsonBytes -Bytes (Get-FileBytes -Path $contractPath) -Label "eval task $($task.Name)"
        if ([string]$contract.oracle.kind -ceq 'power_shell') {
            $oraclePath = Join-Path (Join-Path $task.FullName 'oracle') ([string]$contract.oracle.source)
            $relative = ConvertTo-RelativeTracePath -Path $oraclePath
            $ast = Get-PowerShellAst -Path $oraclePath
            $marker = [string]$contract.oracle.success_marker
            $matches = @($ast.FindAll({
                        param($node)
                        if ($node -isnot [Management.Automation.Language.CommandAst] -or
                            [string]$node.GetCommandName() -cne 'Write-Output') { return $false }
                        return @($node.CommandElements | Where-Object {
                                    $_ -is [Management.Automation.Language.StringConstantExpressionAst] -and
                                    [string]$_.Value -ceq $marker
                                }).Count -eq 1
                    }, $true))
            if ($matches.Count -ne 1) {
                Throw-TraceError 'TRACE_TASK_TEST_SET' "eval task $($task.Name) PowerShell oracle marker is missing or ambiguous"
            }
            $rows += New-PowerShellInventoryRow -Role 'eval_oracle' -Path $relative `
                -Kind 'powershell_success_marker' -Selector $marker `
                -ExtentText $matches[0].Extent.Text -Owner $task.Name
        }
        $fixtureTests = Join-Path $task.FullName 'fixture/tests'
        if (Test-Path -LiteralPath $fixtureTests -PathType Container) {
            $fixtureRelative = ConvertTo-RelativeTracePath -Path $fixtureTests
            foreach ($file in @(Get-OrdinaryInventoryFiles -RootRelativePath $fixtureRelative -Extension '.ps1')) {
                $relative = ConvertTo-RelativeTracePath -Path $file
                $rows += New-PowerShellInventoryRow -Role 'eval_fixture' -Path $relative `
                    -Kind 'powershell_script' -Selector '<script>' `
                    -ExtentText ([IO.File]::ReadAllText($file, $script:Utf8)) -Owner $task.Name
            }
        }
    }
    return @($rows)
}

function Get-FirstPartyTestInventory {
    $rows = @((Get-RustTestInventory) + (Get-PowerShellTestInventory) | Sort-Object identity)
    $seen = [Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
    foreach ($row in $rows) {
        if (-not $seen.Add([string]$row.identity)) {
            Throw-TraceError 'TRACE_TEST_INVENTORY_SET' "duplicate discovered test identity: $($row.identity)"
        }
    }
    return $rows
}

function Get-PowerShellSelectorCount {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$Kind,
        [Parameter(Mandatory = $true)][string]$Selector
    )
    $ast = Get-PowerShellAst -Path $Path
    if ($Kind -eq 'powershell_function') {
        return @($ast.FindAll({
                    param($node)
                    $node -is [Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -ceq $Selector
                }, $true)).Count
    }
    if ($Kind -eq 'powershell_self_test_suite') {
        return [int](Test-HasSelfTestParameter -Ast $ast)
    }
    if ($Kind -eq 'powershell_success_marker') {
        return @($ast.FindAll({
                    param($node)
                    if ($node -isnot [Management.Automation.Language.CommandAst] -or
                        [string]$node.GetCommandName() -cne 'Write-Output') { return $false }
                    return @($node.CommandElements | Where-Object {
                                $_ -is [Management.Automation.Language.StringConstantExpressionAst] -and
                                [string]$_.Value -ceq $Selector
                            }).Count -eq 1
                }, $true)).Count
    }
    Throw-TraceError 'TRACE_SELECTOR_KIND' "unsupported PowerShell selector kind: $Kind"
}

function Get-RustSymbolCount {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$Selector
    )
    $escaped = [regex]::Escape($Selector)
    return [regex]::Matches(
        (Get-RustCode -Path $Path),
        "(?m)^\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?fn\s+$escaped\s*\("
    ).Count
}

function Assert-Retirement {
    param(
        [AllowNull()]$Retirement,
        [Parameter(Mandatory = $true)][string]$Status,
        [Parameter(Mandatory = $true)][string]$Label
    )
    if ($Status -eq 'active') {
        if ($null -ne $Retirement) { Throw-TraceError 'TRACE_LINK_STATUS' "$Label is active but has retirement metadata" }
        return
    }
    if ($Status -ne 'retired' -or $null -eq $Retirement) {
        Throw-TraceError 'TRACE_LINK_STATUS' "$Label has an invalid lifecycle"
    }
    Assert-ExactProperties -Value $Retirement -Expected @('retired_at', 'reason', 'replaced_by') -Label "$Label retirement"
    $null = ConvertFrom-TraceDate -Value ([string]$Retirement.retired_at) -Label "$Label retired_at"
    if ([string]::IsNullOrWhiteSpace([string]$Retirement.reason) -or $Retirement.replaced_by -isnot [array]) {
        Throw-TraceError 'TRACE_LINK_STATUS' "$Label retirement is incomplete"
    }
    $seen = [Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
    foreach ($replacement in @($Retirement.replaced_by)) {
        if ($replacement -isnot [string] -or
            [string]$replacement -cnotmatch '^(?:SRC|RULE|GATE|TEST|ASSET|LINK|BIND)-[A-Z0-9]+(?:-[A-Z0-9]+)*$' -or
            -not $seen.Add([string]$replacement)) {
            Throw-TraceError 'TRACE_RETIREMENT_REPLACEMENT' "$Label has an invalid or duplicate replacement ID"
        }
    }
}

function Assert-Review {
    param(
        [Parameter(Mandatory = $true)]$Review,
        [Parameter(Mandatory = $true)][string]$Label,
        [Parameter(Mandatory = $true)][DateTime]$AsOf,
        [switch]$Active
    )
    Assert-ExactProperties -Value $Review -Expected @('reviewed_at', 'valid_until') -Label "$Label review"
    $reviewed = ConvertFrom-TraceDate -Value ([string]$Review.reviewed_at) -Label "$Label reviewed_at"
    $validUntil = ConvertFrom-TraceDate -Value ([string]$Review.valid_until) -Label "$Label valid_until"
    if ($reviewed -gt $AsOf.Date) { Throw-TraceError 'TRACE_DATE' "$Label review is dated in the future" }
    if ($validUntil -lt $reviewed) { Throw-TraceError 'TRACE_DATE' "$Label review expires before it begins" }
    if ($Active -and $validUntil -lt $AsOf.Date) {
        Throw-TraceError 'TRACE_SEMANTIC_LINK_EXPIRED' "$Label semantic review expired; retire it or create a newly reviewed replacement"
    }
}

function Get-NodeMap {
    param(
        [Parameter(Mandatory = $true)][AllowEmptyCollection()][array]$Items,
        [Parameter(Mandatory = $true)][string]$IdProperty,
        [Parameter(Mandatory = $true)][string]$Prefix,
        [Parameter(Mandatory = $true)][string]$Label
    )
    $map = @{}
    $previous = $null
    foreach ($item in $Items) {
        $id = [string]$item.$IdProperty
        Assert-TraceId -Value $id -Prefix $Prefix -Label $Label
        if ($map.ContainsKey($id)) { Throw-TraceError 'TRACE_ID_DUPLICATE' "duplicate $Label ID: $id" }
        if ($null -ne $previous -and [StringComparer]::Ordinal.Compare($previous, $id) -ge 0) {
            Throw-TraceError 'TRACE_ORDER_INVALID' "$Label entries are not sorted by ID"
        }
        $map[$id] = $item
        $previous = $id
    }
    return $map
}

function Get-ActiveReplacementNodes {
    param(
        [Parameter(Mandatory = $true)]$Node,
        [Parameter(Mandatory = $true)]$Map,
        [Parameter(Mandatory = $true)][string]$Label,
        [Collections.Generic.HashSet[string]]$Trail = ([Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal))
    )
    $active = [Collections.Generic.List[object]]::new()
    foreach ($replacementId in @($Node.retirement.replaced_by)) {
        $id = [string]$replacementId
        if (-not $Map.ContainsKey($id)) {
            Throw-TraceError 'TRACE_RETIREMENT_REPLACEMENT' "$Label references a missing replacement: $id"
        }
        if (-not $Trail.Add($id)) {
            Throw-TraceError 'TRACE_RETIREMENT_REPLACEMENT' "$Label replacement chain contains a cycle at: $id"
        }
        $replacement = $Map[$id]
        if ([string]$replacement.status -eq 'active') {
            $active.Add($replacement)
        } else {
            foreach ($terminal in @(Get-ActiveReplacementNodes -Node $replacement -Map $Map -Label $Label -Trail $Trail)) {
                $active.Add($terminal)
            }
        }
        $null = $Trail.Remove($id)
    }
    return @($active)
}

function Invoke-GitBytes {
    param([Parameter(Mandatory = $true)][string[]]$Arguments)
    $git = @(Get-Command git -All -ErrorAction SilentlyContinue | Where-Object CommandType -eq Application | Select-Object -First 1)
    if ($git.Count -ne 1) { Throw-TraceError 'TRACE_HISTORY_GIT' 'git must resolve directly to an application' }
    $start = [Diagnostics.ProcessStartInfo]::new()
    $start.FileName = $git[0].Source
    $start.WorkingDirectory = $script:RepoRoot
    $start.UseShellExecute = $false
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    foreach ($argument in $Arguments) { $null = $start.ArgumentList.Add($argument) }
    $process = [Diagnostics.Process]::Start($start)
    $stderrTask = $process.StandardError.ReadToEndAsync()
    $memory = [IO.MemoryStream]::new()
    $process.StandardOutput.BaseStream.CopyTo($memory)
    $process.WaitForExit()
    $stderr = $stderrTask.GetAwaiter().GetResult()
    $result = [pscustomobject]@{
        ExitCode = $process.ExitCode
        Bytes = $memory.ToArray()
        Stderr = $stderr
    }
    $memory.Dispose()
    $process.Dispose()
    return $result
}

function Assert-CanonicalManifestBytes {
    param([byte[]]$Bytes, [string]$Label)
    $text = $script:Utf8.GetString($Bytes)
    if ($text.StartsWith([string][char]0xFEFF, [StringComparison]::Ordinal) -or $text.Contains("`r")) {
        Throw-TraceError 'TRACE_NONCANONICAL_BYTES' "$Label must be UTF-8 without BOM and use LF before hashing; preserve exact byte bindings"
    }
}

function ConvertTo-CanonicalJsonBytes {
    param([Parameter(Mandatory = $true)]$Document)
    $text = ($Document | ConvertTo-Json -Depth 100).Replace("`r`n", "`n") + "`n"
    $bytes = $script:Utf8.GetBytes($text)
    Assert-CanonicalManifestBytes -Bytes $bytes -Label 'generated JSON'
    return ,$bytes
}

function New-TraceabilityCandidate {
    param([string]$InventoryDraft, [string]$ManifestDraft, [string]$Destination)
    if (-not $InventoryDraft -or -not $ManifestDraft -or -not [IO.Path]::IsPathFullyQualified($Destination)) {
        Throw-TraceError 'TRACE_GENERATE_ARGUMENTS' 'generation requires two explicit draft files and a new absolute output directory'
    }
    if (Test-Path -LiteralPath $Destination) { Throw-TraceError 'TRACE_GENERATE_DESTINATION' 'output directory already exists' }
    $null = & (Join-Path $PSScriptRoot 'source-bytes.ps1') -Root $script:RepoRoot
    $inventory = ConvertFrom-StrictJsonBytes -Bytes (Get-FileBytes $InventoryDraft) -Label 'inventory draft'
    $manifest = ConvertFrom-StrictJsonBytes -Bytes (Get-FileBytes $ManifestDraft) -Label 'traceability draft'
    $oldInventory = Invoke-GitBytes @('show', "HEAD:$script:InventoryRelativePath")
    $oldManifest = Invoke-GitBytes @('show', "HEAD:$script:ManifestRelativePath")
    if ($oldInventory.ExitCode -ne 0 -or $oldManifest.ExitCode -ne 0) {
        Throw-TraceError 'TRACE_GENERATE_PREDECESSOR' 'generation requires both committed predecessors'
    }
    $before = @{}
    foreach ($path in @($script:InventoryRelativePath, $script:ManifestRelativePath)) {
        $before[$path] = Get-Sha256Bytes (Get-FileBytes (Resolve-TracePath $path))
    }
    # The author supplies reviewed successor IDs, rules, links and horizons.
    # Only derived hashes/revision bindings are generated; unchanged immutable
    # identities cannot be renewed in place because the ordinary checker runs.
    foreach ($pair in @(@($inventory, $oldInventory.Bytes), @($manifest, $oldManifest.Bytes))) {
        $previous = ConvertFrom-StrictJsonBytes -Bytes $pair[1] -Label 'committed predecessor'
        $pair[0].revision.generation = [int]$previous.revision.generation + 1
        $pair[0].revision.predecessor = [pscustomobject][ordered]@{
            schema = $previous.schema; generation = $previous.revision.generation
            sha256 = Get-Sha256Bytes $pair[1]
        }
    }
    $discovered = @(Get-FirstPartyTestInventory)
    $discovery = @{}
    foreach ($test in $discovered) { $discovery[$test.identity] = $test }
    foreach ($test in @($inventory.tests | Where-Object status -eq 'active')) {
        if (-not $discovery.ContainsKey([string]$test.identity)) { Throw-TraceError 'TRACE_GENERATE_TEST' "undiscovered active test: $($test.identity)" }
        $test.selector_sha256 = $discovery[[string]$test.identity].selector_sha256
    }
    foreach ($asset in @($inventory.assets | Where-Object status -eq 'active')) {
        $asset.sha256 = Get-Sha256Bytes (Get-FileBytes (Resolve-TracePath $asset.path))
    }
    foreach ($entry in @($manifest.sources) + @($manifest.gates)) {
        if ($entry.status -eq 'active') { $entry.sha256 = Get-Sha256Bytes (Get-FileBytes (Resolve-TracePath $entry.path)) }
    }
    $inventoryBytes = ConvertTo-CanonicalJsonBytes $inventory
    $context = Assert-InventoryDocument -Inventory $inventory -Discovered $discovered -PredecessorBytes $oldInventory.Bytes
    $manifest.inventory.sha256 = Get-Sha256Bytes $inventoryBytes
    $sources = @{}; $rules = @{}; $gates = @{}; $links = @{}; $tests = @{}; $assets = @{}
    foreach ($x in $manifest.sources) { $sources[$x.source_id] = $x }
    foreach ($x in $manifest.rules) { $rules[$x.rule_id] = $x }
    foreach ($x in $manifest.gates) { $gates[$x.gate_id] = $x }
    foreach ($x in $manifest.source_rule_links) { $links[$x.link_id] = $x }
    foreach ($x in $inventory.tests) { $tests[$x.test_id] = $x }
    foreach ($x in $inventory.assets) { $assets[$x.asset_id] = $x }
    foreach ($binding in @($manifest.semantic_bindings | Where-Object status -eq 'active')) {
        $binding.semantic_sha256 = Get-SemanticBindingSha256 -Binding $binding -Rule $rules[$binding.rule_id] `
            -Gate $gates[$binding.gate_id] -SourceLinks @($binding.source_rule_link_ids | ForEach-Object { $links[$_] }) `
            -Tests @($binding.test_ids | ForEach-Object { $tests[$_] }) -Sources $sources -Assets $assets
    }
    $null = Assert-TraceabilityDocument -Manifest $manifest -InventoryContext $context -AsOf ([DateTime]::UtcNow.Date) `
        -PredecessorBytes $oldManifest.Bytes -ExpectedInventorySha256 $manifest.inventory.sha256
    $manifestBytes = ConvertTo-CanonicalJsonBytes $manifest
    $parent = Get-Item -LiteralPath (Split-Path -Parent $Destination) -Force
    if (-not $parent.PSIsContainer -or ($parent.Attributes -band [IO.FileAttributes]::ReparsePoint)) { Throw-TraceError 'TRACE_GENERATE_DESTINATION' 'ordinary output parent required' }
    $null = New-Item -ItemType Directory -Path $Destination -ErrorAction Stop
    $files = @(
        @{ path = $script:InventoryRelativePath; bytes = $inventoryBytes },
        @{ path = $script:ManifestRelativePath; bytes = $manifestBytes }
    )
    foreach ($file in $files) {
        if ((Get-Sha256Bytes (Get-FileBytes (Resolve-TracePath $file.path))) -cne $before[$file.path]) { Throw-TraceError 'TRACE_GENERATE_DRIFT' 'live manifest changed during generation' }
        $target = Join-Path $Destination ([IO.Path]::GetFileName($file.path))
        $stream = [IO.File]::Open($target, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write, [IO.FileShare]::None)
        try { $stream.Write($file.bytes); $stream.Flush($true) } finally { $stream.Dispose() }
        if ((Get-Sha256Bytes (Get-FileBytes $target)) -cne (Get-Sha256Bytes $file.bytes)) { Throw-TraceError 'TRACE_GENERATE_READBACK' 'candidate bytes differ after writing' }
    }
    [ordered]@{
        schema = 'rayman.traceability.candidate.v1'; status = 'validated'; source_modified = $false
        output_directory = $Destination; predecessor_worktree_sha256 = $before
        inventory_sha256 = Get-Sha256Bytes $inventoryBytes; manifest_sha256 = Get-Sha256Bytes $manifestBytes
        active_tests = @($inventory.tests | Where-Object status -eq 'active').Count
        active_bindings = @($manifest.semantic_bindings | Where-Object status -eq 'active').Count
        retired_bindings = @($manifest.semantic_bindings | Where-Object status -eq 'retired').Count
    }
}

function Get-PredecessorBytes {
    param(
        [Parameter(Mandatory = $true)][string]$RelativePath,
        [Parameter(Mandatory = $true)][byte[]]$CurrentBytes
    )
    $head = Invoke-GitBytes -Arguments @('show', "HEAD:$RelativePath")
    if ($head.ExitCode -eq 0) {
        if ((Get-Sha256Bytes -Bytes $head.Bytes) -cne (Get-Sha256Bytes -Bytes $CurrentBytes)) { return $head.Bytes }
        $log = Invoke-GitBytes -Arguments @('log', '--format=%H', '--follow', '--', $RelativePath)
        if ($log.ExitCode -ne 0) { Throw-TraceError 'TRACE_HISTORY_GIT' "cannot read history for ${RelativePath}: $($log.Stderr)" }
        $commits = @($script:Utf8.GetString($log.Bytes) -split "`r?`n" | Where-Object { $_ -match '^[0-9a-f]{40}$' })
        if ($commits.Count -lt 2) { return $null }
        $previous = Invoke-GitBytes -Arguments @('show', "$($commits[1]):$RelativePath")
        if ($previous.ExitCode -ne 0) { Throw-TraceError 'TRACE_HISTORY_GIT' "cannot read predecessor for ${RelativePath}: $($previous.Stderr)" }
        return $previous.Bytes
    }

    $history = Invoke-GitBytes -Arguments @('log', '--all', '--format=%H', '--', $RelativePath)
    if ($history.ExitCode -ne 0) { Throw-TraceError 'TRACE_HISTORY_GIT' "cannot determine prior history for ${RelativePath}: $($history.Stderr)" }
    $priorCommits = @($script:Utf8.GetString($history.Bytes) -split "`r?`n" | Where-Object { $_ -match '^[0-9a-f]{40}$' })
    if ($priorCommits.Count -ne 0) {
        Throw-TraceError 'TRACE_PREDECESSOR_MISSING' "$RelativePath existed in reachable history but is absent from HEAD; bootstrap is forbidden"
    }
    $shallow = Invoke-GitBytes -Arguments @('rev-parse', '--is-shallow-repository')
    if ($shallow.ExitCode -ne 0) { Throw-TraceError 'TRACE_HISTORY_GIT' "cannot inspect repository depth: $($shallow.Stderr)" }
    if ($script:Utf8.GetString($shallow.Bytes).Trim() -ceq 'true') {
        Throw-TraceError 'TRACE_PREDECESSOR_MISSING' "$RelativePath is absent from shallow history; bootstrap authority is unavailable"
    }
    return $null
}

function Get-ComparableJson {
    param(
        [Parameter(Mandatory = $true)]$Node,
        [switch]$IgnoreLifecycle
    )
    $copy = [ordered]@{}
    foreach ($property in $Node.PSObject.Properties) {
        if ($IgnoreLifecycle -and $property.Name -in @('status', 'retirement')) { continue }
        $copy[$property.Name] = $property.Value
    }
    return $copy | ConvertTo-Json -Depth 30 -Compress
}

function Assert-CollectionHistory {
    param(
        [Parameter(Mandatory = $true)][AllowEmptyCollection()][array]$Previous,
        [Parameter(Mandatory = $true)][AllowEmptyCollection()][array]$Current,
        [Parameter(Mandatory = $true)][string]$IdProperty,
        [Parameter(Mandatory = $true)][string]$Label
    )
    $previousMap = @{}
    $currentMap = @{}
    foreach ($node in $Previous) { $previousMap[[string]$node.$IdProperty] = $node }
    foreach ($node in $Current) { $currentMap[[string]$node.$IdProperty] = $node }
    foreach ($old in $Previous) {
        $id = [string]$old.$IdProperty
        if (-not $currentMap.ContainsKey($id)) {
            Throw-TraceError 'TRACE_NODE_REMOVED_WITHOUT_TOMBSTONE' "$Label node disappeared without a tombstone: $id"
        }
        $new = $currentMap[$id]
        if ([string]$old.status -eq 'retired') {
            if ([string]$new.status -ne 'retired') { Throw-TraceError 'TRACE_TOMBSTONE_RESURRECTED' "retired ID was resurrected: $id" }
            if ((Get-ComparableJson -Node $old) -cne (Get-ComparableJson -Node $new)) {
                Throw-TraceError 'TRACE_TOMBSTONE_MUTATED' "retired tombstone changed: $id"
            }
        } elseif ([string]$new.status -eq 'retired') {
            if ((Get-ComparableJson -Node $old -IgnoreLifecycle) -cne (Get-ComparableJson -Node $new -IgnoreLifecycle)) {
                Throw-TraceError 'TRACE_TOMBSTONE_MUTATED' "retirement changed the identity of $id"
            }
        } elseif ((Get-ComparableJson -Node $old) -cne (Get-ComparableJson -Node $new)) {
            Throw-TraceError 'TRACE_ACTIVE_ID_MUTATED' "active semantic identity changed without retirement: $id"
        }
    }
    foreach ($new in @($Current | Where-Object status -eq 'retired')) {
        $id = [string]$new.$IdProperty
        if (-not $previousMap.ContainsKey($id)) {
            Throw-TraceError 'TRACE_NEW_RETIRED_TOMBSTONE' "retired node was never active in predecessor history: $id"
        }
    }
}

function Assert-Revision {
    param(
        [Parameter(Mandatory = $true)]$Current,
        [AllowNull()][byte[]]$PredecessorBytes,
        [Parameter(Mandatory = $true)][string]$Label,
        [Parameter(Mandatory = $true)][string]$ExpectedSchema
    )
    Assert-ExactProperties -Value $Current.revision -Expected @('generation', 'predecessor') -Label "$Label revision"
    $generation = [int64]$Current.revision.generation
    if ($generation -le 0) { Throw-TraceError 'TRACE_GENERATION' "$Label generation must be positive" }
    if ($null -eq $PredecessorBytes) {
        if ($generation -ne 1 -or $null -ne $Current.revision.predecessor) {
            Throw-TraceError 'TRACE_GENERATION' "$Label bootstrap must be generation 1 with no predecessor"
        }
        return $null
    }
    $previous = ConvertFrom-StrictJsonBytes -Bytes $PredecessorBytes -Label "$Label predecessor"
    if ([string]$previous.schema -cne $ExpectedSchema) { Throw-TraceError 'TRACE_SCHEMA_MIGRATION' "$Label predecessor schema is unsupported" }
    Assert-ExactProperties -Value $Current.revision.predecessor -Expected @('schema', 'generation', 'sha256') -Label "$Label predecessor binding"
    if ($generation -ne ([int64]$previous.revision.generation + 1) -or
        [string]$Current.revision.predecessor.schema -cne [string]$previous.schema -or
        [int64]$Current.revision.predecessor.generation -ne [int64]$previous.revision.generation -or
        [string]$Current.revision.predecessor.sha256 -cne (Get-Sha256Bytes -Bytes $PredecessorBytes)) {
        Throw-TraceError 'TRACE_PREDECESSOR_HASH' "$Label does not bind the exact predecessor schema, generation, and bytes"
    }
    return $previous
}

function Get-CfgTargets {
    param([Parameter(Mandatory = $true)][string]$Cfg)
    switch -CaseSensitive ($Cfg) {
        'all' { return @('windows', 'linux', 'other_unix') }
        'windows' { return @('windows') }
        'unix' { return @('linux', 'other_unix') }
        'target_os="linux"' { return @('linux') }
        'not(windows)' { return @('linux', 'other_unix') }
        'all(windows,debug_assertions)' { return @('windows') }
        'all(unix,not(target_os="linux"))' { return @('other_unix') }
        default { Throw-TraceError 'TRACE_GATE_PLATFORM_GAP' "unsupported Rust cfg expression in test inventory: $Cfg" }
    }
}

function Get-TestExecutionTargets {
    param([Parameter(Mandatory = $true)]$Test)
    if ([string]$Test.role -in @('eval_fixture', 'eval_oracle')) {
        return @('windows', 'eval_runtime')
    }
    return @(Get-CfgTargets -Cfg ([string]$Test.cfg))
}

function Assert-TestGateCompatibility {
    param(
        [Parameter(Mandatory = $true)]$Test,
        [Parameter(Mandatory = $true)]$Gate
    )
    $role = [string]$Test.role
    $compatible = $false
    if ($role -eq 'root_cargo') {
        $compatible = [string]$Gate.kind -ceq 'cargo_root_suite' -and
            [string]$Gate.path -ceq 'scripts/check-repo.ps1' -and
            [string]$Gate.selector_kind -ceq 'powershell_function' -and
            [string]$Gate.selector -ceq 'Get-RepositoryQualityCommands'
    } elseif ($role -eq 'eval_cargo') {
        $compatible = [string]$Gate.kind -ceq 'cargo_evals_suite' -and
            [string]$Gate.path -ceq 'scripts/check-repo.ps1' -and
            [string]$Gate.selector_kind -ceq 'powershell_function' -and
            [string]$Gate.selector -ceq 'Get-RepositoryQualityCommands'
    } elseif ($role -in @('eval_fixture', 'eval_oracle')) {
        $compatible = [string]$Gate.kind -ceq 'eval_oracle' -and
            [string]$Gate.path -ceq 'evals/src/task.rs' -and
            [string]$Gate.selector_kind -ceq 'rust_symbol' -and
            [string]$Gate.selector -ceq 'verify_grade_observation' -and
            (@($Gate.target_matrix) -join ',') -ceq 'windows,eval_runtime'
    } elseif ($role -in @('powershell_self_test', 'powershell_named_case')) {
        $expectedPath = if ([string]$Test.path -ceq 'scripts/check-test-traceability-v2.ps1') {
            'scripts/check-test-traceability.ps1'
        } else { [string]$Test.path }
        $compatible = [string]$Gate.kind -ceq 'powershell_self_test' -and
            [string]$Gate.path -ceq $expectedPath -and
            [string]$Gate.selector_kind -ceq 'powershell_self_test_suite' -and
            [string]$Gate.selector -ceq '-SelfTest'
    }
    if (-not $compatible) {
        Throw-TraceError 'TRACE_ENFORCEMENT_MISSING' "test $($Test.test_id) is not executed by compatible gate $($Gate.gate_id)"
    }
    $gateTargets = @($Gate.target_matrix)
    foreach ($target in @(Get-TestExecutionTargets -Test $Test)) {
        if ($target -notin $gateTargets) {
            Throw-TraceError 'TRACE_GATE_PLATFORM_GAP' "gate $($Gate.gate_id) does not cover $target for test $($Test.test_id)"
        }
    }
}

function Test-CfgOnCurrentPlatform {
    param([Parameter(Mandatory = $true)][string]$Cfg)
    $current = if ($IsWindows) { 'windows' } elseif ($IsLinux) { 'linux' } else { 'other_unix' }
    return $current -in @(Get-CfgTargets -Cfg $Cfg)
}

function Assert-InventoryDocument {
    param(
        [Parameter(Mandatory = $true)]$Inventory,
        [Parameter(Mandatory = $true)][array]$Discovered,
        [AllowNull()][byte[]]$PredecessorBytes
    )
    Assert-ExactProperties -Value $Inventory -Expected @('schema', 'scope', 'revision', 'tests', 'assets') -Label 'test inventory'
    if ([string]$Inventory.schema -cne 'rayman.first-party-test-inventory.v1') {
        Throw-TraceError 'TRACE_SCHEMA_VERSION' 'unsupported first-party test inventory schema'
    }
    Assert-ExactProperties -Value $Inventory.scope -Expected @('name', 'claim', 'rust_roots', 'powershell_root', 'eval_task_root', 'excluded_prefixes') -Label 'inventory scope'
    if ([string]$Inventory.scope.name -cne 'repository_first_party_executable_tests' -or
        [string]$Inventory.scope.powershell_root -cne 'scripts' -or
        [string]$Inventory.scope.eval_task_root -cne 'evals/tasks' -or
        (@($Inventory.scope.rust_roots) -join ',') -cne ($script:RustRoots -join ',') -or
        (@($Inventory.scope.excluded_prefixes) -join ',') -cne ($script:ExcludedPrefixes -join ',')) {
        Throw-TraceError 'TRACE_TEST_INVENTORY_SET' 'inventory scope was narrowed or drifted from the fixed first-party roots'
    }
    if ($Inventory.tests -isnot [array] -or $Inventory.assets -isnot [array]) {
        Throw-TraceError 'TRACE_SCHEMA_TYPE' 'inventory tests and assets must be arrays'
    }
    $tests = Get-NodeMap -Items @($Inventory.tests) -IdProperty 'test_id' -Prefix 'TEST' -Label 'test'
    $assets = Get-NodeMap -Items @($Inventory.assets) -IdProperty 'asset_id' -Prefix 'ASSET' -Label 'test asset'
    $activeIdentity = @{}
    foreach ($test in $tests.Values) {
        Assert-ExactProperties -Value $test -Expected @(
            'test_id', 'status', 'identity', 'role', 'path', 'selector_kind', 'selector',
            'selector_sha256', 'cfg', 'owner', 'retirement'
        ) -Label "test $($test.test_id)"
        Assert-Retirement -Retirement $test.retirement -Status ([string]$test.status) -Label "test $($test.test_id)"
        if ([string]$test.identity -cnotmatch '^(?:rust|powershell)::' -or
            [string]$test.selector_sha256 -cnotmatch '^[0-9a-f]{64}$') {
            Throw-TraceError 'TRACE_TEST_SHAPE' "test $($test.test_id) is incomplete"
        }
        if (Test-InventoryPathExcluded -RelativePath ([string]$test.path)) {
            Throw-TraceError 'TRACE_TEST_INVENTORY_SET' "test inventory contains an excluded generated path: $($test.path)"
        }
        $null = Resolve-TracePath -RelativePath ([string]$test.path) -AllowMissing
        if ([string]$test.status -eq 'active') {
            if ($activeIdentity.ContainsKey([string]$test.identity)) {
                Throw-TraceError 'TRACE_TEST_INVENTORY_SET' "multiple active tests claim identity $($test.identity)"
            }
            $activeIdentity[[string]$test.identity] = $test
        }
    }
    foreach ($asset in $assets.Values) {
        Assert-ExactProperties -Value $asset -Expected @('asset_id', 'status', 'path', 'sha256', 'kind', 'cleanup_policy', 'retirement') -Label "asset $($asset.asset_id)"
        Assert-Retirement -Retirement $asset.retirement -Status ([string]$asset.status) -Label "asset $($asset.asset_id)"
        if ([string]$asset.kind -cne 'dedicated_file' -or [string]$asset.cleanup_policy -cne 'delete_on_last_reference') {
            Throw-TraceError 'TRACE_ASSET_OWNERSHIP' "asset $($asset.asset_id) has an unsupported ownership policy"
        }
        if (Test-InventoryPathExcluded -RelativePath ([string]$asset.path)) {
            Throw-TraceError 'TRACE_ASSET_OWNERSHIP' "test asset uses an excluded generated path: $($asset.path)"
        }
        if ([string]$asset.status -eq 'active') {
            $null = Assert-FileHash -Path ([string]$asset.path) -Sha256 ([string]$asset.sha256) -Label "asset $($asset.asset_id)"
        } else {
            $null = Resolve-TracePath -RelativePath ([string]$asset.path) -AllowMissing
        }
    }
    foreach ($test in @($tests.Values | Where-Object status -eq 'retired')) {
        $null = Get-ActiveReplacementNodes -Node $test -Map $tests -Label 'retired test'
    }
    foreach ($asset in @($assets.Values | Where-Object status -eq 'retired')) {
        $null = Get-ActiveReplacementNodes -Node $asset -Map $assets -Label 'retired asset'
    }

    $discoveredMap = @{}
    foreach ($row in $Discovered) { $discoveredMap[[string]$row.identity] = $row }
    foreach ($identity in $discoveredMap.Keys) {
        if (-not $activeIdentity.ContainsKey($identity)) {
            Throw-TraceError 'TRACE_TEST_INVENTORY_UNCLASSIFIED' "discovered first-party test is not registered: $identity"
        }
        $registered = $activeIdentity[$identity]
        $row = $discoveredMap[$identity]
        foreach ($field in @('role', 'path', 'selector_kind', 'selector', 'selector_sha256', 'cfg')) {
            if ([string]$registered.$field -cne [string]$row.$field) {
                Throw-TraceError 'TRACE_TEST_INVENTORY_SET' "registered test identity drifted at $field for $identity"
            }
        }
        if (($null -eq $registered.owner) -ne ($null -eq $row.owner) -or
            ($null -ne $registered.owner -and [string]$registered.owner -cne [string]$row.owner)) {
            Throw-TraceError 'TRACE_TEST_INVENTORY_SET' "registered test owner drifted for $identity"
        }
    }
    foreach ($identity in $activeIdentity.Keys) {
        if (-not $discoveredMap.ContainsKey($identity)) {
            Throw-TraceError 'TRACE_TEST_MISSING' "active registered test is no longer executable: $identity"
        }
    }

    foreach ($test in @($tests.Values | Where-Object status -eq 'retired')) {
        if (-not $discoveredMap.ContainsKey([string]$test.identity)) { continue }
        $replacementAllowed = $false
        foreach ($replacement in @(Get-ActiveReplacementNodes -Node $test -Map $tests -Label 'retired executable test')) {
            if ([string]$replacement.identity -ceq [string]$test.identity -and
                [string]$replacement.selector_sha256 -cne [string]$test.selector_sha256) {
                $replacementAllowed = $true
            }
        }
        if (-not $replacementAllowed) {
            Throw-TraceError 'TRACE_RETIRED_SELECTOR_PRESENT' "retired test identity is still executable without a reviewed replacement: $($test.identity)"
        }
    }

    $activeTestsByPath = @{}
    foreach ($test in @($tests.Values | Where-Object status -eq 'active')) {
        $path = [string]$test.path
        $activeTestsByPath[$path] = 1 + [int]($activeTestsByPath[$path])
    }
    $activeAssetPaths = @{}
    foreach ($asset in @($assets.Values | Where-Object status -eq 'active')) {
        $path = [string]$asset.path
        if ($activeAssetPaths.ContainsKey($path)) {
            Throw-TraceError 'TRACE_ASSET_OWNERSHIP' "multiple active dedicated assets claim one path: $path"
        }
        $activeAssetPaths[$path] = [string]$asset.asset_id
        if (-not $activeTestsByPath.ContainsKey($path)) {
            Throw-TraceError 'TRACE_ASSET_OWNERSHIP' "active dedicated asset has no active test owner: $($asset.asset_id)"
        }
    }
    foreach ($asset in @($assets.Values | Where-Object status -eq 'retired')) {
        $path = Resolve-TracePath -RelativePath ([string]$asset.path) -AllowMissing
        if (-not (Test-Path -LiteralPath $path)) { continue }
        $replacementAllowed = $false
        foreach ($replacement in @(Get-ActiveReplacementNodes -Node $asset -Map $assets -Label 'retired dedicated asset')) {
            if ([string]$replacement.path -ceq [string]$asset.path -and
                [string]$replacement.sha256 -cne [string]$asset.sha256) {
                $replacementAllowed = $true
            }
        }
        if (-not $replacementAllowed) {
            Throw-TraceError 'TRACE_RETIRED_ASSET_PRESENT' "retired dedicated asset still exists without a reviewed replacement: $($asset.path)"
        }
    }

    $previous = Assert-Revision -Current $Inventory -PredecessorBytes $PredecessorBytes -Label 'test inventory' -ExpectedSchema 'rayman.first-party-test-inventory.v1'
    if ($null -eq $previous) {
        if (@($Inventory.tests | Where-Object status -eq 'retired').Count -ne 0 -or
            @($Inventory.assets | Where-Object status -eq 'retired').Count -ne 0) {
            Throw-TraceError 'TRACE_NEW_RETIRED_TOMBSTONE' 'generation 1 cannot fabricate inventory tombstones'
        }
    } else {
        Assert-CollectionHistory -Previous @($previous.tests) -Current @($Inventory.tests) -IdProperty 'test_id' -Label 'test'
        Assert-CollectionHistory -Previous @($previous.assets) -Current @($Inventory.assets) -IdProperty 'asset_id' -Label 'asset'
    }
    return [pscustomobject]@{
        tests = $tests
        assets = $assets
        active_tests = @($tests.Values | Where-Object status -eq 'active')
        discovered = $Discovered
    }
}

function Get-SemanticBindingSha256 {
    param(
        [Parameter(Mandatory = $true)]$Binding,
        [Parameter(Mandatory = $true)]$Rule,
        [Parameter(Mandatory = $true)]$Gate,
        [Parameter(Mandatory = $true)][array]$SourceLinks,
        [Parameter(Mandatory = $true)][array]$Tests,
        [Parameter(Mandatory = $true)]$Sources,
        [Parameter(Mandatory = $true)]$Assets
    )
    $sourceProjection = @()
    foreach ($link in @($SourceLinks | Sort-Object link_id)) {
        $source = $Sources[[string]$link.source_id]
        $sourceProjection += [ordered]@{
            link_id = [string]$link.link_id
            source_id = [string]$source.source_id
            source_sha256 = [string]$source.sha256
            relation = [string]$link.relation
            reviewed_at = [string]$link.review.reviewed_at
            valid_until = [string]$link.review.valid_until
        }
    }
    $testProjection = @()
    $assetIds = @{}
    foreach ($test in @($Tests | Sort-Object test_id)) {
        foreach ($asset in @($Assets.Values | Where-Object {
                    [string]$_.status -eq 'active' -and [string]$_.path -ceq [string]$test.path
                })) {
            $assetIds[[string]$asset.asset_id] = $true
        }
        $testProjection += [ordered]@{
            test_id = [string]$test.test_id
            identity = [string]$test.identity
            role = [string]$test.role
            selector_sha256 = [string]$test.selector_sha256
            cfg = [string]$test.cfg
        }
    }
    $assetProjection = @()
    foreach ($assetId in @($assetIds.Keys | Sort-Object)) {
        $asset = $Assets[$assetId]
        $assetProjection += [ordered]@{
            asset_id = $assetId
            path = [string]$asset.path
            sha256 = [string]$asset.sha256
        }
    }
    $projection = [ordered]@{
        schema = 'rayman.semantic-binding.projection.v1'
        binding_id = [string]$Binding.binding_id
        source_rule_links = $sourceProjection
        rule = [ordered]@{
            rule_id = [string]$Rule.rule_id
            statement = [string]$Rule.statement
            criticality = [string]$Rule.criticality
            required_modes = @($Rule.required_modes)
        }
        gate = [ordered]@{
            gate_id = [string]$Gate.gate_id
            kind = [string]$Gate.kind
            path = [string]$Gate.path
            sha256 = [string]$Gate.sha256
            selector_kind = [string]$Gate.selector_kind
            selector = [string]$Gate.selector
            target_matrix = @($Gate.target_matrix)
        }
        modes = @($Binding.modes)
        review = [ordered]@{
            reviewed_at = [string]$Binding.review.reviewed_at
            valid_until = [string]$Binding.review.valid_until
        }
        tests = $testProjection
        assets = $assetProjection
    }
    return Get-Sha256Text -Text ($projection | ConvertTo-Json -Depth 30 -Compress)
}

function Assert-CargoGateCommandContract {
    param([Parameter(Mandatory = $true)]$Gate)

    $checkRepoPath = Resolve-TracePath -RelativePath 'scripts/check-repo.ps1'
    $providerPath = Resolve-TracePath -RelativePath 'scripts/repository-quality.ps1'
    $checkRepo = [IO.File]::ReadAllText($checkRepoPath, $script:Utf8)
    $providerHash = (Get-FileHash -LiteralPath $providerPath -Algorithm SHA256).Hash.ToLowerInvariant()
    if (-not $checkRepo.Contains($providerHash) -or
        $checkRepo -match '(?m)--skip\b' -or
        $checkRepo -notmatch '(?s)Invoke-NativeChecked\s+-Application\s+\$cargo\s+-Arguments\s+\$qualityCommand\.argv') {
        Throw-TraceError 'TRACE_ENFORCEMENT_MISSING' 'Cargo gate does not byte-bind and directly execute the selector-free repository quality provider'
    }
    $suite = if ([string]$Gate.kind -ceq 'cargo_root_suite') { 'Root' } else { 'Evals' }
    $text = & $providerPath -Suite $suite | Out-String
    try {
        $document = $text | ConvertFrom-Json -Depth 8 -NoEnumerate -ErrorAction Stop
    } catch {
        Throw-TraceError 'TRACE_ENFORCEMENT_MISSING' "Cargo gate provider returned invalid JSON for $suite"
    }
    $test = @($document.commands | Where-Object name -CEQ 'test')
    $expected = if ($suite -eq 'Root') {
        @('test', '--locked', '--workspace', '--all-targets')
    } else {
        @('test', '--manifest-path', 'evals/Cargo.toml', '--locked', '--all-targets')
    }
    if ($test.Count -ne 1 -or (@($test[0].argv) -join "`0") -cne ($expected -join "`0")) {
        Throw-TraceError 'TRACE_ENFORCEMENT_MISSING' "Cargo gate provider narrows or changes the $suite test command"
    }
}

function Assert-GateSelector {
    param([Parameter(Mandatory = $true)]$Gate)
    $path = Resolve-TracePath -RelativePath ([string]$Gate.path)
    if ([string]$Gate.selector_kind -eq 'powershell_function') {
        if ((Get-PowerShellSelectorCount -Path $path -Kind 'powershell_function' -Selector ([string]$Gate.selector)) -ne 1) {
            Throw-TraceError 'TRACE_ENFORCEMENT_MISSING' "gate function is missing or ambiguous: $($Gate.gate_id)"
        }
        if ([string]$Gate.kind -in @('cargo_root_suite', 'cargo_evals_suite')) {
            Assert-CargoGateCommandContract -Gate $Gate
        }
    } elseif ([string]$Gate.selector_kind -eq 'powershell_self_test_suite') {
        if ((Get-PowerShellSelectorCount -Path $path -Kind 'powershell_self_test_suite' -Selector '-SelfTest') -ne 1) {
            Throw-TraceError 'TRACE_ENFORCEMENT_MISSING' "PowerShell self-test gate is missing: $($Gate.gate_id)"
        }
        $checkRepo = [IO.File]::ReadAllText((Resolve-TracePath -RelativePath 'scripts/check-repo.ps1'), $script:Utf8)
        $name = [IO.Path]::GetFileName([string]$Gate.path)
        $escaped = [regex]::Escape($name)
        $reachable = $checkRepo -match "(?m)Join-Path\s+\`$PSScriptRoot\s+'$escaped'\)\s+-SelfTest"
        if ($name -ceq 'audit-repository.ps1') {
            $reachable = $checkRepo -match "(?m)Join-Path\s+\`$PSScriptRoot\s+'$escaped'\)\s+-DependencyPolicyOnly"
        }
        if (-not $reachable) {
            Throw-TraceError 'TRACE_ENFORCEMENT_MISSING' "PowerShell self-test suite is not reachable from check-repo.ps1: $name"
        }
    } elseif ([string]$Gate.selector_kind -eq 'rust_symbol') {
        if ((Get-RustSymbolCount -Path $path -Selector ([string]$Gate.selector)) -ne 1) {
            Throw-TraceError 'TRACE_ENFORCEMENT_MISSING' "Rust gate symbol is missing or ambiguous: $($Gate.gate_id)"
        }
    } else {
        Throw-TraceError 'TRACE_GATE_TYPE' "unsupported gate selector kind: $($Gate.selector_kind)"
    }
}

function Assert-EvalCrosscheck {
    param(
        [Parameter(Mandatory = $true)]$InventoryContext,
        [Parameter(Mandatory = $true)]$Bindings,
        [Parameter(Mandatory = $true)]$SourceLinks
    )
    $tasksRoot = Resolve-TracePath -RelativePath 'evals/tasks' -AllowDirectory
    $taskNames = @((Get-ChildItem -LiteralPath $tasksRoot -Directory -Force | Sort-Object Name).Name)
    foreach ($test in $InventoryContext.active_tests) {
        if (-not [string]::IsNullOrWhiteSpace([string]$test.owner) -and [string]$test.owner -notin $taskNames) {
            Throw-TraceError 'TRACE_TASK_OWNER_UNKNOWN' "test $($test.test_id) names an unknown eval owner: $($test.owner)"
        }
    }
    foreach ($taskName in $taskNames) {
        $contractPath = Join-Path $tasksRoot "$taskName/task.json"
        $contract = ConvertFrom-StrictJsonBytes -Bytes (Get-FileBytes -Path $contractPath) -Label "eval task $taskName"
        $taskSources = @($contract.source_ids | Sort-Object)
        $taskRules = @($contract.rule_ids | Sort-Object)
        $owned = @($InventoryContext.active_tests | Where-Object { [string]$_.owner -ceq $taskName })
        $oracleTests = @($owned | Where-Object role -eq 'eval_oracle')
        $fixtureTests = @($owned | Where-Object role -eq 'eval_fixture')
        if ([string]$contract.oracle.kind -eq 'rust_module') {
            $expected = @($contract.oracle.expected_tests | Sort-Object)
            $actual = @($oracleTests | Where-Object selector_kind -eq 'rust_test_fn' | ForEach-Object selector | Sort-Object)
            if (($expected -join ',') -cne ($actual -join ',')) {
                Throw-TraceError 'TRACE_TASK_TEST_SET' "eval task $taskName Rust oracle selectors drifted from inventory"
            }
            if (@($fixtureTests | Where-Object selector_kind -ne 'rust_test_fn').Count -ne 0) {
                Throw-TraceError 'TRACE_ENFORCEMENT_MISSING' "eval task $taskName has a non-Rust fixture test outside its cargo grade gate"
            }
        } else {
            $expected = @([string]$contract.oracle.success_marker)
            $actual = @($oracleTests | Where-Object selector_kind -eq 'powershell_success_marker' | ForEach-Object selector | Sort-Object)
            if (($expected -join ',') -cne ($actual -join ',')) {
                Throw-TraceError 'TRACE_TASK_TEST_SET' "eval task $taskName PowerShell oracle selector drifted from inventory"
            }
            if ($fixtureTests.Count -ne 0) {
                Throw-TraceError 'TRACE_ENFORCEMENT_MISSING' "eval task $taskName contains fixture tests that its PowerShell grade never executes"
            }
        }
        $ownedTestIds = [Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
        foreach ($test in $owned) { $null = $ownedTestIds.Add([string]$test.test_id) }
        $ownedBindingIds = @($Bindings.Values | Where-Object {
                [string]$_.status -eq 'active' -and
                @($_.test_ids | Where-Object { $ownedTestIds.Contains([string]$_) }).Count -ne 0
            } | ForEach-Object binding_id | Sort-Object -Unique)
        $coveredRules = @($ownedBindingIds | ForEach-Object { [string]$Bindings[[string]$_].rule_id } | Sort-Object -Unique)
        if (@($coveredRules | Where-Object { $_ -notin $taskRules }).Count -ne 0 -or
            @($taskRules | Where-Object { $_ -notin $coveredRules }).Count -ne 0) {
            Throw-TraceError 'TRACE_TASK_RULE_SET' "eval task $taskName semantic rules drifted from inventory bindings"
        }
        $coveredSources = @()
        foreach ($bindingId in $ownedBindingIds) {
            foreach ($linkId in @($Bindings[[string]$bindingId].source_rule_link_ids)) {
                $coveredSources += [string]$SourceLinks[[string]$linkId].source_id
            }
        }
        $coveredSources = @($coveredSources | Sort-Object -Unique)
        if (@($coveredSources | Where-Object { $_ -notin $taskSources }).Count -ne 0 -or
            @($taskSources | Where-Object { $_ -notin $coveredSources }).Count -ne 0) {
            Throw-TraceError 'TRACE_TASK_SOURCE_SET' "eval task $taskName semantic sources drifted from inventory bindings"
        }
    }
}

function Assert-TraceabilityDocument {
    param(
        [Parameter(Mandatory = $true)]$Manifest,
        [Parameter(Mandatory = $true)]$InventoryContext,
        [Parameter(Mandatory = $true)][DateTime]$AsOf,
        [AllowNull()][byte[]]$PredecessorBytes,
        [string]$ExpectedInventorySha256,
        [switch]$SkipEvalCrosscheck
    )
    Assert-ExactProperties -Value $Manifest -Expected @(
        'schema', 'scope', 'revision', 'inventory', 'sources', 'rules', 'gates',
        'source_rule_links', 'semantic_bindings'
    ) -Label 'traceability manifest'
    if ([string]$Manifest.schema -cne 'rayman.test-traceability.v2') {
        Throw-TraceError 'TRACE_SCHEMA_VERSION' 'unsupported traceability schema'
    }
    Assert-ExactProperties -Value $Manifest.scope -Expected @('name', 'claim') -Label 'traceability scope'
    if ([string]$Manifest.scope.name -cne 'repository_first_party_executable_tests') {
        Throw-TraceError 'TRACE_SCHEMA_VERSION' 'traceability scope must cover the complete first-party executable inventory'
    }
    Assert-ExactProperties -Value $Manifest.inventory -Expected @('path', 'schema', 'sha256') -Label 'traceability inventory binding'
    if ([string]$Manifest.inventory.path -cne $script:InventoryRelativePath -or
        [string]$Manifest.inventory.schema -cne 'rayman.first-party-test-inventory.v1' -or
        [string]$Manifest.inventory.sha256 -cne $ExpectedInventorySha256) {
        Throw-TraceError 'TRACE_TEST_INVENTORY_SET' 'traceability manifest is not bound to the exact current inventory bytes'
    }
    foreach ($name in @('sources', 'rules', 'gates', 'source_rule_links', 'semantic_bindings')) {
        if ($Manifest.$name -isnot [array]) { Throw-TraceError 'TRACE_SCHEMA_TYPE' "$name must be an array" }
    }
    $sources = Get-NodeMap -Items @($Manifest.sources) -IdProperty 'source_id' -Prefix 'SRC' -Label 'source'
    $rules = Get-NodeMap -Items @($Manifest.rules) -IdProperty 'rule_id' -Prefix 'RULE' -Label 'rule'
    $gates = Get-NodeMap -Items @($Manifest.gates) -IdProperty 'gate_id' -Prefix 'GATE' -Label 'gate'
    $sourceLinks = Get-NodeMap -Items @($Manifest.source_rule_links) -IdProperty 'link_id' -Prefix 'LINK' -Label 'source-rule link'
    $bindings = Get-NodeMap -Items @($Manifest.semantic_bindings) -IdProperty 'binding_id' -Prefix 'BIND' -Label 'semantic binding'

    foreach ($source in $sources.Values) {
        Assert-ExactProperties -Value $source -Expected @('source_id', 'status', 'kind', 'title', 'path', 'sha256', 'valid_until', 'retirement') -Label "source $($source.source_id)"
        Assert-Retirement -Retirement $source.retirement -Status ([string]$source.status) -Label "source $($source.source_id)"
        if ([string]$source.kind -notin @('internal_contract', 'internal_policy', 'synthetic_case', 'incident_snapshot', 'external_standard')) {
            Throw-TraceError 'TRACE_SOURCE_KIND' "source $($source.source_id) has an invalid kind"
        }
        $validUntil = ConvertFrom-TraceDate -Value ([string]$source.valid_until) -Label "source $($source.source_id) valid_until"
        if ([string]$source.status -eq 'active') {
            $null = Assert-FileHash -Path ([string]$source.path) -Sha256 ([string]$source.sha256) -Label "source $($source.source_id)"
            if ($validUntil -lt $AsOf.Date) { Throw-TraceError 'TRACE_SOURCE_EXPIRED' "source $($source.source_id) expired" }
        } else {
            $null = Resolve-TracePath -RelativePath ([string]$source.path) -AllowMissing
        }
    }
    $allowedModes = @('behavioral_positive', 'behavioral_negative', 'integration', 'mutation', 'structural')
    foreach ($rule in $rules.Values) {
        Assert-ExactProperties -Value $rule -Expected @('rule_id', 'status', 'statement', 'criticality', 'required_modes', 'retirement') -Label "rule $($rule.rule_id)"
        Assert-Retirement -Retirement $rule.retirement -Status ([string]$rule.status) -Label "rule $($rule.rule_id)"
        if ([string]::IsNullOrWhiteSpace([string]$rule.statement) -or
            [string]$rule.criticality -notin @('release_blocking', 'important') -or
            $rule.required_modes -isnot [array] -or
            @($rule.required_modes | Where-Object { $_ -notin $allowedModes }).Count -ne 0) {
            Throw-TraceError 'TRACE_RULE_SHAPE' "rule $($rule.rule_id) is incomplete"
        }
    }
    foreach ($gate in $gates.Values) {
        Assert-ExactProperties -Value $gate -Expected @(
            'gate_id', 'status', 'kind', 'title', 'path', 'sha256', 'selector_kind',
            'selector', 'target_matrix', 'retirement'
        ) -Label "gate $($gate.gate_id)"
        Assert-Retirement -Retirement $gate.retirement -Status ([string]$gate.status) -Label "gate $($gate.gate_id)"
        if ([string]$gate.kind -notin @('cargo_root_suite', 'cargo_evals_suite', 'powershell_self_test', 'eval_oracle') -or
            $gate.target_matrix -isnot [array] -or @($gate.target_matrix).Count -eq 0 -or
            @($gate.target_matrix | Where-Object { $_ -notin @('windows', 'linux', 'other_unix', 'eval_runtime') }).Count -ne 0) {
            Throw-TraceError 'TRACE_GATE_TYPE' "gate $($gate.gate_id) is incomplete"
        }
        if ([string]$gate.status -eq 'active') {
            $null = Assert-FileHash -Path ([string]$gate.path) -Sha256 ([string]$gate.sha256) -Label "gate $($gate.gate_id)"
            Assert-GateSelector -Gate $gate
        }
    }
    foreach ($link in $sourceLinks.Values) {
        Assert-ExactProperties -Value $link -Expected @('link_id', 'status', 'source_id', 'rule_id', 'relation', 'review', 'retirement') -Label "source-rule link $($link.link_id)"
        Assert-Retirement -Retirement $link.retirement -Status ([string]$link.status) -Label "source-rule link $($link.link_id)"
        Assert-Review -Review $link.review -Label "source-rule link $($link.link_id)" -AsOf $AsOf -Active:([string]$link.status -eq 'active')
        if (-not $sources.ContainsKey([string]$link.source_id) -or -not $rules.ContainsKey([string]$link.rule_id)) {
            Throw-TraceError 'TRACE_LINK_UNKNOWN_ID' "source-rule link $($link.link_id) points to an unknown node"
        }
        if ([string]$link.relation -notin @('defines', 'derives', 'constrains')) {
            Throw-TraceError 'TRACE_LINK_RELATION' "source-rule link $($link.link_id) has an invalid relation"
        }
        if ([string]$link.status -eq 'active' -and
            ([string]$sources[[string]$link.source_id].status -ne 'active' -or [string]$rules[[string]$link.rule_id].status -ne 'active')) {
            Throw-TraceError 'TRACE_LINK_STATUS' "active source-rule link $($link.link_id) references a retired node"
        }
    }
    foreach ($binding in $bindings.Values) {
        Assert-ExactProperties -Value $binding -Expected @(
            'binding_id', 'status', 'source_rule_link_ids', 'test_ids', 'rule_id', 'gate_id',
            'modes', 'review', 'semantic_sha256', 'retirement'
        ) -Label "semantic binding $($binding.binding_id)"
        Assert-Retirement -Retirement $binding.retirement -Status ([string]$binding.status) -Label "semantic binding $($binding.binding_id)"
        Assert-Review -Review $binding.review -Label "semantic binding $($binding.binding_id)" -AsOf $AsOf -Active:([string]$binding.status -eq 'active')
        if ($binding.source_rule_link_ids -isnot [array] -or @($binding.source_rule_link_ids).Count -eq 0 -or
            $binding.test_ids -isnot [array] -or @($binding.test_ids).Count -ne 1 -or
            $binding.modes -isnot [array] -or @($binding.modes).Count -eq 0 -or
            @($binding.modes | Where-Object { $_ -notin $allowedModes }).Count -ne 0 -or
            [string]$binding.semantic_sha256 -cnotmatch '^[0-9a-f]{64}$') {
            Throw-TraceError 'TRACE_BINDING_SHAPE' "semantic binding $($binding.binding_id) is incomplete"
        }
        if (-not $rules.ContainsKey([string]$binding.rule_id) -or -not $gates.ContainsKey([string]$binding.gate_id)) {
            Throw-TraceError 'TRACE_GATE_UNKNOWN' "semantic binding $($binding.binding_id) references an unknown rule or gate"
        }
        $resolvedLinks = @()
        foreach ($linkId in @($binding.source_rule_link_ids)) {
            if (-not $sourceLinks.ContainsKey([string]$linkId)) { Throw-TraceError 'TRACE_LINK_UNKNOWN_ID' "binding references unknown source link $linkId" }
            $link = $sourceLinks[[string]$linkId]
            if ([string]$link.rule_id -cne [string]$binding.rule_id) { Throw-TraceError 'TRACE_LINK_UNKNOWN_ID' "binding/source link rule mismatch: $($binding.binding_id)" }
            $resolvedLinks += $link
        }
        $boundTests = @()
        $seenTestIds = [Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
        $previousTestId = $null
        foreach ($testId in @($binding.test_ids)) {
            if ($testId -isnot [string] -or -not $seenTestIds.Add([string]$testId) -or
                ($null -ne $previousTestId -and [StringComparer]::Ordinal.Compare($previousTestId, [string]$testId) -ge 0)) {
                Throw-TraceError 'TRACE_BINDING_SHAPE' "semantic binding has invalid, duplicate, or unsorted test IDs: $($binding.binding_id)"
            }
            if (-not $InventoryContext.tests.ContainsKey([string]$testId)) {
                Throw-TraceError 'TRACE_LINK_UNKNOWN_ID' "binding references unknown test $testId"
            }
            $boundTests += $InventoryContext.tests[[string]$testId]
            $previousTestId = [string]$testId
        }
        if ([string]$binding.status -eq 'active' -and
            ([string]$rules[[string]$binding.rule_id].status -ne 'active' -or
             [string]$gates[[string]$binding.gate_id].status -ne 'active' -or
             @($resolvedLinks | Where-Object status -ne 'active').Count -ne 0 -or
             @($boundTests | Where-Object status -ne 'active').Count -ne 0)) {
            Throw-TraceError 'TRACE_LINK_STATUS' "active semantic binding $($binding.binding_id) references a retired node"
        }
        if ([string]$binding.status -eq 'active') {
            foreach ($test in $boundTests) {
                Assert-TestGateCompatibility -Test $test -Gate $gates[[string]$binding.gate_id]
            }
            $actualSemantic = Get-SemanticBindingSha256 -Binding $binding -Rule $rules[[string]$binding.rule_id] `
                -Gate $gates[[string]$binding.gate_id] -SourceLinks $resolvedLinks -Tests $boundTests `
                -Sources $sources -Assets $InventoryContext.assets
            if ([string]$binding.semantic_sha256 -cne $actualSemantic) {
                Throw-TraceError 'TRACE_SEMANTIC_BINDING_HASH' "semantic binding byte/review identity drifted: $($binding.binding_id)"
            }
        }
    }

    $activeLinks = @($sourceLinks.Values | Where-Object status -eq 'active')
    $activeBindings = @($bindings.Values | Where-Object status -eq 'active')
    foreach ($source in @($sources.Values | Where-Object status -eq 'active')) {
        if (@($activeLinks | Where-Object source_id -eq $source.source_id).Count -eq 0) { Throw-TraceError 'TRACE_SOURCE_ORPHAN' "active source has no rule: $($source.source_id)" }
    }
    foreach ($rule in @($rules.Values | Where-Object status -eq 'active')) {
        if (@($activeLinks | Where-Object { $_.rule_id -eq $rule.rule_id -and $_.relation -eq 'defines' }).Count -eq 0) {
            Throw-TraceError 'TRACE_RULE_NO_SOURCE' "active rule has no defining source: $($rule.rule_id)"
        }
        $ruleBindings = @($activeBindings | Where-Object rule_id -eq $rule.rule_id)
        if ($ruleBindings.Count -eq 0) { Throw-TraceError 'TRACE_RULE_NO_GATE' "active rule has no active gated test: $($rule.rule_id)" }
        $modes = @($ruleBindings.modes | ForEach-Object { $_ } | Sort-Object -Unique)
        foreach ($required in @($rule.required_modes)) {
            if ($required -notin $modes) { Throw-TraceError 'TRACE_COVERAGE_MODE' "rule $($rule.rule_id) lacks required mode $required" }
        }
        if ([string]$rule.criticality -eq 'release_blocking' -and
            @($modes | Where-Object { $_ -in @('behavioral_positive', 'behavioral_negative', 'integration', 'mutation') }).Count -eq 0) {
            Throw-TraceError 'TRACE_COVERAGE_MODE' "release-blocking rule has structural-only coverage: $($rule.rule_id)"
        }
    }
    foreach ($gate in @($gates.Values | Where-Object status -eq 'active')) {
        if (@($activeBindings | Where-Object gate_id -eq $gate.gate_id).Count -eq 0) {
            Throw-TraceError 'TRACE_GATE_ORPHAN' "active gate has no tests: $($gate.gate_id)"
        }
    }
    foreach ($test in $InventoryContext.active_tests) {
        $allTestBindings = @($bindings.Values | Where-Object { [string]$test.test_id -in @($_.test_ids) })
        $activeTestBindings = @($allTestBindings | Where-Object status -eq 'active')
        if ($activeTestBindings.Count -eq 0) {
            $code = if ($allTestBindings.Count -ne 0) { 'TRACE_RETIREMENT_CASCADE' } else { 'TRACE_TEST_ORPHAN' }
            Throw-TraceError $code "active test has no active semantic association: $($test.test_id)"
        }
    }

    if (-not $SkipEvalCrosscheck) {
        Assert-EvalCrosscheck -InventoryContext $InventoryContext -Bindings $bindings -SourceLinks $sourceLinks
    }

    foreach ($descriptor in @(
            @($sources, 'source_id', 'source'), @($rules, 'rule_id', 'rule'),
            @($gates, 'gate_id', 'gate'), @($sourceLinks, 'link_id', 'source-rule link'),
            @($bindings, 'binding_id', 'semantic binding')
        )) {
        $map = $descriptor[0]
        $label = $descriptor[2]
        foreach ($node in @($map.Values | Where-Object status -eq 'retired')) {
            $null = Get-ActiveReplacementNodes -Node $node -Map $map -Label "retired $label"
        }
    }

    $previous = Assert-Revision -Current $Manifest -PredecessorBytes $PredecessorBytes -Label 'traceability manifest' -ExpectedSchema 'rayman.test-traceability.v2'
    if ($null -eq $previous) {
        foreach ($name in @('sources', 'rules', 'gates', 'source_rule_links', 'semantic_bindings')) {
            if (@($Manifest.$name | Where-Object status -eq 'retired').Count -ne 0) {
                Throw-TraceError 'TRACE_NEW_RETIRED_TOMBSTONE' 'generation 1 cannot fabricate semantic tombstones'
            }
        }
    } else {
        Assert-CollectionHistory -Previous @($previous.sources) -Current @($Manifest.sources) -IdProperty 'source_id' -Label 'source'
        Assert-CollectionHistory -Previous @($previous.rules) -Current @($Manifest.rules) -IdProperty 'rule_id' -Label 'rule'
        Assert-CollectionHistory -Previous @($previous.gates) -Current @($Manifest.gates) -IdProperty 'gate_id' -Label 'gate'
        Assert-CollectionHistory -Previous @($previous.source_rule_links) -Current @($Manifest.source_rule_links) -IdProperty 'link_id' -Label 'source-rule link'
        Assert-CollectionHistory -Previous @($previous.semantic_bindings) -Current @($Manifest.semantic_bindings) -IdProperty 'binding_id' -Label 'semantic binding'
    }
    return [pscustomobject]@{
        sources = @($sources.Values | Where-Object status -eq 'active').Count
        rules = @($rules.Values | Where-Object status -eq 'active').Count
        gates = @($gates.Values | Where-Object status -eq 'active').Count
        bindings = @($bindings.Values | Where-Object status -eq 'active').Count
        tests = @($InventoryContext.active_tests).Count
        assets = @($InventoryContext.assets.Values | Where-Object status -eq 'active').Count
    }
}

function Invoke-NativeCapture {
    param(
        [Parameter(Mandatory = $true)][string]$Application,
        [Parameter(Mandatory = $true)][string[]]$Arguments,
        [Parameter(Mandatory = $true)][string]$WorkingDirectory
    )
    $start = [Diagnostics.ProcessStartInfo]::new()
    $start.FileName = $Application
    $start.WorkingDirectory = $WorkingDirectory
    $start.UseShellExecute = $false
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    foreach ($argument in $Arguments) { $null = $start.ArgumentList.Add($argument) }
    $process = [Diagnostics.Process]::Start($start)
    $stdoutTask = $process.StandardOutput.ReadToEndAsync()
    $stderrTask = $process.StandardError.ReadToEndAsync()
    $process.WaitForExit()
    $result = [pscustomobject]@{
        ExitCode = $process.ExitCode
        Stdout = $stdoutTask.GetAwaiter().GetResult()
        Stderr = $stderrTask.GetAwaiter().GetResult()
    }
    $process.Dispose()
    return $result
}

function Get-CargoHarnesses {
    param(
        [Parameter(Mandatory = $true)][string]$Cargo,
        [Parameter(Mandatory = $true)][ValidateSet('Root', 'Evals')][string]$Suite
    )
    $arguments = if ($Suite -eq 'Root') {
        @('test', '--locked', '--workspace', '--all-targets', '--no-run', '--message-format=json')
    } else {
        @('test', '--locked', '--manifest-path', 'evals/Cargo.toml', '--all-targets', '--no-run', '--message-format=json')
    }
    $result = Invoke-NativeCapture -Application $Cargo -Arguments $arguments -WorkingDirectory $script:RepoRoot
    if ($result.ExitCode -ne 0) {
        Throw-TraceError 'TRACE_RUNTIME_INVENTORY' "cargo test --no-run failed for $Suite with exit $($result.ExitCode): $($result.Stderr)"
    }
    $harnesses = [Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
    foreach ($line in @($result.Stdout -split "`r?`n" | Where-Object { -not [string]::IsNullOrWhiteSpace($_) })) {
        try { $message = $line | ConvertFrom-Json -Depth 30 -NoEnumerate -ErrorAction Stop } catch { continue }
        if ([string]$message.reason -ceq 'compiler-artifact' -and
            $null -ne $message.profile -and $message.profile.test -eq $true -and
            $message.executable -is [string] -and -not [string]::IsNullOrWhiteSpace([string]$message.executable)) {
            $null = $harnesses.Add([IO.Path]::GetFullPath([string]$message.executable))
        }
    }
    if ($harnesses.Count -eq 0) { Throw-TraceError 'TRACE_RUNTIME_INVENTORY' "cargo emitted no test harnesses for $Suite" }
    return @($harnesses | Sort-Object)
}

function Get-HarnessTests {
    param([Parameter(Mandatory = $true)][string[]]$Harnesses)
    $tests = @()
    $ignored = @()
    foreach ($harness in $Harnesses) {
        if (-not (Test-Path -LiteralPath $harness -PathType Leaf)) {
            Throw-TraceError 'TRACE_RUNTIME_INVENTORY' "cargo test harness is missing: $harness"
        }
        $listed = Invoke-NativeCapture -Application $harness -Arguments @('--list', '--format', 'terse') -WorkingDirectory $script:RepoRoot
        if ($listed.ExitCode -ne 0) { Throw-TraceError 'TRACE_RUNTIME_INVENTORY' "test harness listing failed: $harness" }
        foreach ($line in @($listed.Stdout -split "`r?`n")) {
            $match = [regex]::Match($line, '^(?<name>.+): test$')
            if ($match.Success) { $tests += $match.Groups['name'].Value }
        }
        $ignoredList = Invoke-NativeCapture -Application $harness -Arguments @('--ignored', '--list', '--format', 'terse') -WorkingDirectory $script:RepoRoot
        if ($ignoredList.ExitCode -ne 0) { Throw-TraceError 'TRACE_RUNTIME_INVENTORY' "ignored-test listing failed: $harness" }
        foreach ($line in @($ignoredList.Stdout -split "`r?`n")) {
            $match = [regex]::Match($line, '^(?<name>.+): test$')
            if ($match.Success) { $ignored += $match.Groups['name'].Value }
        }
    }
    if ($ignored.Count -ne 0) { Throw-TraceError 'TRACE_RUNTIME_IGNORED_TEST' "first-party cargo inventory contains ignored tests: $($ignored -join ', ')" }
    return @($tests)
}

function Assert-UniqueRuntimeLeafSelectors {
    param(
        [Parameter(Mandatory = $true)][array]$Selectors,
        [Parameter(Mandatory = $true)][string]$Suite
    )
    $duplicates = @($Selectors | Group-Object -CaseSensitive | Where-Object Count -gt 1 | ForEach-Object Name)
    if ($duplicates.Count -ne 0) {
        Throw-TraceError 'TRACE_RUNTIME_INVENTORY' "$Suite static test selectors are leaf-ambiguous; exact runtime identity cannot be proven: $($duplicates -join ', ')"
    }
}

function Assert-RuntimeCargoInventory {
    param([Parameter(Mandatory = $true)]$InventoryContext)
    $cargo = @(Get-Command cargo -All -ErrorAction SilentlyContinue | Where-Object CommandType -eq Application | Select-Object -First 1)
    if ($cargo.Count -ne 1) { Throw-TraceError 'TRACE_RUNTIME_INVENTORY' 'cargo must resolve directly to an application' }
    $counts = [ordered]@{}
    foreach ($descriptor in @(@('Root', 'root_cargo'), @('Evals', 'eval_cargo'))) {
        $suite = $descriptor[0]
        $role = $descriptor[1]
        $harnesses = @(Get-CargoHarnesses -Cargo $cargo[0].Source -Suite $suite)
        $observed = @(Get-HarnessTests -Harnesses $harnesses)
        $expected = @($InventoryContext.active_tests | Where-Object {
                [string]$_.role -ceq $role -and (Test-CfgOnCurrentPlatform -Cfg ([string]$_.cfg))
            } | ForEach-Object selector)
        Assert-UniqueRuntimeLeafSelectors -Selectors $expected -Suite $suite
        $observedCounts = @{}
        foreach ($name in $observed) {
            $leaf = @($name -split '::')[-1]
            $observedCounts[$leaf] = 1 + [int]$observedCounts[$leaf]
        }
        $expectedCounts = @{}
        foreach ($name in $expected) { $expectedCounts[$name] = 1 + [int]$expectedCounts[$name] }
        $allNames = @($observedCounts.Keys + $expectedCounts.Keys | Sort-Object -Unique)
        foreach ($name in $allNames) {
            if ([int]$observedCounts[$name] -ne [int]$expectedCounts[$name]) {
                Throw-TraceError 'TRACE_RUNTIME_INVENTORY' "$suite runtime/static test multiset differs at ${name}: observed=$([int]$observedCounts[$name]) expected=$([int]$expectedCounts[$name])"
            }
        }
        $counts[$suite.ToLowerInvariant()] = [ordered]@{
            harnesses = $harnesses.Count
            tests = $observed.Count
            ignored = 0
        }
    }
    return $counts
}

function Copy-JsonObject {
    param([Parameter(Mandatory = $true)]$Value)
    return $Value | ConvertTo-Json -Depth 100 | ConvertFrom-Json -Depth 100 -NoEnumerate -DateKind String
}

function Assert-SelfTestRejected {
    param(
        [Parameter(Mandatory = $true)][string]$Code,
        [Parameter(Mandatory = $true)][scriptblock]$Action
    )
    $observed = $null
    try { & $Action } catch { $observed = $_.Exception.Message }
    if ($null -eq $observed -or -not $observed.StartsWith("[$Code]", [StringComparison]::Ordinal)) {
        throw "traceability self-test did not reject $Code (observed: $observed)"
    }
}

function New-SelfTestFixture {
    $wrapperPath = 'scripts/check-test-traceability.ps1'
    $wrapper = Resolve-TracePath -RelativePath $wrapperPath
    $wrapperText = [IO.File]::ReadAllText($wrapper, $script:Utf8)
    $identity = "powershell::powershell_self_test::$wrapperPath::powershell_self_test_suite::-SelfTest"
    $testId = 'TEST-SELF-TRACEABILITY'
    $discovered = @([pscustomobject]@{
            identity = $identity
            role = 'powershell_self_test'
            path = $wrapperPath
            selector_kind = 'powershell_self_test_suite'
            selector = '-SelfTest'
            selector_sha256 = Get-Sha256Text -Text $wrapperText
            cfg = 'all'
            owner = $null
            suggested_test_id = $testId
        })
    $inventory = [ordered]@{
        schema = 'rayman.first-party-test-inventory.v1'
        scope = [ordered]@{
            name = 'repository_first_party_executable_tests'
            claim = 'self-test fixture'
            rust_roots = $script:RustRoots
            powershell_root = 'scripts'
            eval_task_root = 'evals/tasks'
            excluded_prefixes = $script:ExcludedPrefixes
        }
        revision = [ordered]@{ generation = 1; predecessor = $null }
        tests = @([ordered]@{
                test_id = $testId
                status = 'active'
                identity = $identity
                role = 'powershell_self_test'
                path = $wrapperPath
                selector_kind = 'powershell_self_test_suite'
                selector = '-SelfTest'
                selector_sha256 = $discovered[0].selector_sha256
                cfg = 'all'
                owner = $null
                retirement = $null
            })
        assets = @()
    } | ConvertTo-Json -Depth 30 | ConvertFrom-Json -Depth 30 -NoEnumerate -DateKind String
    $inventoryContext = Assert-InventoryDocument -Inventory $inventory -Discovered $discovered -PredecessorBytes $null
    $sourcePath = 'scripts/check-test-traceability-v2.ps1'
    $sourceSha = (Get-FileHash -LiteralPath (Resolve-TracePath -RelativePath $sourcePath) -Algorithm SHA256).Hash.ToLowerInvariant()
    $wrapperSha = (Get-FileHash -LiteralPath $wrapper -Algorithm SHA256).Hash.ToLowerInvariant()
    $manifest = [ordered]@{
        schema = 'rayman.test-traceability.v2'
        scope = [ordered]@{ name = 'repository_first_party_executable_tests'; claim = 'self-test fixture' }
        revision = [ordered]@{ generation = 1; predecessor = $null }
        inventory = [ordered]@{ path = $script:InventoryRelativePath; schema = 'rayman.first-party-test-inventory.v1'; sha256 = ('0' * 64) }
        sources = @([ordered]@{
                source_id = 'SRC-SELF-TRACEABILITY'; status = 'active'; kind = 'internal_policy'; title = 'self test'
                path = $sourcePath; sha256 = $sourceSha; valid_until = '2027-08-28'; retirement = $null
            })
        rules = @([ordered]@{
                rule_id = 'RULE-SELF-TRACEABILITY'; status = 'active'; statement = 'self-test rule'
                criticality = 'important'; required_modes = @('behavioral_negative'); retirement = $null
            })
        gates = @([ordered]@{
                gate_id = 'GATE-SELF-POWERSHELL'; status = 'active'; kind = 'powershell_self_test'; title = 'self test gate'
                path = $wrapperPath; sha256 = $wrapperSha; selector_kind = 'powershell_self_test_suite'; selector = '-SelfTest'
                target_matrix = @('windows', 'linux', 'other_unix'); retirement = $null
            })
        source_rule_links = @([ordered]@{
                link_id = 'LINK-SELF-SOURCE-RULE'; status = 'active'; source_id = 'SRC-SELF-TRACEABILITY'
                rule_id = 'RULE-SELF-TRACEABILITY'; relation = 'defines'
                review = [ordered]@{ reviewed_at = '2026-08-28'; valid_until = '2027-08-28' }; retirement = $null
            })
        semantic_bindings = @([ordered]@{
                binding_id = 'BIND-SELF-TRACEABILITY'; status = 'active'; source_rule_link_ids = @('LINK-SELF-SOURCE-RULE')
                test_ids = @('TEST-SELF-TRACEABILITY')
                rule_id = 'RULE-SELF-TRACEABILITY'; gate_id = 'GATE-SELF-POWERSHELL'; modes = @('behavioral_negative')
                review = [ordered]@{ reviewed_at = '2026-08-28'; valid_until = '2027-08-28' }
                semantic_sha256 = ('0' * 64); retirement = $null
            })
    } | ConvertTo-Json -Depth 30 | ConvertFrom-Json -Depth 30 -NoEnumerate -DateKind String
    $sources = Get-NodeMap -Items @($manifest.sources) -IdProperty 'source_id' -Prefix 'SRC' -Label 'source'
    $rules = Get-NodeMap -Items @($manifest.rules) -IdProperty 'rule_id' -Prefix 'RULE' -Label 'rule'
    $gates = Get-NodeMap -Items @($manifest.gates) -IdProperty 'gate_id' -Prefix 'GATE' -Label 'gate'
    $links = Get-NodeMap -Items @($manifest.source_rule_links) -IdProperty 'link_id' -Prefix 'LINK' -Label 'link'
    $binding = $manifest.semantic_bindings[0]
    $binding.semantic_sha256 = Get-SemanticBindingSha256 -Binding $binding -Rule $rules[$binding.rule_id] `
        -Gate $gates[$binding.gate_id] -SourceLinks @($links['LINK-SELF-SOURCE-RULE']) `
        -Tests @($inventoryContext.active_tests) -Sources $sources -Assets $inventoryContext.assets
    return [pscustomobject]@{ Inventory = $inventory; Context = $inventoryContext; Manifest = $manifest; Discovered = $discovered }
}

function Invoke-TraceabilitySelfTest {
    $fixture = New-SelfTestFixture
    $asOf = [DateTime]::new(2026, 8, 28)
    $inventoryBytes = $script:Utf8.GetBytes(($fixture.Inventory | ConvertTo-Json -Depth 100 -Compress))
    $fixture.Manifest.inventory.sha256 = Get-Sha256Bytes -Bytes $inventoryBytes
    $null = Assert-TraceabilityDocument -Manifest $fixture.Manifest -InventoryContext $fixture.Context -AsOf $asOf `
        -PredecessorBytes $null -ExpectedInventorySha256 $fixture.Manifest.inventory.sha256 -SkipEvalCrosscheck

    $rustSpoof = @'
// #[test]
// fn ghost_selector() {}
const SPOOF: &str = r#"#[test]
fn ghost_selector() {}"#;
/* #[test] fn ghost_selector() {} */
#[test]
fn real_selector() {}
'@
    $code = Remove-RustNonCode -Text $rustSpoof
    if ([regex]::Matches($code, '#\s*\[\s*test\s*\]').Count -ne 1) {
        throw 'traceability Rust lexer accepted a comment/raw-string selector or lost executable code'
    }
    $powerShellSpoof = @"
# Assert-Rejected -Label 'ghost'
`$text = @'
Assert-Rejected -Label 'ghost'
'@
Assert-Rejected -Label 'real'
"@
    $ast = Get-PowerShellAstFromText -Text $powerShellSpoof
    $named = @($ast.FindAll({ param($node) $node -is [Management.Automation.Language.CommandAst] -and [string]$node.GetCommandName() -ceq 'Assert-Rejected' }, $true))
    if ($named.Count -ne 1) { throw 'traceability PowerShell AST accepted comment/here-string text or lost executable structure' }

    $unreachableText = @'
param([switch]$SelfTest)
function Invoke-UnreachableCases {
    Assert-Rejected -Label 'unreachable'
}
if ($SelfTest) { Write-Output 'no cases dispatched' }
'@
    $unreachableAst = Get-PowerShellAstFromText -Text $unreachableText
    $unreachableCommand = @($unreachableAst.FindAll({
                param($node)
                $node -is [Management.Automation.Language.CommandAst] -and
                [string]$node.GetCommandName() -ceq 'Assert-Rejected'
            }, $true))[0]
    $unreachableFunctions = Get-SelfTestReachablePowerShellFunctions -Ast $unreachableAst
    Assert-SelfTestRejected -Code 'TRACE_POWERSHELL_CASE_UNREACHABLE' -Action {
        Assert-PowerShellNamedCaseReachable -Command $unreachableCommand -ReachableFunctions $unreachableFunctions -Path 'scripts/unreachable.ps1'
    }
    foreach ($excluded in @('target/generated.rs', 'crates/rayman/target/generated.rs', 'evals/.runs-123/task/src/lib.rs')) {
        if (-not (Test-InventoryPathExcluded -RelativePath $excluded)) {
            throw "traceability inventory did not exclude generated path: $excluded"
        }
    }
    Assert-SelfTestRejected -Code 'TRACE_PATH_REPARSE' -Action {
        Assert-OrdinaryInventoryEntry -Entry ([pscustomobject]@{ Attributes = [IO.FileAttributes]::ReparsePoint }) `
            -RelativePath 'crates/rayman/src/reparse.rs'
    }
    Assert-SelfTestRejected -Code 'TRACE_RUNTIME_INVENTORY' -Action {
        Assert-UniqueRuntimeLeafSelectors -Selectors @('same_leaf', 'same_leaf') -Suite 'self-test'
    }

    $unregistered = Copy-JsonObject $fixture.Inventory
    $extra = Copy-JsonObject $fixture.Discovered[0]
    $extra.identity = 'powershell::powershell_self_test::scripts/check-test-traceability.ps1::powershell_named_case::Assert-Rejected::ghost'
    Assert-SelfTestRejected -Code 'TRACE_TEST_INVENTORY_UNCLASSIFIED' -Action {
        $null = Assert-InventoryDocument -Inventory $unregistered -Discovered @($fixture.Discovered + $extra) -PredecessorBytes $null
    }
    $expired = Copy-JsonObject $fixture.Manifest
    $expired.semantic_bindings[0].review.reviewed_at = '2026-08-27'
    $expired.semantic_bindings[0].review.valid_until = '2026-08-27'
    Assert-SelfTestRejected -Code 'TRACE_SEMANTIC_LINK_EXPIRED' -Action {
        $null = Assert-TraceabilityDocument -Manifest $expired -InventoryContext $fixture.Context -AsOf $asOf -PredecessorBytes $null -ExpectedInventorySha256 $fixture.Manifest.inventory.sha256 -SkipEvalCrosscheck
    }
    $unknownGate = Copy-JsonObject $fixture.Manifest
    $unknownGate.semantic_bindings[0].gate_id = 'GATE-UNKNOWN'
    Assert-SelfTestRejected -Code 'TRACE_GATE_UNKNOWN' -Action {
        $null = Assert-TraceabilityDocument -Manifest $unknownGate -InventoryContext $fixture.Context -AsOf $asOf -PredecessorBytes $null -ExpectedInventorySha256 $fixture.Manifest.inventory.sha256 -SkipEvalCrosscheck
    }
    $multiTestBinding = Copy-JsonObject $fixture.Manifest
    $multiTestBinding.semantic_bindings[0].test_ids = @('TEST-SELF-TRACEABILITY', 'TEST-SELF-TRACEABILITY')
    Assert-SelfTestRejected -Code 'TRACE_BINDING_SHAPE' -Action {
        $null = Assert-TraceabilityDocument -Manifest $multiTestBinding -InventoryContext $fixture.Context -AsOf $asOf -PredecessorBytes $null -ExpectedInventorySha256 $fixture.Manifest.inventory.sha256 -SkipEvalCrosscheck
    }
    $platformGap = Copy-JsonObject $fixture.Manifest
    $platformGap.gates[0].target_matrix = @('windows', 'linux')
    Assert-SelfTestRejected -Code 'TRACE_GATE_PLATFORM_GAP' -Action {
        $pgSources = Get-NodeMap -Items @($platformGap.sources) -IdProperty 'source_id' -Prefix 'SRC' -Label 'source'
        $pgRules = Get-NodeMap -Items @($platformGap.rules) -IdProperty 'rule_id' -Prefix 'RULE' -Label 'rule'
        $pgGates = Get-NodeMap -Items @($platformGap.gates) -IdProperty 'gate_id' -Prefix 'GATE' -Label 'gate'
        $pgLinks = Get-NodeMap -Items @($platformGap.source_rule_links) -IdProperty 'link_id' -Prefix 'LINK' -Label 'link'
        $pgBinding = $platformGap.semantic_bindings[0]
        $pgBinding.semantic_sha256 = Get-SemanticBindingSha256 -Binding $pgBinding -Rule $pgRules[$pgBinding.rule_id] `
            -Gate $pgGates[$pgBinding.gate_id] -SourceLinks @($pgLinks['LINK-SELF-SOURCE-RULE']) `
            -Tests @($fixture.Context.active_tests) -Sources $pgSources -Assets $fixture.Context.assets
        $null = Assert-TraceabilityDocument -Manifest $platformGap -InventoryContext $fixture.Context -AsOf $asOf -PredecessorBytes $null -ExpectedInventorySha256 $fixture.Manifest.inventory.sha256 -SkipEvalCrosscheck
    }
    $retiredBinding = Copy-JsonObject $fixture.Manifest
    $retiredBinding.semantic_bindings[0].status = 'retired'
    $retiredBinding.semantic_bindings[0].retirement = [pscustomobject]@{ retired_at = '2026-08-28'; reason = 'self test'; replaced_by = @() }
    Assert-SelfTestRejected -Code 'TRACE_RULE_NO_GATE' -Action {
        $null = Assert-TraceabilityDocument -Manifest $retiredBinding -InventoryContext $fixture.Context -AsOf $asOf -PredecessorBytes $null -ExpectedInventorySha256 $fixture.Manifest.inventory.sha256 -SkipEvalCrosscheck
    }
    $retiredSource = Copy-JsonObject $fixture.Manifest
    $retiredSource.sources[0].status = 'retired'
    $retiredSource.sources[0].retirement = [pscustomobject]@{ retired_at = '2026-08-28'; reason = 'self test'; replaced_by = @() }
    Assert-SelfTestRejected -Code 'TRACE_LINK_STATUS' -Action {
        $null = Assert-TraceabilityDocument -Manifest $retiredSource -InventoryContext $fixture.Context -AsOf $asOf -PredecessorBytes $null -ExpectedInventorySha256 $fixture.Manifest.inventory.sha256 -SkipEvalCrosscheck
    }
    $assetOrphan = Copy-JsonObject $fixture.Inventory
    $assetOrphan.assets = @([pscustomobject]@{
            asset_id = 'ASSET-SELF-ORPHAN'; status = 'active'; path = 'scripts/check-test-traceability-v2.ps1'
            sha256 = (Get-FileHash -LiteralPath (Resolve-TracePath -RelativePath 'scripts/check-test-traceability-v2.ps1') -Algorithm SHA256).Hash.ToLowerInvariant()
            kind = 'dedicated_file'; cleanup_policy = 'delete_on_last_reference'; retirement = $null
        })
    Assert-SelfTestRejected -Code 'TRACE_ASSET_OWNERSHIP' -Action {
        $null = Assert-InventoryDocument -Inventory $assetOrphan -Discovered $fixture.Discovered -PredecessorBytes $null
    }

    $previous = Copy-JsonObject $fixture.Inventory
    $removed = Copy-JsonObject $fixture.Inventory
    $removed.tests = @()
    Assert-SelfTestRejected -Code 'TRACE_NODE_REMOVED_WITHOUT_TOMBSTONE' -Action {
        Assert-CollectionHistory -Previous @($previous.tests) -Current @($removed.tests) -IdProperty 'test_id' -Label 'test'
    }
    $retired = Copy-JsonObject $fixture.Inventory
    $retired.tests[0].status = 'retired'
    $retired.tests[0].retirement = [pscustomobject]@{ retired_at = '2026-08-28'; reason = 'history'; replaced_by = @() }
    $mutated = Copy-JsonObject $retired
    $mutated.tests[0].retirement.reason = 'rewritten'
    Assert-SelfTestRejected -Code 'TRACE_TOMBSTONE_MUTATED' -Action {
        Assert-CollectionHistory -Previous @($retired.tests) -Current @($mutated.tests) -IdProperty 'test_id' -Label 'test'
    }
    $resurrected = Copy-JsonObject $retired
    $resurrected.tests[0].status = 'active'
    $resurrected.tests[0].retirement = $null
    Assert-SelfTestRejected -Code 'TRACE_TOMBSTONE_RESURRECTED' -Action {
        Assert-CollectionHistory -Previous @($retired.tests) -Current @($resurrected.tests) -IdProperty 'test_id' -Label 'test'
    }

    $semanticDrift = Copy-JsonObject $fixture.Manifest
    $semanticDrift.semantic_bindings[0].semantic_sha256 = 'f' * 64
    Assert-SelfTestRejected -Code 'TRACE_SEMANTIC_BINDING_HASH' -Action {
        $null = Assert-TraceabilityDocument -Manifest $semanticDrift -InventoryContext $fixture.Context -AsOf $asOf -PredecessorBytes $null -ExpectedInventorySha256 $fixture.Manifest.inventory.sha256 -SkipEvalCrosscheck
    }
    $missingEnforcement = Copy-JsonObject $fixture.Manifest
    $missingEnforcement.gates[0].selector_kind = 'powershell_function'
    $missingEnforcement.gates[0].selector = 'Missing-SelfTest-Gate'
    Assert-SelfTestRejected -Code 'TRACE_ENFORCEMENT_MISSING' -Action {
        $null = Assert-TraceabilityDocument -Manifest $missingEnforcement -InventoryContext $fixture.Context -AsOf $asOf -PredecessorBytes $null -ExpectedInventorySha256 $fixture.Manifest.inventory.sha256 -SkipEvalCrosscheck
    }
    $activeMutation = Copy-JsonObject $fixture.Inventory
    $activeMutation.tests[0].selector = '-ChangedSelfTest'
    Assert-SelfTestRejected -Code 'TRACE_ACTIVE_ID_MUTATED' -Action {
        Assert-CollectionHistory -Previous @($fixture.Inventory.tests) -Current @($activeMutation.tests) -IdProperty 'test_id' -Label 'test'
    }
    $gateMutation = Copy-JsonObject $fixture.Manifest
    $gateMutation.gates[0].sha256 = 'f' * 64
    $gateMutationError = $null
    try {
        Assert-CollectionHistory -Previous @($fixture.Manifest.gates) -Current @($gateMutation.gates) -IdProperty 'gate_id' -Label 'gate'
    } catch {
        $gateMutationError = $_.Exception.Message
    }
    if ($null -eq $gateMutationError -or
        -not $gateMutationError.StartsWith('[TRACE_ACTIVE_ID_MUTATED]', [StringComparison]::Ordinal)) {
        throw "traceability self-test allowed an active gate hash mutation: $gateMutationError"
    }
    $chain = @{
        'TEST-CHAIN-V1' = [pscustomobject]@{ status = 'retired'; retirement = [pscustomobject]@{ replaced_by = @('TEST-CHAIN-V2') } }
        'TEST-CHAIN-V2' = [pscustomobject]@{ status = 'retired'; retirement = [pscustomobject]@{ replaced_by = @('TEST-CHAIN-V3') } }
        'TEST-CHAIN-V3' = [pscustomobject]@{ status = 'active'; retirement = $null }
    }
    $terminals = @(Get-ActiveReplacementNodes -Node $chain['TEST-CHAIN-V1'] -Map $chain -Label 'self-test chain')
    if ($terminals.Count -ne 1 -or $terminals[0] -ne $chain['TEST-CHAIN-V3']) {
        throw 'traceability self-test did not resolve a transitive active replacement'
    }
    $previousInventoryBytes = $script:Utf8.GetBytes(($fixture.Inventory | ConvertTo-Json -Depth 100 -Compress))
    $badPredecessor = Copy-JsonObject $fixture.Inventory
    $badPredecessor.revision.generation = 2
    $badPredecessor.revision.predecessor = [pscustomobject]@{
        schema = 'rayman.first-party-test-inventory.v1'; generation = 1; sha256 = ('0' * 64)
    }
    Assert-SelfTestRejected -Code 'TRACE_PREDECESSOR_HASH' -Action {
        $null = Assert-Revision -Current $badPredecessor -PredecessorBytes $previousInventoryBytes -Label 'test inventory' -ExpectedSchema 'rayman.first-party-test-inventory.v1'
    }
    $retiredAssetPresent = Copy-JsonObject $fixture.Inventory
    $retiredAssetPresent.assets = @([pscustomobject]@{
            asset_id = 'ASSET-SELF-RETIRED'; status = 'retired'; path = 'scripts/check-test-traceability.ps1'
            sha256 = (Get-FileHash -LiteralPath (Resolve-TracePath -RelativePath 'scripts/check-test-traceability.ps1') -Algorithm SHA256).Hash.ToLowerInvariant()
            kind = 'dedicated_file'; cleanup_policy = 'delete_on_last_reference'
            retirement = [pscustomobject]@{ retired_at = '2026-08-28'; reason = 'self test'; replaced_by = @() }
        })
    Assert-SelfTestRejected -Code 'TRACE_RETIRED_ASSET_PRESENT' -Action {
        $null = Assert-InventoryDocument -Inventory $retiredAssetPresent -Discovered $fixture.Discovered -PredecessorBytes $null
    }

    $cascadePrevious = Copy-JsonObject $fixture.Manifest
    $cascadePreviousBytes = $script:Utf8.GetBytes(($cascadePrevious | ConvertTo-Json -Depth 100 -Compress))
    $cascade = Copy-JsonObject $cascadePrevious
    $cascade.revision.generation = 2
    $cascade.revision.predecessor = [pscustomobject]@{
        schema = 'rayman.test-traceability.v2'; generation = 1
        sha256 = Get-Sha256Bytes -Bytes $cascadePreviousBytes
    }
    foreach ($node in @($cascade.sources[0], $cascade.rules[0], $cascade.gates[0], $cascade.source_rule_links[0], $cascade.semantic_bindings[0])) {
        $node.status = 'retired'
        $node.retirement = [pscustomobject]@{ retired_at = '2026-08-28'; reason = 'declaration removed'; replaced_by = @() }
    }
    Assert-SelfTestRejected -Code 'TRACE_RETIREMENT_CASCADE' -Action {
        $null = Assert-TraceabilityDocument -Manifest $cascade -InventoryContext $fixture.Context -AsOf $asOf -PredecessorBytes $cascadePreviousBytes -ExpectedInventorySha256 $fixture.Manifest.inventory.sha256 -SkipEvalCrosscheck
    }

    # A shared test survives retirement of only one separately reviewed
    # source/binding. Both semantic paths exist in the predecessor; no new edge
    # is fabricated in the retirement revision.
    $sharedInventory = Copy-JsonObject $fixture.Inventory
    $sharedContext = Assert-InventoryDocument -Inventory $sharedInventory -Discovered $fixture.Discovered -PredecessorBytes $null
    $sharedInventoryBytes = $script:Utf8.GetBytes(($sharedInventory | ConvertTo-Json -Depth 100 -Compress))
    $shared = Copy-JsonObject $fixture.Manifest
    $shared.inventory.sha256 = Get-Sha256Bytes -Bytes $sharedInventoryBytes
    $secondSource = Copy-JsonObject $shared.sources[0]
    $secondSource.source_id = 'SRC-SELF-SHARED'
    $secondSource.title = 'shared self-test source'
    $shared.sources = @($shared.sources + $secondSource | Sort-Object source_id)
    $secondLink = Copy-JsonObject $shared.source_rule_links[0]
    $secondLink.link_id = 'LINK-SELF-SHARED'
    $secondLink.source_id = 'SRC-SELF-SHARED'
    $shared.source_rule_links = @($shared.source_rule_links + $secondLink | Sort-Object link_id)
    $secondBinding = Copy-JsonObject $shared.semantic_bindings[0]
    $secondBinding.binding_id = 'BIND-SELF-SHARED'
    $secondBinding.source_rule_link_ids = @('LINK-SELF-SHARED')
    $secondBinding.semantic_sha256 = '0' * 64
    $shared.semantic_bindings = @($shared.semantic_bindings + $secondBinding | Sort-Object binding_id)
    $sharedSources = Get-NodeMap -Items @($shared.sources) -IdProperty 'source_id' -Prefix 'SRC' -Label 'source'
    $sharedRules = Get-NodeMap -Items @($shared.rules) -IdProperty 'rule_id' -Prefix 'RULE' -Label 'rule'
    $sharedGates = Get-NodeMap -Items @($shared.gates) -IdProperty 'gate_id' -Prefix 'GATE' -Label 'gate'
    $sharedLinks = Get-NodeMap -Items @($shared.source_rule_links) -IdProperty 'link_id' -Prefix 'LINK' -Label 'link'
    foreach ($binding in $shared.semantic_bindings) {
        $binding.semantic_sha256 = Get-SemanticBindingSha256 -Binding $binding -Rule $sharedRules[[string]$binding.rule_id] `
            -Gate $sharedGates[[string]$binding.gate_id] `
            -SourceLinks @($binding.source_rule_link_ids | ForEach-Object { $sharedLinks[[string]$_] }) `
            -Tests @($binding.test_ids | ForEach-Object { $sharedContext.tests[[string]$_] }) `
            -Sources $sharedSources -Assets $sharedContext.assets
    }
    $null = Assert-TraceabilityDocument -Manifest $shared -InventoryContext $sharedContext -AsOf $asOf -PredecessorBytes $null -ExpectedInventorySha256 $shared.inventory.sha256 -SkipEvalCrosscheck
    $sharedPreviousBytes = $script:Utf8.GetBytes(($shared | ConvertTo-Json -Depth 100 -Compress))
    $sharedCurrent = Copy-JsonObject $shared
    $sharedCurrent.revision.generation = 2
    $sharedCurrent.revision.predecessor = [pscustomobject]@{
        schema = 'rayman.test-traceability.v2'; generation = 1
        sha256 = Get-Sha256Bytes -Bytes $sharedPreviousBytes
    }
    foreach ($node in @(
            ($sharedCurrent.sources | Where-Object source_id -eq 'SRC-SELF-SHARED'),
            ($sharedCurrent.source_rule_links | Where-Object link_id -eq 'LINK-SELF-SHARED'),
            ($sharedCurrent.semantic_bindings | Where-Object binding_id -eq 'BIND-SELF-SHARED')
        )) {
        $node.status = 'retired'
        $node.retirement = [pscustomobject]@{ retired_at = '2026-08-28'; reason = 'shared source removed'; replaced_by = @() }
    }
    $null = Assert-TraceabilityDocument -Manifest $sharedCurrent -InventoryContext $sharedContext -AsOf $asOf -PredecessorBytes $sharedPreviousBytes -ExpectedInventorySha256 $shared.inventory.sha256 -SkipEvalCrosscheck

    # A source-rule review, execution gate, and semantic binding can all be
    # replaced while the exact executable test identity remains unchanged.
    $renewPrevious = Copy-JsonObject $fixture.Manifest
    $renewPrevious.sources[0].valid_until = '2026-08-28'
    $renewPrevious.source_rule_links[0].review.valid_until = '2026-08-28'
    $renewPrevious.semantic_bindings[0].review.valid_until = '2026-08-28'
    $renewSources = Get-NodeMap -Items @($renewPrevious.sources) -IdProperty 'source_id' -Prefix 'SRC' -Label 'source'
    $renewRules = Get-NodeMap -Items @($renewPrevious.rules) -IdProperty 'rule_id' -Prefix 'RULE' -Label 'rule'
    $renewGates = Get-NodeMap -Items @($renewPrevious.gates) -IdProperty 'gate_id' -Prefix 'GATE' -Label 'gate'
    $renewLinks = Get-NodeMap -Items @($renewPrevious.source_rule_links) -IdProperty 'link_id' -Prefix 'LINK' -Label 'link'
    $renewPrevious.semantic_bindings[0].semantic_sha256 = Get-SemanticBindingSha256 `
        -Binding $renewPrevious.semantic_bindings[0] -Rule $renewRules['RULE-SELF-TRACEABILITY'] `
        -Gate $renewGates['GATE-SELF-POWERSHELL'] -SourceLinks @($renewLinks['LINK-SELF-SOURCE-RULE']) `
        -Tests @($fixture.Context.tests['TEST-SELF-TRACEABILITY']) -Sources $renewSources -Assets $fixture.Context.assets
    $null = Assert-TraceabilityDocument -Manifest $renewPrevious -InventoryContext $fixture.Context -AsOf $asOf `
        -PredecessorBytes $null -ExpectedInventorySha256 $fixture.Manifest.inventory.sha256 -SkipEvalCrosscheck
    $renewPreviousBytes = $script:Utf8.GetBytes(($renewPrevious | ConvertTo-Json -Depth 100 -Compress))
    $renewCurrent = Copy-JsonObject $renewPrevious
    $renewCurrent.revision.generation = 2
    $renewCurrent.revision.predecessor = [pscustomobject]@{
        schema = 'rayman.test-traceability.v2'; generation = 1
        sha256 = Get-Sha256Bytes -Bytes $renewPreviousBytes
    }
    $newSource = Copy-JsonObject $renewCurrent.sources[0]
    $newSource.source_id = 'SRC-SELF-TRACEABILITY-R2'
    $newSource.valid_until = '2027-08-28'
    $renewCurrent.sources[0].status = 'retired'
    $renewCurrent.sources[0].retirement = [pscustomobject]@{
        retired_at = '2026-08-28'; reason = 'source horizon renewed'; replaced_by = @('SRC-SELF-TRACEABILITY-R2')
    }
    $renewCurrent.sources = @($renewCurrent.sources + $newSource | Sort-Object source_id)
    $newLink = Copy-JsonObject $renewCurrent.source_rule_links[0]
    $newLink.link_id = 'LINK-SELF-SOURCE-RULE-R2'
    $newLink.source_id = 'SRC-SELF-TRACEABILITY-R2'
    $newLink.review = [pscustomobject]@{ reviewed_at = '2026-08-28'; valid_until = '2027-08-28' }
    $renewCurrent.source_rule_links[0].status = 'retired'
    $renewCurrent.source_rule_links[0].retirement = [pscustomobject]@{
        retired_at = '2026-08-28'; reason = 'review horizon renewed'; replaced_by = @('LINK-SELF-SOURCE-RULE-R2')
    }
    $renewCurrent.source_rule_links = @($renewCurrent.source_rule_links + $newLink | Sort-Object link_id)
    $newGate = Copy-JsonObject $renewCurrent.gates[0]
    $newGate.gate_id = 'GATE-SELF-POWERSHELL-R2'
    $renewCurrent.gates[0].status = 'retired'
    $renewCurrent.gates[0].retirement = [pscustomobject]@{
        retired_at = '2026-08-28'; reason = 'execution gate renewed'; replaced_by = @('GATE-SELF-POWERSHELL-R2')
    }
    $renewCurrent.gates = @($renewCurrent.gates + $newGate | Sort-Object gate_id)
    $newBinding = Copy-JsonObject $renewCurrent.semantic_bindings[0]
    $newBinding.binding_id = 'BIND-SELF-TRACEABILITY-R2'
    $newBinding.source_rule_link_ids = @('LINK-SELF-SOURCE-RULE-R2')
    $newBinding.gate_id = 'GATE-SELF-POWERSHELL-R2'
    $newBinding.review = [pscustomobject]@{ reviewed_at = '2026-08-28'; valid_until = '2027-08-28' }
    $newBinding.semantic_sha256 = '0' * 64
    $renewCurrent.semantic_bindings[0].status = 'retired'
    $renewCurrent.semantic_bindings[0].retirement = [pscustomobject]@{
        retired_at = '2026-08-28'; reason = 'semantic review renewed'; replaced_by = @('BIND-SELF-TRACEABILITY-R2')
    }
    $renewCurrent.semantic_bindings = @($renewCurrent.semantic_bindings + $newBinding | Sort-Object binding_id)
    $renewCurrentSources = Get-NodeMap -Items @($renewCurrent.sources) -IdProperty 'source_id' -Prefix 'SRC' -Label 'source'
    $renewCurrentRules = Get-NodeMap -Items @($renewCurrent.rules) -IdProperty 'rule_id' -Prefix 'RULE' -Label 'rule'
    $renewCurrentGates = Get-NodeMap -Items @($renewCurrent.gates) -IdProperty 'gate_id' -Prefix 'GATE' -Label 'gate'
    $renewCurrentLinks = Get-NodeMap -Items @($renewCurrent.source_rule_links) -IdProperty 'link_id' -Prefix 'LINK' -Label 'link'
    $newBinding.semantic_sha256 = Get-SemanticBindingSha256 -Binding $newBinding `
        -Rule $renewCurrentRules['RULE-SELF-TRACEABILITY'] -Gate $renewCurrentGates['GATE-SELF-POWERSHELL-R2'] `
        -SourceLinks @($renewCurrentLinks['LINK-SELF-SOURCE-RULE-R2']) `
        -Tests @($fixture.Context.tests['TEST-SELF-TRACEABILITY']) -Sources $renewCurrentSources -Assets $fixture.Context.assets
    $null = Assert-TraceabilityDocument -Manifest $renewCurrent -InventoryContext $fixture.Context -AsOf $asOf `
        -PredecessorBytes $renewPreviousBytes -ExpectedInventorySha256 $fixture.Manifest.inventory.sha256 -SkipEvalCrosscheck

    # Dedicated asset ownership is path-qualified rather than embedded in an
    # immutable test node, so asset byte replacement does not rewrite a stable
    # test identity.
    $assetPrevious = Copy-JsonObject $fixture.Inventory
    $assetPrevious.assets = @([pscustomobject]@{
            asset_id = 'ASSET-SELF-OLD'; status = 'active'; path = 'scripts/check-test-traceability.ps1'
            sha256 = ('0' * 64); kind = 'dedicated_file'; cleanup_policy = 'delete_on_last_reference'; retirement = $null
        })
    $assetPreviousBytes = $script:Utf8.GetBytes(($assetPrevious | ConvertTo-Json -Depth 100 -Compress))
    $assetCurrent = Copy-JsonObject $assetPrevious
    $assetCurrent.revision.generation = 2
    $assetCurrent.revision.predecessor = [pscustomobject]@{
        schema = 'rayman.first-party-test-inventory.v1'; generation = 1
        sha256 = Get-Sha256Bytes -Bytes $assetPreviousBytes
    }
    $assetCurrent.assets[0].status = 'retired'
    $assetCurrent.assets[0].retirement = [pscustomobject]@{
        retired_at = '2026-08-28'; reason = 'asset bytes replaced'; replaced_by = @('ASSET-SELF-NEW')
    }
    $assetCurrent.assets = @($assetCurrent.assets + [pscustomobject]@{
            asset_id = 'ASSET-SELF-NEW'; status = 'active'; path = 'scripts/check-test-traceability.ps1'
            sha256 = (Get-FileHash -LiteralPath (Resolve-TracePath -RelativePath 'scripts/check-test-traceability.ps1') -Algorithm SHA256).Hash.ToLowerInvariant()
            kind = 'dedicated_file'; cleanup_policy = 'delete_on_last_reference'; retirement = $null
        } | Sort-Object asset_id)
    $null = Assert-InventoryDocument -Inventory $assetCurrent -Discovered $fixture.Discovered -PredecessorBytes $assetPreviousBytes

    $jsonValue = [ordered]@{ label = '中文'; value = "a`r`nb" }
    $canonicalJson = ConvertTo-CanonicalJsonBytes $jsonValue
    Assert-CanonicalManifestBytes -Bytes $canonicalJson -Label 'generated JSON'
    $roundTrip = ConvertFrom-StrictJsonBytes -Bytes $canonicalJson -Label 'generated JSON'
    if ($roundTrip.value -cne "a`r`nb" -or
        (Get-Sha256Bytes $canonicalJson) -cne (Get-Sha256Bytes (ConvertTo-CanonicalJsonBytes $roundTrip))) {
        throw 'canonical JSON generation changed data or was not idempotent'
    }
    $generationRejected = $false
    try { New-TraceabilityCandidate '' '' 'relative' | Out-Null }
    catch { $generationRejected = $_.Exception.Message.Contains('TRACE_GENERATE_ARGUMENTS') }
    if (-not $generationRejected) { throw 'generation accepted incomplete draft/output authority' }
    Assert-CanonicalManifestBytes -Bytes ($script:Utf8.GetBytes("{}`n")) -Label 'LF fixture'
    foreach ($invalidText in @("{}`r`n", ([string][char]0xFEFF + "{}`n"))) {
        $rejected = $false
        try { Assert-CanonicalManifestBytes -Bytes ($script:Utf8.GetBytes($invalidText)) -Label 'invalid fixture' }
        catch { $rejected = $_.Exception.Message.Contains('[TRACE_NONCANONICAL_BYTES]') }
        if (-not $rejected) { throw 'Noncanonical manifest bytes were accepted' }
    }
    $fixtureIdentity = 'rust::eval_fixture::evals/tasks/sample/fixture/src/lib.rs::same_name'
    $oracleIdentity = 'rust::eval_oracle::evals/tasks/sample/oracle/tests.rs::same_name'
    if ($fixtureIdentity -ceq $oracleIdentity -or
        (Get-SuggestedTestId -Identity $fixtureIdentity -SelectorKind 'rust_test_fn' -Selector 'same_name') -ceq
        (Get-SuggestedTestId -Identity $oracleIdentity -SelectorKind 'rust_test_fn' -Selector 'same_name')) {
        throw 'fixture and oracle same-name tests lost their role-qualified identities'
    }
    Write-Output 'check-test-traceability self-test: PASS'
}

if ($Generate) {
    if ($SelfTest -or $ListInventory -or $RuntimeInventory -or $Summary) { throw 'Generation cannot be combined with a check or listing mode' }
    New-TraceabilityCandidate $DraftInventoryPath $DraftManifestPath $OutputDirectory | ConvertTo-Json -Depth 8
    return
}
if ($DraftInventoryPath -or $DraftManifestPath -or $OutputDirectory) { throw 'Draft/output paths require -Generate' }
if ($Summary) {
    $document = ConvertFrom-StrictJsonBytes -Bytes (Get-FileBytes $ManifestPath) -Label 'traceability summary'
    [ordered]@{
        schema = 'rayman.traceability.summary.v1'; authority = $false; generation = $document.revision.generation
        active_sources = @($document.sources | Where-Object status -eq 'active' | Select-Object source_id, path, valid_until)
        active_rules = @($document.rules | Where-Object status -eq 'active' | Select-Object rule_id, statement)
        active_bindings = @($document.semantic_bindings | Where-Object status -eq 'active').Count
        retired_bindings = @($document.semantic_bindings | Where-Object status -eq 'retired').Count
    } | ConvertTo-Json -Depth 8
    return
}
if ($ListInventory) {
    [ordered]@{
        schema = 'rayman.first-party-test-inventory.discovery.v1'
        status = 'observed'
        tests = @(Get-FirstPartyTestInventory)
    } | ConvertTo-Json -Depth 12
    return
}
if ($SelfTest) {
    Invoke-TraceabilitySelfTest
    return
}

$expectedManifest = [IO.Path]::GetFullPath((Join-Path $script:RepoRoot $script:ManifestRelativePath))
$expectedInventory = [IO.Path]::GetFullPath((Join-Path $script:RepoRoot $script:InventoryRelativePath))
$resolvedManifest = [IO.Path]::GetFullPath($ManifestPath)
$resolvedInventory = [IO.Path]::GetFullPath($InventoryPath)
if (-not $resolvedManifest.Equals($expectedManifest, [StringComparison]::OrdinalIgnoreCase) -or
    -not $resolvedInventory.Equals($expectedInventory, [StringComparison]::OrdinalIgnoreCase)) {
    Throw-TraceError 'TRACE_PATH_ESCAPE' 'live traceability check accepts only the fixed repository manifests'
}
$null = Resolve-TracePath -RelativePath $script:ManifestRelativePath
$null = Resolve-TracePath -RelativePath $script:InventoryRelativePath
$inventoryBytes = Get-FileBytes -Path $resolvedInventory
$manifestBytes = Get-FileBytes -Path $resolvedManifest
Assert-CanonicalManifestBytes -Bytes $inventoryBytes -Label 'first-party test inventory'
Assert-CanonicalManifestBytes -Bytes $manifestBytes -Label 'test traceability manifest'
$inventory = ConvertFrom-StrictJsonBytes -Bytes $inventoryBytes -Label 'first-party test inventory'
$manifest = ConvertFrom-StrictJsonBytes -Bytes $manifestBytes -Label 'test traceability manifest'
$discovered = @(Get-FirstPartyTestInventory)
$inventoryPredecessor = Get-PredecessorBytes -RelativePath $script:InventoryRelativePath -CurrentBytes $inventoryBytes
$manifestPredecessor = Get-PredecessorBytes -RelativePath $script:ManifestRelativePath -CurrentBytes $manifestBytes
$inventoryContext = Assert-InventoryDocument -Inventory $inventory -Discovered $discovered -PredecessorBytes $inventoryPredecessor
$inventorySha = Get-Sha256Bytes -Bytes $inventoryBytes
$counts = Assert-TraceabilityDocument -Manifest $manifest -InventoryContext $inventoryContext `
    -AsOf ([DateTime]::UtcNow.Date) -PredecessorBytes $manifestPredecessor -ExpectedInventorySha256 $inventorySha
$runtime = if ($RuntimeInventory) { Assert-RuntimeCargoInventory -InventoryContext $inventoryContext } else { $null }
[ordered]@{
    schema = 'rayman.test-traceability.check.v2'
    status = 'pass'
    scope = [string]$manifest.scope.name
    manifest_sha256 = Get-Sha256Bytes -Bytes $manifestBytes
    inventory_sha256 = $inventorySha
    as_of = [DateTime]::UtcNow.ToString('yyyy-MM-dd')
    counts = $counts
    runtime_inventory = $runtime
} | ConvertTo-Json -Depth 10
