[CmdletBinding(DefaultParameterSetName = 'Plan')]
param(
    [Parameter(ParameterSetName = 'Plan')][switch]$Plan,
    [Parameter(Mandatory, ParameterSetName = 'Prepare')][switch]$Prepare,
    [Parameter(Mandatory, ParameterSetName = 'Prepare')][switch]$Yes,
    [Parameter(Mandatory, ParameterSetName = 'SelfTest')][switch]$SelfTest,
    [string]$ConfigPath = (Join-Path $env:USERPROFILE '.codex\config.toml'),
    [string]$OwnerAccount = 'qinrm'
)

$aclMode = $PSCmdlet.ParameterSetName
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if (-not $IsWindows -or $PSVersionTable.PSVersion.Major -lt 7) {
    throw 'Native Windows PowerShell 7 is required.'
}
. (Join-Path $PSScriptRoot 'configure-codex-validation-temp.ps1') -Library

function Get-AccountSid([string]$Account) {
    try {
        return ([Security.Principal.NTAccount]::new($Account)).Translate(
            [Security.Principal.SecurityIdentifier]
        ).Value
    } catch {
        throw "Cannot resolve the fixed owner account '$Account'."
    }
}

function Get-RuleSid($Rule) {
    try {
        return $Rule.IdentityReference.Translate(
            [Security.Principal.SecurityIdentifier]
        ).Value
    } catch {
        return $null
    }
}

function Test-HasChangePermissions($Acl, [string]$Sid) {
    foreach ($rule in $Acl.Access) {
        if ((Get-RuleSid $rule) -cne $Sid -or
            $rule.AccessControlType -ne [Security.AccessControl.AccessControlType]::Allow -or
            $rule.IsInherited) {
            continue
        }
        if (($rule.FileSystemRights -band [Security.AccessControl.FileSystemRights]::ChangePermissions) -ne 0) {
            return $true
        }
    }
    return $false
}

function Add-ExactOwnerAclPreparation([string]$Path, [string]$OwnerSid) {
    $acl = Get-Acl -LiteralPath $Path -ErrorAction Stop
    $before = $acl.Sddl
    if (Test-HasChangePermissions $acl $OwnerSid) {
        return [ordered]@{ changed = $false; before_sddl = $before; after_sddl = $before }
    }
    $rule = [Security.AccessControl.FileSystemAccessRule]::new(
        [Security.Principal.SecurityIdentifier]::new($OwnerSid),
        [Security.AccessControl.FileSystemRights]::ChangePermissions,
        [Security.AccessControl.InheritanceFlags]::None,
        [Security.AccessControl.PropagationFlags]::None,
        [Security.AccessControl.AccessControlType]::Allow
    )
    $acl.AddAccessRule($rule)
    Set-Acl -LiteralPath $Path -AclObject $acl
    $after = Get-Acl -LiteralPath $Path -ErrorAction Stop
    if (-not (Test-HasChangePermissions $after $OwnerSid)) {
        throw "Exact owner ACL preparation did not persist: $Path"
    }
    return [ordered]@{ changed = $true; before_sddl = $before; after_sddl = $after.Sddl }
}

function Assert-OrdinaryPath([string]$Path) {
    $cursor = [IO.Path]::GetFullPath($Path)
    while ($cursor) {
        if (Test-Path -LiteralPath $cursor) {
            $item = Get-Item -LiteralPath $cursor -Force
            if ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) {
                throw "Reparse project path refused: $cursor"
            }
        }
        $parent = [IO.Path]::GetDirectoryName($cursor)
        if ($parent -eq $cursor) { break }
        $cursor = $parent
    }
}

function Get-TrustedProjects([string]$Path) {
    $document = Read-StrictUtf8Document $Path
    $lines = ConvertTo-TomlLines $document.Text
    $structure = Get-TomlStructure $lines
    $projects = [Collections.Generic.List[string]]::new()
    foreach ($header in $structure.Headers) {
        if ($header.IsArray -or $header.Path.Count -ne 2 -or $header.Path[0] -cne 'projects') {
            continue
        }
        $next = @($structure.Headers | Where-Object LineIndex -gt $header.LineIndex |
            Sort-Object LineIndex | Select-Object -First 1)
        $end = if ($next.Count) { [int]$next[0].LineIndex } else { $lines.Count }
        $trust = [Collections.Generic.List[string]]::new()
        for ($index = $header.LineIndex + 1; $index -lt $end; $index++) {
            $code = (Split-TomlComment ([string]$lines[$index].Content)).Code
            if ([string]::IsNullOrWhiteSpace($code)) { continue }
            $equals = Find-TomlEquals $code
            if ($equals -lt 1) { continue }
            $keys = @(ConvertFrom-TomlKeyPath $code.Substring(0, $equals).Trim())
            if ($keys.Count -eq 1 -and $keys[0] -ceq 'trust_level') {
                $trust.Add((ConvertFrom-TomlStringValue $code.Substring($equals + 1).Trim()))
            }
        }
        if ($trust.Count -eq 1 -and $trust[0] -ceq 'trusted') {
            $candidate = [IO.Path]::GetFullPath($header.Path[1])
            if (Test-Path -LiteralPath $candidate -PathType Container) {
                Assert-OrdinaryPath $candidate
                $projects.Add($candidate)
            }
        }
    }
    return @($projects | Sort-Object -Unique)
}

function Get-AclState([string]$Workspace, [string]$OwnerSid) {
    $workspaceAcl = Get-Acl -LiteralPath $Workspace -ErrorAction Stop
    $workspaceOwnerSid = Get-AccountSid $workspaceAcl.Owner
    $workspaceSandboxOwner = $workspaceAcl.Owner -match '\\CodexSandbox(?:Offline|Online)$'
    $workspacePrepared = Test-HasChangePermissions $workspaceAcl $OwnerSid
    $currentSid = [Security.Principal.WindowsIdentity]::GetCurrent().User.Value
    if ($workspaceOwnerSid -cne $OwnerSid -and -not $workspaceSandboxOwner) {
        return [ordered]@{
            workspace = $Workspace; git_path = (Join-Path $Workspace '.git')
            state = 'blocked_foreign_owner'; workspace_owner = $workspaceAcl.Owner
            git_owner = $null; prepare_paths = @(); current_principal_can_prepare = $false
        }
    }
    $marker = Join-Path $Workspace '.git'
    if (-not (Test-Path -LiteralPath $marker)) {
        $state = if ($workspaceOwnerSid -eq $OwnerSid) { 'no_git' } elseif ($workspacePrepared) { 'prepared' } else { 'needs_prepare' }
        return [ordered]@{
            workspace = $Workspace; git_path = $marker; state = $state
            workspace_owner = $workspaceAcl.Owner; git_owner = $null
            prepare_paths = if ($state -eq 'needs_prepare') { @($Workspace) } else { @() }
            current_principal_can_prepare = ($state -eq 'needs_prepare' -and $workspaceOwnerSid -ceq $currentSid)
        }
    }
    $item = Get-Item -LiteralPath $marker -Force
    if (-not $item.PSIsContainer) {
        $state = if ($workspaceOwnerSid -eq $OwnerSid) { 'linked_worktree' } elseif ($workspacePrepared) { 'prepared' } else { 'needs_prepare' }
        return [ordered]@{
            workspace = $Workspace; git_path = $marker; state = $state
            workspace_owner = $workspaceAcl.Owner; git_owner = $null
            prepare_paths = if ($state -eq 'needs_prepare') { @($Workspace) } else { @() }
            current_principal_can_prepare = ($state -eq 'needs_prepare' -and $workspaceOwnerSid -ceq $currentSid)
        }
    }
    if ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) {
        return [ordered]@{ workspace = $Workspace; git_path = $marker; state = 'blocked_reparse'; workspace_owner = $workspaceAcl.Owner; git_owner = $null; prepare_paths = @(); current_principal_can_prepare = $false }
    }
    $acl = Get-Acl -LiteralPath $marker -ErrorAction Stop
    $gitOwnerSid = Get-AccountSid $acl.Owner
    $gitSandboxOwner = $acl.Owner -match '\\CodexSandbox(?:Offline|Online)$'
    $gitPrepared = Test-HasChangePermissions $acl $OwnerSid
    if ($gitOwnerSid -cne $OwnerSid -and -not $gitSandboxOwner) {
        return [ordered]@{
            workspace = $Workspace; git_path = $marker; state = 'blocked_foreign_owner'
            workspace_owner = $workspaceAcl.Owner; git_owner = $acl.Owner
            prepare_paths = @(); current_principal_can_prepare = $false
        }
    }
    $preparePaths = [Collections.Generic.List[string]]::new()
    if ($workspaceOwnerSid -cne $OwnerSid -and -not $workspacePrepared) { $preparePaths.Add($Workspace) }
    if ($gitOwnerSid -cne $OwnerSid -and -not $gitPrepared) { $preparePaths.Add($marker) }
    $state = if ($preparePaths.Count) {
        'needs_prepare'
    } elseif ($workspaceOwnerSid -cne $OwnerSid -or $gitOwnerSid -cne $OwnerSid) {
        'prepared'
    } else {
        'owner_ready'
    }
    $canPrepare = $state -eq 'needs_prepare'
    if ($preparePaths -contains $Workspace -and $workspaceOwnerSid -cne $currentSid) { $canPrepare = $false }
    if ($preparePaths -contains $marker -and $gitOwnerSid -cne $currentSid) { $canPrepare = $false }
    return [ordered]@{
        workspace = $Workspace
        git_path = $marker
        state = $state
        workspace_owner = $workspaceAcl.Owner
        git_owner = $acl.Owner
        prepare_paths = @($preparePaths)
        current_principal_can_prepare = $canPrepare
    }
}

function Get-AclPlan([string]$Path, [string]$Account) {
    $ownerSid = Get-AccountSid $Account
    $rows = [Collections.Generic.List[object]]::new()
    foreach ($workspace in Get-TrustedProjects $Path) {
        $rows.Add([pscustomobject](Get-AclState $workspace $ownerSid))
    }
    $needs = @($rows | Where-Object state -eq 'needs_prepare')
    $blocked = @($rows | Where-Object state -like 'blocked_*')
    return [ordered]@{
        schema = 'rayman.codex-workspace-acl-plan.v1'
        owner_account = $Account
        owner_sid = $ownerSid
        project_principal = [Security.Principal.WindowsIdentity]::GetCurrent().Name
        project_count = $rows.Count
        needs_prepare_count = $needs.Count
        blocked_count = $blocked.Count
        safe = ($needs.Count -eq 0 -and $blocked.Count -eq 0)
        projects = $rows
    }
}

function Invoke-SelfTest {
    $base = Join-Path ([IO.Path]::GetTempPath()) ('codex-workspace-acl-test-' + [Guid]::NewGuid().ToString('N'))
    $git = Join-Path $base '.git'
    try {
        [void][IO.Directory]::CreateDirectory($git)
        $before = Get-Acl -LiteralPath $git
        $beforeOwner = $before.Owner
        $targetSid = 'S-1-5-32-545'
        $first = Add-ExactOwnerAclPreparation $git $targetSid
        if (-not $first.changed) { throw 'First ACL preparation made no change.' }
        $after = Get-Acl -LiteralPath $git
        if ($after.Owner -cne $beforeOwner -or -not (Test-HasChangePermissions $after $targetSid)) {
            throw 'ACL preparation changed owner or omitted the exact right.'
        }
        $rules = @($after.Access | Where-Object {
            (Get-RuleSid $_) -ceq $targetSid -and -not $_.IsInherited -and
            ($_.FileSystemRights -band [Security.AccessControl.FileSystemRights]::ChangePermissions) -ne 0
        })
        if ($rules.Count -ne 1 -or $rules[0].InheritanceFlags -ne [Security.AccessControl.InheritanceFlags]::None) {
            throw 'ACL preparation was duplicated or inherited.'
        }
        $second = Add-ExactOwnerAclPreparation $git $targetSid
        if ($second.changed -or $second.after_sddl -cne $first.after_sddl) {
            throw 'ACL preparation is not idempotent.'
        }
        Write-Output 'repair-codex-workspace-acl: PASS (exact non-recursive idempotent ACE)'
    } finally {
        if (Test-Path -LiteralPath $base) { Remove-Item -LiteralPath $base -Recurse -Force }
    }
}

if ($aclMode -eq 'SelfTest') {
    Invoke-SelfTest
    return
}

$aclPlan = Get-AclPlan $ConfigPath $OwnerAccount
if ($aclMode -eq 'Plan') {
    $aclPlan | ConvertTo-Json -Depth 8
    return
}

$changed = [Collections.Generic.List[object]]::new()
foreach ($project in @($aclPlan.projects | Where-Object state -eq 'needs_prepare')) {
    if (-not $project.current_principal_can_prepare) { continue }
    foreach ($path in @($project.prepare_paths)) {
        $result = Add-ExactOwnerAclPreparation $path $aclPlan.owner_sid
        $changed.Add([pscustomobject]@{
            workspace = $project.workspace
            path = $path
            changed = $result.changed
            recursive = $false
        })
    }
}
$verified = Get-AclPlan $ConfigPath $OwnerAccount
[ordered]@{
    schema = 'rayman.codex-workspace-acl-result.v1'
    changed_count = @($changed | Where-Object changed).Count
    changed = $changed
    needs_prepare_count = $verified.needs_prepare_count
    blocked_count = $verified.blocked_count
    safe = $verified.safe
    projects = $verified.projects
} | ConvertTo-Json -Depth 8
