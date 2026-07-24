[CmdletBinding(DefaultParameterSetName = 'Closeout')]
param(
    [Parameter(Mandatory = $true, ParameterSetName = 'Closeout')]
    [ValidateNotNullOrEmpty()]
    [string]$GoalId,

    [Parameter(Mandatory = $true, ParameterSetName = 'Closeout')]
    [ValidateNotNullOrEmpty()]
    [string]$RequirementId,

    [Parameter(Mandatory = $true, ParameterSetName = 'Closeout')]
    [ValidateNotNullOrEmpty()]
    [string]$CliPath,

    [Parameter(Mandatory = $true, ParameterSetName = 'Closeout')]
    [ValidateNotNullOrEmpty()]
    [string]$SkillPath,

    [Parameter(ParameterSetName = 'Closeout')]
    [string[]]$ChangedPath = @(),

    [Parameter(ParameterSetName = 'Closeout')]
    [string]$EvidencePath = '.RaymanCodingSkill/release-closeout-evidence.json',

    [Parameter(ParameterSetName = 'Closeout')]
    [switch]$AllowEvidenceReuse,

    [Parameter(ParameterSetName = 'Closeout')]
    [switch]$CloseAndFinish,

    [Parameter(Mandatory = $true, ParameterSetName = 'SelfTest')]
    [switch]$SelfTest
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if ($PSVersionTable.PSVersion.Major -lt 7) {
    throw 'release-closeout.ps1 requires PowerShell 7+. Run it with pwsh.'
}

$repoRoot = Split-Path -Parent $PSScriptRoot

function Get-Sha256 {
    param([Parameter(Mandatory = $true)][string]$Path)
    return (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}

function Get-ObjectSha256 {
    param([Parameter(Mandatory = $true)]$Value)
    $json = $Value | ConvertTo-Json -Depth 20 -Compress
    $bytes = [Text.Encoding]::UTF8.GetBytes($json)
    $hash = [Security.Cryptography.SHA256]::HashData($bytes)
    return [Convert]::ToHexString($hash).ToLowerInvariant()
}

function Resolve-ApplicationIdentity {
    param([Parameter(Mandatory = $true)][string]$Name)
    $commands = @(Get-Command $Name -All -ErrorAction SilentlyContinue)
    if ($commands.Count -eq 0 -or $commands[0].CommandType -ne 'Application') {
        throw "$Name must resolve directly to an Application."
    }
    $path = (Resolve-Path -LiteralPath $commands[0].Source).Path
    return [ordered]@{ path = $path; sha256 = Get-Sha256 $path }
}

function Resolve-OrdinaryFile {
    param([Parameter(Mandatory = $true)][string]$Path, [string]$Label)
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        throw "$Label is missing: $Path"
    }
    $item = Get-Item -LiteralPath $Path -Force
    if ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) {
        throw "$Label must not be a symlink or reparse point: $Path"
    }
    return $item.FullName
}

function Get-ReleaseBinding {
    param([Parameter(Mandatory = $true)][string]$Cli, [Parameter(Mandatory = $true)][string]$Skill)

    $git = Resolve-ApplicationIdentity 'git'
    $status = & $git.path '-C' $repoRoot 'status' '--porcelain=v1' '--untracked-files=all'
    if ($LASTEXITCODE -ne 0) { throw 'git status failed while building release binding.' }
    if (@($status).Count -ne 0) {
        throw 'Release closeout requires a clean source tree; dirty evidence is never reusable.'
    }
    $head = (& $git.path '-C' $repoRoot 'rev-parse' 'HEAD').Trim()
    if ($LASTEXITCODE -ne 0 -or $head -notmatch '^[0-9a-f]{40}$') {
        throw 'Unable to resolve an exact Git HEAD for release binding.'
    }

    $resolvedCli = Resolve-OrdinaryFile $Cli 'CLI'
    $resolvedSkill = Resolve-OrdinaryFile $Skill 'Skill'
    $scripts = [ordered]@{}
    foreach ($name in @(
        'audit-repository.ps1',
        'check-repo.ps1',
        'release-closeout.ps1',
        'verify-release-contract.ps1'
    )) {
        $path = Resolve-OrdinaryFile (Join-Path $PSScriptRoot $name) "Release script $name"
        $scripts[$name] = Get-Sha256 $path
    }
    $tools = [ordered]@{}
    foreach ($name in @('cargo', 'git', 'pwsh', 'rustc', 'rustup')) {
        $tools[$name] = Resolve-ApplicationIdentity $name
    }

    return [ordered]@{
        schema = 'rayman.release.binding.v1'
        workspace = (Resolve-Path -LiteralPath $repoRoot).Path
        head = $head
        clean = $true
        cli = [ordered]@{ path = $resolvedCli; sha256 = Get-Sha256 $resolvedCli }
        skill = [ordered]@{ path = $resolvedSkill; sha256 = Get-Sha256 $resolvedSkill }
        scripts = $scripts
        tools = $tools
    }
}

function Test-ReusableEvidence {
    param([Parameter(Mandatory = $true)]$Evidence, [Parameter(Mandatory = $true)]$Binding)

    if ($Evidence.schema -ne 'rayman.release.evidence.v1' -or $Evidence.status -ne 'pass') {
        return $false
    }
    $recordedHash = [string]$Evidence.binding_sha256
    if ($recordedHash -notmatch '^[0-9a-f]{64}$') {
        return $false
    }
    return (Get-ObjectSha256 $Evidence.binding) -eq $recordedHash -and
        (Get-ObjectSha256 $Binding) -eq $recordedHash
}

function Get-AuthorityArguments {
    param([string]$Goal, [string]$Requirement, [string[]]$Changed)

    $arguments = @(
        'goal', 'validate', $Goal,
        '--req', $Requirement,
        '--message', 'release closeout authority gate passed twice on one exact source binding',
        '--command', 'pwsh -NoProfile -File scripts/check-repo.ps1',
        '--authority', '--repeat', '2'
    )
    foreach ($path in $Changed) {
        if ([string]::IsNullOrWhiteSpace($path)) { throw 'ChangedPath must not be blank.' }
        $arguments += @('--changed', $path)
    }
    return $arguments
}

function Resolve-EvidencePath {
    param([string]$Path)
    $full = [IO.Path]::GetFullPath((Join-Path $repoRoot $Path))
    $stateRoot = [IO.Path]::GetFullPath((Join-Path $repoRoot '.RaymanCodingSkill'))
    $comparison = if ($IsWindows) { [StringComparison]::OrdinalIgnoreCase } else { [StringComparison]::Ordinal }
    $prefix = $stateRoot.TrimEnd([IO.Path]::DirectorySeparatorChar) + [IO.Path]::DirectorySeparatorChar
    if (-not $full.StartsWith($prefix, $comparison)) {
        throw "EvidencePath must stay inside .RaymanCodingSkill: $full"
    }
    return $full
}

function Write-ReleaseEvidence {
    param([string]$Path, $Binding)

    $parent = Split-Path -Parent $Path
    New-Item -ItemType Directory -Path $parent -Force | Out-Null
    $record = [ordered]@{
        schema = 'rayman.release.evidence.v1'
        status = 'pass'
        recorded_at_utc = [DateTimeOffset]::UtcNow.ToString('O')
        binding_sha256 = Get-ObjectSha256 $Binding
        binding = $Binding
        audit = 'scripts/audit-repository.ps1'
        source_fresh = 'scripts/verify-release-contract.ps1 -RequireSourceFresh'
    }
    $temporary = "$Path.tmp-$([Guid]::NewGuid().ToString('N'))"
    try {
        [IO.File]::WriteAllText(
            $temporary,
            ($record | ConvertTo-Json -Depth 20),
            [Text.UTF8Encoding]::new($false)
        )
        Move-Item -LiteralPath $temporary -Destination $Path -Force
    } finally {
        if (Test-Path -LiteralPath $temporary -PathType Leaf) {
            Remove-Item -LiteralPath $temporary -Force
        }
    }
}

if ($SelfTest) {
    $binding = [ordered]@{
        schema = 'rayman.release.binding.v1'
        head = ('a' * 40)
        clean = $true
        cli = [ordered]@{ sha256 = ('b' * 64) }
        scripts = [ordered]@{ audit = ('c' * 64) }
        tools = [ordered]@{ cargo = [ordered]@{ sha256 = ('d' * 64) } }
    }
    $evidence = [pscustomobject]@{
        schema = 'rayman.release.evidence.v1'
        status = 'pass'
        binding_sha256 = Get-ObjectSha256 $binding
        binding = $binding
    }
    if (-not (Test-ReusableEvidence $evidence $binding)) {
        throw 'release closeout self-test rejected an exact binding.'
    }
    $drifted = $binding | ConvertTo-Json -Depth 20 | ConvertFrom-Json -Depth 20
    $drifted.head = 'e' * 40
    if (Test-ReusableEvidence $evidence $drifted) {
        throw 'release closeout self-test reused evidence across HEAD drift.'
    }
    $authority = @(Get-AuthorityArguments 'goal_test' 'req_8' @('a.rs'))
    if ($authority -notcontains '--authority' -or
        $authority[([Array]::IndexOf($authority, '--repeat') + 1)] -ne '2') {
        throw 'release closeout self-test lost mandatory authority repeat 2.'
    }
    Write-Output 'release-closeout self-test: PASS'
    return
}

Push-Location $repoRoot
try {
    $binding = Get-ReleaseBinding -Cli $CliPath -Skill $SkillPath
    $evidenceFile = Resolve-EvidencePath $EvidencePath
    $reuse = $false
    if ($AllowEvidenceReuse -and (Test-Path -LiteralPath $evidenceFile -PathType Leaf)) {
        try {
            $existing = Get-Content -Raw -LiteralPath $evidenceFile |
                ConvertFrom-Json -Depth 20 -ErrorAction Stop
            $reuse = Test-ReusableEvidence $existing $binding
        } catch {
            $reuse = $false
        }
    }

    if ($reuse) {
        Write-Output 'RAYMAN_RELEASE_PHASE {"phase":"audit_and_source_fresh","status":"reused_exact_binding"}'
    } else {
        Write-Output 'RAYMAN_RELEASE_PHASE {"phase":"audit_and_source_fresh","status":"start"}'
        & (Join-Path $PSScriptRoot 'audit-repository.ps1') -CliPath $CliPath -SkillPath $SkillPath
        & (Join-Path $PSScriptRoot 'verify-release-contract.ps1') -CliPath $CliPath -ReferenceCliPath $CliPath -SkillPath $SkillPath -RequirePath -RequireSourceFresh
        $after = Get-ReleaseBinding -Cli $CliPath -Skill $SkillPath
        if ((Get-ObjectSha256 $after) -ne (Get-ObjectSha256 $binding)) {
            throw 'Release binding drifted during audit; evidence was not written.'
        }
        Write-ReleaseEvidence -Path $evidenceFile -Binding $binding
        Write-Output 'RAYMAN_RELEASE_PHASE {"phase":"audit_and_source_fresh","status":"pass"}'
    }

    Write-Output 'RAYMAN_RELEASE_PHASE {"phase":"authority_repeat_2","status":"start"}'
    $authorityArguments = Get-AuthorityArguments $GoalId $RequirementId $ChangedPath
    & $CliPath @authorityArguments
    if ($LASTEXITCODE -ne 0) { throw 'Goal authority repeat 2 failed.' }
    Write-Output 'RAYMAN_RELEASE_PHASE {"phase":"authority_repeat_2","status":"pass"}'

    if ($CloseAndFinish) {
        & $CliPath 'goal' 'close' $GoalId
        if ($LASTEXITCODE -ne 0) { throw 'Goal close failed.' }
        & $CliPath 'finish' '--goal' $GoalId '--profile' 'release'
        if ($LASTEXITCODE -ne 0) { throw 'Goal-bound release finish failed.' }
    }
} finally {
    Pop-Location
}
