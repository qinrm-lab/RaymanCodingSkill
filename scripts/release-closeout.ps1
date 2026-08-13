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
switch ($PSCmdlet.ParameterSetName) {
    'SelfTest' {
        if (-not $SelfTest.IsPresent) {
            throw 'The SelfTest parameter set requires -SelfTest to be present and true; -SelfTest:$false grants no self-test authority.'
        }
    }
}

$repoRoot = Split-Path -Parent $PSScriptRoot
$script:RequiredMsrv = '1.97.1'
$script:RequiredCoverageToolVersion = '0.8.7'
$script:PathComparison = if ($IsWindows) {
    [StringComparison]::OrdinalIgnoreCase
} else {
    [StringComparison]::Ordinal
}

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

function Get-SourceFreshInputInspection {
    $verifier = Resolve-OrdinaryFile `
        -Path (Join-Path $PSScriptRoot 'verify-release-contract.ps1') `
        -Label 'Release verifier'
    $output = & $verifier -InspectSourceFreshInputs
    $text = ($output | Out-String).Trim()
    if ([string]::IsNullOrWhiteSpace($text)) {
        throw 'Release verifier source-fresh input inspection returned no output.'
    }
    try {
        $inspection = $text | ConvertFrom-Json -Depth 20 -ErrorAction Stop
    } catch {
        throw "Release verifier source-fresh input inspection returned invalid JSON: $($_.Exception.Message)"
    }
    try {
        $environment = $inspection.source_fresh_environment
        $activation = $inspection.workspace_activation
        if ($inspection.schema -ne 'rayman.source-fresh.input-inspection.v1' -or
            $activation.schema -ne 'rayman.workspace-activation.snapshot.v1' -or
            $activation.path -notmatch '[\\/]\.RaymanCodingSkill[\\/]workspace_skill\.yaml$' -or
            $activation.sha256 -notmatch '^[0-9a-f]{64}$' -or
            $environment.schema -ne 'rayman.source-fresh.environment.v1' -or
            $environment.policy.schema -ne 'rayman.source-fresh.environment-policy.v1' -or
            $environment.policy_sha256 -notmatch '^[0-9a-f]{64}$' -or
            $environment.policy_sha256 -ne (Get-ObjectSha256 $environment.policy) -or
            $environment.clear -ne $true -or
            @($environment.rejected_names).Count -ne 0) {
            throw 'inspection fields are incomplete, inconsistent, or not clear'
        }
    } catch {
        throw "Release verifier source-fresh input inspection failed closed: $($_.Exception.Message)"
    }
    return $inspection
}

function Resolve-ApplicationIdentity {
    param([Parameter(Mandatory = $true)][string]$Name)
    $commands = @(Get-Command $Name -All -ErrorAction SilentlyContinue)
    if ($commands.Count -eq 0 -or $commands[0].CommandType -ne 'Application') {
        throw "$Name must resolve directly to an Application."
    }
    $path = (Resolve-Path -LiteralPath $commands[0].Source).ProviderPath
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

function Invoke-VersionText {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string[]]$Arguments,
        [Parameter(Mandatory = $true)][string]$Label
    )

    $output = & $Path @Arguments 2>&1
    if ($LASTEXITCODE -ne 0) {
        throw "$Label version query failed while building the release binding."
    }
    $text = ($output | Out-String).Trim()
    if ([string]::IsNullOrWhiteSpace($text)) {
        throw "$Label version query returned no output."
    }
    return $text
}

function Get-ExactApplicationIdentity {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$Label,
        [Parameter(Mandatory = $true)][string[]]$VersionArguments
    )

    $resolved = Resolve-OrdinaryFile $Path $Label
    return [ordered]@{
        path = $resolved
        sha256 = Get-Sha256 $resolved
        version = Invoke-VersionText `
            -Path $resolved `
            -Arguments $VersionArguments `
            -Label $Label
    }
}

function Get-ResolvedApplicationIdentity {
    param(
        [Parameter(Mandatory = $true)][string]$Name,
        [Parameter(Mandatory = $true)][string[]]$VersionArguments
    )

    $identity = Resolve-ApplicationIdentity $Name
    $identity['version'] = Invoke-VersionText `
        -Path $identity.path `
        -Arguments $VersionArguments `
        -Label $Name
    return $identity
}

function Get-CurrentPowerShellHostIdentity {
    $candidate = [Environment]::ProcessPath
    if ([string]::IsNullOrWhiteSpace($candidate)) {
        $applicationName = if ($IsWindows) { 'pwsh.exe' } else { 'pwsh' }
        $candidate = Join-Path $PSHOME $applicationName
    }
    return Get-ExactApplicationIdentity `
        -Path $candidate `
        -Label 'current PowerShell host' `
        -VersionArguments @('--version')
}

function Resolve-RustupToolchainIdentity {
    param(
        [Parameter(Mandatory = $true)]$Rustup,
        [Parameter(Mandatory = $true)][ValidateSet('cargo', 'rustc')][string]$Name
    )

    $output = & $Rustup.path 'which' $Name '--toolchain' $script:RequiredMsrv 2>&1
    if ($LASTEXITCODE -ne 0) {
        throw "rustup which $Name --toolchain $script:RequiredMsrv failed while building the release binding."
    }
    $paths = @(
        $output |
            ForEach-Object { $_.ToString().Trim() } |
            Where-Object { -not [string]::IsNullOrWhiteSpace($_) }
    )
    if ($paths.Count -ne 1) {
        throw "rustup which $Name --toolchain $script:RequiredMsrv returned $($paths.Count) paths; expected exactly one."
    }
    $identity = Get-ExactApplicationIdentity `
        -Path $paths[0] `
        -Label "MSRV $script:RequiredMsrv $Name" `
        -VersionArguments @('--version')
    if ($identity.version -notmatch ('^' + [regex]::Escape($Name) + ' ' + [regex]::Escape($script:RequiredMsrv) + '(?:\s|$)')) {
        throw "MSRV $Name reports '$($identity.version)', expected exact $Name $script:RequiredMsrv."
    }
    return $identity
}

function Resolve-MsrvLlvmIdentities {
    param([Parameter(Mandatory = $true)]$Rustc)

    $targetLibdirOutput = & $Rustc.path '--print' 'target-libdir' 2>&1
    if ($LASTEXITCODE -ne 0) {
        throw "MSRV rustc target-libdir query failed while building the release binding."
    }
    $targetLibdirs = @(
        $targetLibdirOutput |
            ForEach-Object { $_.ToString().Trim() } |
            Where-Object { -not [string]::IsNullOrWhiteSpace($_) }
    )
    if ($targetLibdirs.Count -ne 1) {
        throw "MSRV rustc target-libdir returned $($targetLibdirs.Count) paths; expected exactly one."
    }
    $targetLibdir = Get-Item -LiteralPath $targetLibdirs[0] -Force -ErrorAction Stop
    if (-not $targetLibdir.PSIsContainer -or
        $targetLibdir.Attributes -band [IO.FileAttributes]::ReparsePoint) {
        throw "MSRV rustc target-libdir must be an ordinary directory: $($targetLibdirs[0])"
    }
    $resolvedTargetLibdir = (Resolve-Path -LiteralPath $targetLibdir.FullName).ProviderPath
    $llvmBin = Join-Path (Split-Path -Parent $resolvedTargetLibdir) 'bin'
    $suffix = if ($IsWindows) { '.exe' } else { '' }
    return [ordered]@{
        'llvm-cov' = Get-ExactApplicationIdentity `
            -Path (Join-Path $llvmBin "llvm-cov$suffix") `
            -Label "MSRV $script:RequiredMsrv llvm-cov" `
            -VersionArguments @('--version')
        'llvm-profdata' = Get-ExactApplicationIdentity `
            -Path (Join-Path $llvmBin "llvm-profdata$suffix") `
            -Label "MSRV $script:RequiredMsrv llvm-profdata" `
            -VersionArguments @('--version')
    }
}

function Get-CargoOfflineBinding {
    $present = Test-Path -LiteralPath 'Env:CARGO_NET_OFFLINE'
    $raw = if ($present) { (Get-Item -LiteralPath 'Env:CARGO_NET_OFFLINE').Value } else { $null }
    $effective = -not [string]::IsNullOrWhiteSpace($raw) -and
        $raw.Trim().ToLowerInvariant() -in @('1', 'true')
    return [ordered]@{
        present = $present
        raw = $raw
        effective = $effective
    }
}

function Get-DirectoryTreeDigest {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$Label
    )

    $rootItem = Get-Item -LiteralPath $Path -Force -ErrorAction Stop
    if (-not $rootItem.PSIsContainer -or
        $rootItem.Attributes -band [IO.FileAttributes]::ReparsePoint) {
        throw "$Label must be an ordinary directory: $Path"
    }
    $root = (Resolve-Path -LiteralPath $rootItem.FullName).ProviderPath
    $records = [Collections.Generic.List[string]]::new()
    $items = @(Get-ChildItem -LiteralPath $root -Force -Recurse)
    foreach ($item in $items) {
        if ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) {
            throw "$Label must not contain a symlink or reparse point: $($item.FullName)"
        }
        $full = (Resolve-Path -LiteralPath $item.FullName).ProviderPath
        $prefix = $root.TrimEnd([IO.Path]::DirectorySeparatorChar) + [IO.Path]::DirectorySeparatorChar
        if (-not $full.StartsWith($prefix, $script:PathComparison)) {
            throw "$Label entry escaped its canonical root: $full"
        }
        $relative = $full.Substring($prefix.Length).Replace([IO.Path]::DirectorySeparatorChar, '/')
        if ($item.PSIsContainer) {
            $records.Add("D`t$relative")
        } else {
            $records.Add("F`t$relative`t$($item.Length)`t$(Get-Sha256 $full)")
        }
    }
    $canonicalRecords = @($records | Sort-Object -CaseSensitive)
    $payload = [string]::Join("`n", $canonicalRecords)
    $digest = [Convert]::ToHexString(
        [Security.Cryptography.SHA256]::HashData([Text.Encoding]::UTF8.GetBytes($payload))
    ).ToLowerInvariant()
    return [ordered]@{
        path = $root
        state = 'present'
        sha256 = $digest
        entries = $canonicalRecords.Count
    }
}

function Get-AdvisoryDatabaseBinding {
    $cargoHome = if ([string]::IsNullOrWhiteSpace($env:CARGO_HOME)) {
        Join-Path $HOME '.cargo'
    } else {
        $env:CARGO_HOME
    }
    $path = [IO.Path]::GetFullPath((Join-Path $cargoHome 'advisory-dbs'))
    if (-not (Test-Path -LiteralPath $path)) {
        return [ordered]@{
            path = $path
            state = 'missing'
            sha256 = $null
            entries = 0
        }
    }
    if (-not (Test-Path -LiteralPath $path -PathType Container)) {
        throw "Default cargo-deny advisory database must be a directory when present: $path"
    }
    return Get-DirectoryTreeDigest -Path $path -Label 'Default cargo-deny advisory database'
}

function Get-ReleaseBinding {
    param(
        [Parameter(Mandatory = $true)][string]$Cli,
        [Parameter(Mandatory = $true)][string]$Skill
    )

    # The verifier owns both the environment policy and activation snapshot.
    # Calling its read-only inspector on every binding computation makes the
    # initial, post-audit, reuse, and terminal boundaries use one exact source.
    $sourceFreshInputs = Get-SourceFreshInputInspection
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
    $tools = [ordered]@{
        cargo = Get-ResolvedApplicationIdentity -Name 'cargo' -VersionArguments @('--version')
        git = Get-ResolvedApplicationIdentity -Name 'git' -VersionArguments @('--version')
        pwsh = Get-ResolvedApplicationIdentity -Name 'pwsh' -VersionArguments @('--version')
        rustc = Get-ResolvedApplicationIdentity -Name 'rustc' -VersionArguments @('--version')
        rustup = Get-ResolvedApplicationIdentity -Name 'rustup' -VersionArguments @('--version')
        'cargo-deny' = Get-ResolvedApplicationIdentity -Name 'cargo-deny' -VersionArguments @('--version')
        'pwsh-host' = Get-CurrentPowerShellHostIdentity
    }
    try {
        $tools['cargo-llvm-cov'] = Get-ResolvedApplicationIdentity `
            -Name 'cargo-llvm-cov' `
            -VersionArguments @('llvm-cov', '--version')
    } catch {
        throw "Release closeout requires preinstalled cargo-llvm-cov $script:RequiredCoverageToolVersion on PATH. Run 'pwsh -NoProfile -File scripts/audit-repository.ps1 -PrepareAuditTools' separately, then rerun closeout. $($_.Exception.Message)"
    }
    $expectedCoverageVersion = "cargo-llvm-cov $script:RequiredCoverageToolVersion"
    if ($tools['cargo-llvm-cov'].version -cne $expectedCoverageVersion) {
        throw "cargo-llvm-cov reports '$($tools['cargo-llvm-cov'].version)', expected exact '$expectedCoverageVersion'."
    }

    $msrv = [ordered]@{
        toolchain = $script:RequiredMsrv
        cargo = Resolve-RustupToolchainIdentity -Rustup $tools.rustup -Name 'cargo'
        rustc = Resolve-RustupToolchainIdentity -Rustup $tools.rustup -Name 'rustc'
    }
    $msrv['llvm'] = Resolve-MsrvLlvmIdentities -Rustc $msrv.rustc

    return [ordered]@{
        schema = 'rayman.release.binding.v3'
        workspace = (Resolve-Path -LiteralPath $repoRoot).ProviderPath
        head = $head
        clean = $true
        workspace_activation = $sourceFreshInputs.workspace_activation
        source_fresh_environment = $sourceFreshInputs.source_fresh_environment
        cli = [ordered]@{ path = $resolvedCli; sha256 = Get-Sha256 $resolvedCli }
        skill = [ordered]@{ path = $resolvedSkill; sha256 = Get-Sha256 $resolvedSkill }
        scripts = $scripts
        tools = $tools
        msrv = $msrv
        advisory_database = Get-AdvisoryDatabaseBinding
        cargo_net_offline = Get-CargoOfflineBinding
    }
}

function Test-ReusableEvidence {
    param([Parameter(Mandatory = $true)]$Evidence, [Parameter(Mandatory = $true)]$Binding)

    try {
        foreach ($candidate in @($Evidence.binding, $Binding)) {
            if ($null -eq $candidate -or
                $candidate.schema -ne 'rayman.release.binding.v3' -or
                $candidate.workspace_activation.schema -ne 'rayman.workspace-activation.snapshot.v1' -or
                $candidate.workspace_activation.path -notmatch '[\\/]\.RaymanCodingSkill[\\/]workspace_skill\.yaml$' -or
                $candidate.workspace_activation.sha256 -notmatch '^[0-9a-f]{64}$' -or
                $candidate.source_fresh_environment.schema -ne 'rayman.source-fresh.environment.v1' -or
                $candidate.source_fresh_environment.policy.schema -ne 'rayman.source-fresh.environment-policy.v1' -or
                $candidate.source_fresh_environment.policy_sha256 -notmatch '^[0-9a-f]{64}$' -or
                $candidate.source_fresh_environment.policy_sha256 -ne (Get-ObjectSha256 $candidate.source_fresh_environment.policy) -or
                $candidate.source_fresh_environment.clear -ne $true -or
                @($candidate.source_fresh_environment.rejected_names).Count -ne 0 -or
                $candidate.cargo_net_offline.effective -ne $true) {
                return $false
            }
        }
        if ($Evidence.schema -ne 'rayman.release.evidence.v1' -or
            $Evidence.status -ne 'pass') {
            return $false
        }
    } catch {
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

function Get-AuditArguments {
    param(
        [Parameter(Mandatory = $true)][string]$Cli,
        [Parameter(Mandatory = $true)][string]$Skill
    )

    $arguments = [ordered]@{
        CliPath = $Cli
        SkillPath = $Skill
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
        # audit-repository.ps1 runs the source-fresh verifier internally; the
        # exact terminal release binding below is the consumer boundary.
        source_fresh = 'scripts/audit-repository.ps1 -> verify-release-contract.ps1 -RequireSourceFresh'
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

if ($PSCmdlet.ParameterSetName -eq 'SelfTest') {
    # Keep this self-test runnable before CI provisions workspace activation.
    # The normal Get-ReleaseBinding path always calls the real inspector.
    $selfTestEnvironmentPolicy = [ordered]@{
        schema = 'rayman.source-fresh.environment-policy.v1'
        exact_names = [string[]]@('SELF_TEST_EXACT')
        name_patterns = [string[]]@('^SELF_TEST_PATTERN_')
    }
    $selfTestSourceFreshInputs = [ordered]@{
        workspace_activation = [ordered]@{
            schema = 'rayman.workspace-activation.snapshot.v1'
            path = Join-Path $repoRoot '.RaymanCodingSkill/workspace_skill.yaml'
            sha256 = ('9' * 64)
        }
        source_fresh_environment = [ordered]@{
            schema = 'rayman.source-fresh.environment.v1'
            policy = $selfTestEnvironmentPolicy
            policy_sha256 = Get-ObjectSha256 $selfTestEnvironmentPolicy
            clear = $true
            rejected_names = [string[]]@()
        }
    }
    $binding = [ordered]@{
        schema = 'rayman.release.binding.v3'
        workspace = 'repository'
        head = ('a' * 40)
        clean = $true
        workspace_activation = $selfTestSourceFreshInputs.workspace_activation
        source_fresh_environment = $selfTestSourceFreshInputs.source_fresh_environment
        cli = [ordered]@{ path = 'rayman'; sha256 = ('b' * 64) }
        skill = [ordered]@{ path = 'SKILL.md'; sha256 = ('c' * 64) }
        scripts = [ordered]@{ audit = ('c' * 64) }
        tools = [ordered]@{
            cargo = [ordered]@{ path = 'cargo'; sha256 = ('d' * 64); version = 'cargo 1.97.1' }
            'cargo-deny' = [ordered]@{ path = 'cargo-deny'; sha256 = ('e' * 64); version = 'cargo-deny 0.19.8' }
            'cargo-llvm-cov' = [ordered]@{ path = 'cargo-llvm-cov'; sha256 = ('f' * 64); version = 'cargo-llvm-cov 0.8.7' }
        }
        msrv = [ordered]@{
            toolchain = '1.97.1'
            cargo = [ordered]@{ path = 'msrv-cargo'; sha256 = ('1' * 64); version = 'cargo 1.97.1' }
            rustc = [ordered]@{ path = 'msrv-rustc'; sha256 = ('2' * 64); version = 'rustc 1.97.1' }
            llvm = [ordered]@{
                'llvm-cov' = [ordered]@{ path = 'llvm-cov'; sha256 = ('3' * 64); version = 'LLVM version 22.1.6-rust-1.97.1-stable' }
                'llvm-profdata' = [ordered]@{ path = 'llvm-profdata'; sha256 = ('4' * 64); version = 'LLVM version 22.1.6-rust-1.97.1-stable' }
            }
        }
        advisory_database = [ordered]@{
            path = 'advisory-dbs'
            state = 'present'
            sha256 = ('5' * 64)
            entries = 4
        }
        cargo_net_offline = [ordered]@{ present = $true; raw = 'true'; effective = $true }
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
    foreach ($legacySchema in @('rayman.release.binding.v1', 'rayman.release.binding.v2')) {
        $legacyBinding = $binding | ConvertTo-Json -Depth 20 | ConvertFrom-Json -Depth 20
        $legacyBinding.schema = $legacySchema
        $legacyEvidence = [pscustomobject]@{
            schema = 'rayman.release.evidence.v1'
            status = 'pass'
            binding_sha256 = Get-ObjectSha256 $legacyBinding
            binding = $legacyBinding
        }
        if (Test-ReusableEvidence $legacyEvidence $legacyBinding) {
            throw "release closeout self-test reused legacy $legacySchema evidence."
        }
    }
    $drifted = $binding | ConvertTo-Json -Depth 20 | ConvertFrom-Json -Depth 20
    $drifted.head = 'e' * 40
    if (Test-ReusableEvidence $evidence $drifted) {
        throw 'release closeout self-test reused evidence across HEAD drift.'
    }
    foreach ($case in @(
        @{ Label = 'MSRV Cargo hash'; Mutate = { param($copy) $copy.msrv.cargo.sha256 = ('6' * 64) } },
        @{ Label = 'MSRV rustc version'; Mutate = { param($copy) $copy.msrv.rustc.version = 'rustc 1.97.2' } },
        @{ Label = 'LLVM cov hash'; Mutate = { param($copy) $copy.msrv.llvm.'llvm-cov'.sha256 = ('7' * 64) } },
        @{ Label = 'LLVM profdata version'; Mutate = { param($copy) $copy.msrv.llvm.'llvm-profdata'.version = 'LLVM drift' } },
        @{ Label = 'advisory database digest'; Mutate = { param($copy) $copy.advisory_database.sha256 = ('8' * 64) } },
        @{ Label = 'CARGO_NET_OFFLINE state'; Mutate = { param($copy) $copy.cargo_net_offline.raw = 'false'; $copy.cargo_net_offline.effective = $false } },
        @{ Label = 'coverage version'; Mutate = { param($copy) $copy.tools.'cargo-llvm-cov'.version = 'cargo-llvm-cov 0.8.6' } },
        @{ Label = 'workspace activation path'; Mutate = { param($copy) $copy.workspace_activation.path += '.drifted' } },
        @{ Label = 'workspace activation hash'; Mutate = { param($copy) $copy.workspace_activation.sha256 = ('0' * 64) } },
        @{ Label = 'source-fresh environment policy hash'; Mutate = { param($copy) $copy.source_fresh_environment.policy_sha256 = ('0' * 64) } },
        @{
            Label = 'source-fresh environment policy'
            Mutate = {
                param($copy)
                $copy.source_fresh_environment.policy.exact_names += 'SELF_TEST_OVERRIDE'
                $copy.source_fresh_environment.policy_sha256 =
                    Get-ObjectSha256 $copy.source_fresh_environment.policy
            }
        },
        @{ Label = 'source-fresh environment clear state'; Mutate = { param($copy) $copy.source_fresh_environment.clear = $false; $copy.source_fresh_environment.rejected_names = @('SELF_TEST_OVERRIDE') } }
    )) {
        $changed = $binding | ConvertTo-Json -Depth 20 | ConvertFrom-Json -Depth 20
        & $case.Mutate $changed
        if (Test-ReusableEvidence $evidence $changed) {
            throw "release closeout self-test reused evidence across $($case.Label) drift."
        }
    }
    $onlineBinding = $binding | ConvertTo-Json -Depth 20 | ConvertFrom-Json -Depth 20
    $onlineBinding.cargo_net_offline.present = $false
    $onlineBinding.cargo_net_offline.raw = $null
    $onlineBinding.cargo_net_offline.effective = $false
    $onlineEvidence = [pscustomobject]@{
        schema = 'rayman.release.evidence.v1'
        status = 'pass'
        binding_sha256 = Get-ObjectSha256 $onlineBinding
        binding = $onlineBinding
    }
    if (Test-ReusableEvidence $onlineEvidence $onlineBinding) {
        throw 'release closeout self-test reused online advisory evidence whose database may be refreshed.'
    }
    $auditArguments = Get-AuditArguments 'rayman' 'SKILL.md'
    if ($auditArguments.Contains('PrepareAuditTools')) {
        throw 'release closeout self-test allowed implicit audit-tool provisioning.'
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
    $binding = Get-ReleaseBinding `
        -Cli $CliPath `
        -Skill $SkillPath
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
        $reuseTerminalBinding = Get-ReleaseBinding `
            -Cli $CliPath `
            -Skill $SkillPath
        if ((Get-ObjectSha256 $reuseTerminalBinding) -ne (Get-ObjectSha256 $binding) -or
            -not (Test-ReusableEvidence $existing $reuseTerminalBinding)) {
            throw 'Release binding drifted while revalidating reusable evidence; reuse was not accepted.'
        }
        Write-Output 'RAYMAN_RELEASE_PHASE {"phase":"audit_and_source_fresh","status":"reused_exact_binding"}'
    } else {
        Write-Output 'RAYMAN_RELEASE_PHASE {"phase":"audit_and_source_fresh","status":"start"}'
        $auditArguments = Get-AuditArguments `
            -Cli $CliPath `
            -Skill $SkillPath
        & (Join-Path $PSScriptRoot 'audit-repository.ps1') @auditArguments
        # The audit's installed_release_identity phase already performs the
        # clean isolated -RequireSourceFresh rebuild. Recomputing the exact
        # binding here detects any post-audit drift without a second rebuild.
        $after = Get-ReleaseBinding `
            -Cli $CliPath `
            -Skill $SkillPath
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
    $terminalBinding = Get-ReleaseBinding `
        -Cli $CliPath `
        -Skill $SkillPath
    if ((Get-ObjectSha256 $terminalBinding) -ne (Get-ObjectSha256 $binding)) {
        throw 'Release binding drifted before closeout completion; no completion claim is valid.'
    }
} finally {
    Pop-Location
}
