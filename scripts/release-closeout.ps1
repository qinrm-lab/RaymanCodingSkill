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
    $path = (Resolve-Path -LiteralPath $commands[0].Source).ProviderPath
    $identity = [ordered]@{ path = $path; sha256 = Get-Sha256 $path }
    # Under rustup the toolchain binaries on PATH are shims: `rustc.exe` is
    # byte-identical to `rustup.exe` and never changes when the active toolchain
    # changes. Hashing the shim alone let a `rustup update` / `rustup default`
    # between two closeouts look like an unchanged toolchain, and the reuse
    # branch then skips the source-fresh verification entirely. Bind the
    # reported version too, which is what actually compiles the artifact.
    if ($Name -in @('cargo', 'rustc')) {
        $version = & $path --version 2>&1
        if ($LASTEXITCODE -ne 0) {
            throw "$Name --version failed while binding the release toolchain identity."
        }
        $identity['version'] = ($version | Out-String).Trim()
    }
    return $identity
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
        'repository-quality.ps1',
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
        workspace = (Resolve-Path -LiteralPath $repoRoot).ProviderPath
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
    if ($Changed.Count -eq 0) {
        $arguments += '--workspace-snapshot'
    }
    return $arguments
}

# The evidence file must be one `rayman state audit --check` allowlists.
#
# That audit allowlists top-level names only, and the workflow contract forbids
# deleting state to make it pass: any other in-state path (a nested
# `evidence/run1.json`, a differently named file) passed this check, was written
# after the audit had already run, and then red-lined every later
# `state audit --check` — including the one audit-repository.ps1 runs — with no
# resolution but hand-deleting the artifact. Fail here instead, before the run.
$script:AllowedEvidenceName = 'release-closeout-evidence.json'

function Resolve-EvidencePath {
    param([string]$Path)
    $full = [IO.Path]::GetFullPath((Join-Path $repoRoot $Path))
    $stateRoot = [IO.Path]::GetFullPath((Join-Path $repoRoot '.RaymanCodingSkill'))
    $expected = [IO.Path]::GetFullPath((Join-Path $stateRoot $script:AllowedEvidenceName))
    $comparison = if ($IsWindows) { [StringComparison]::OrdinalIgnoreCase } else { [StringComparison]::Ordinal }
    if (-not $full.Equals($expected, $comparison)) {
        throw "EvidencePath must be the state-audit allowlisted artifact '$script:AllowedEvidenceName' directly under .RaymanCodingSkill (any other path permanently fails `rayman state audit --check`): got $full"
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
    # The scratch name must be one `rayman state audit --check` tolerates. A
    # closeout interrupted between write and rename otherwise leaves a file in
    # `.RaymanCodingSkill/` that matches neither the allowlist nor the audit's
    # leaked-atomic-temp shape, permanently red-lining the very gate this
    # -EvidencePath pinning exists to protect. Mirror what `file_io` produces:
    # `.<allowed-name>.rayman-<pid>-<counter>.tmp`.
    $temporary = Join-Path (Split-Path -Parent $Path) (
        '.{0}.rayman-{1}-{2}.tmp' -f (Split-Path -Leaf $Path), $PID, [Math]::Abs([Guid]::NewGuid().GetHashCode())
    )
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
    $snapshotAuthority = @(Get-AuthorityArguments 'goal_test' 'req_8' @())
    if ($snapshotAuthority -notcontains '--workspace-snapshot' -or
        $snapshotAuthority -contains '--non-code' -or
        $authority -contains '--workspace-snapshot') {
        throw 'release closeout self-test lost the zero-delta workspace snapshot scope boundary.'
    }
    # EvidencePath 必须与 `rayman state audit --check` 的白名单一致，否则一次
    # closeout 就会让此后每一次仓库审计永久翻红（见 Resolve-EvidencePath）。
    if ((Resolve-EvidencePath '.RaymanCodingSkill/release-closeout-evidence.json') -ne
        [IO.Path]::GetFullPath((Join-Path $repoRoot '.RaymanCodingSkill/release-closeout-evidence.json'))) {
        throw 'release closeout self-test rejected the allowlisted evidence path.'
    }
    foreach ($rejected in @(
        '.RaymanCodingSkill/evidence/run1.json',
        '.RaymanCodingSkill/other-evidence.json',
        'release-closeout-evidence.json'
    )) {
        $accepted = $true
        try { Resolve-EvidencePath $rejected | Out-Null } catch { $accepted = $false }
        if ($accepted) {
            throw "release closeout self-test accepted a state-audit-hostile EvidencePath: $rejected"
        }
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
        # The stored binding pins CLI/SKILL/script/tool bytes, but not which `rayman` this
        # shell resolves on PATH — that is runtime state the binding cannot capture. Re-run
        # the cheap PATH-identity check (no rebuild, no repository audit) so a reused
        # closeout still fails when the effective PATH `rayman` differs from -CliPath, just
        # like the non-reuse branch below.
        & (Join-Path $PSScriptRoot 'verify-release-contract.ps1') -CliPath $CliPath -ReferenceCliPath $CliPath -SkillPath $SkillPath -WorkspaceSkillPath (Join-Path $repoRoot 'SKILL.md') -RequirePath
        Write-Output 'RAYMAN_RELEASE_PHASE {"phase":"audit_and_source_fresh","status":"reused_exact_binding"}'
    } else {
        Write-Output 'RAYMAN_RELEASE_PHASE {"phase":"audit_and_source_fresh","status":"start"}'
        & (Join-Path $PSScriptRoot 'audit-repository.ps1') -CliPath $CliPath -SkillPath $SkillPath
        & (Join-Path $PSScriptRoot 'verify-release-contract.ps1') -CliPath $CliPath -ReferenceCliPath $CliPath -SkillPath $SkillPath -WorkspaceSkillPath (Join-Path $repoRoot 'SKILL.md') -RequirePath -RequireSourceFresh
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
