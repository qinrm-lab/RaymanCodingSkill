[CmdletBinding(DefaultParameterSetName = 'Check')]
param(
    [Parameter(Mandatory = $true, ParameterSetName = 'Check')]
    [switch]$Check,
    [Parameter(Mandatory = $true, ParameterSetName = 'Install')]
    [switch]$Install,
    [Parameter(Mandatory = $true, ParameterSetName = 'Uninstall')]
    [switch]$Uninstall,
    [Parameter(Mandatory = $true, ParameterSetName = 'RecoverPartialUninstall')]
    [switch]$RecoverPartialUninstall,
    [Parameter(Mandatory = $true, ParameterSetName = 'SelfTest')]
    [switch]$SelfTest,
    [Parameter(Mandatory = $true, ParameterSetName = 'Install')]
    [Parameter(Mandatory = $true, ParameterSetName = 'Uninstall')]
    [Parameter(Mandatory = $true, ParameterSetName = 'RecoverPartialUninstall')]
    [switch]$Yes,
    [string]$InstallRoot = (Join-Path `
        ([Environment]::GetFolderPath('CommonApplicationData')) `
        'Rayman\CodexPowerShellBroker'),
    [string]$RequestRoot = (Join-Path `
        ([Environment]::GetFolderPath('CommonApplicationData')) `
        'Rayman\CodexPowerShellBroker\requests'),
    [string]$TaskName = '\Rayman-CodexPowerShellBroker',
    [string]$UserAccount = "$env:USERDOMAIN\$env:USERNAME",
    [string]$SandboxGroup = "$env:COMPUTERNAME\CodexSandboxUsers"
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

if ($PSVersionTable.PSVersion.Major -lt 7) {
    throw 'install-codex-powershell-broker.ps1 requires PowerShell 7+.'
}

$script:SchemaVersion = 2
$script:BrokerSource = Join-Path $PSScriptRoot 'codex-powershell-broker.ps1'
$script:ReceiptName = 'install-receipt.json'
$script:HeartbeatName = 'heartbeat.json'
$script:TaskDescription = 'Rayman fixed-capability Codex PowerShell identity broker'

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

function Get-CurrentPowerShellRuntime {
    $path = [Diagnostics.Process]::GetCurrentProcess().MainModule.FileName
    $full = Get-NormalizedAbsolutePath -Path $path -Label 'PowerShell runtime'
    $item = Get-Item -LiteralPath $full -Force -ErrorAction Stop
    if ($item.PSIsContainer -or
        ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0 -or
        $item.Name -ine 'pwsh.exe') {
        throw "PowerShell runtime must be the current ordinary pwsh.exe: $full"
    }
    return [pscustomobject]@{
        Path = $item.FullName
        Sha256 = Get-FileSha256 -Path $item.FullName
    }
}

function Get-NormalizedAbsolutePath {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$Label
    )
    if (-not [IO.Path]::IsPathRooted($Path)) { throw "$Label must be absolute: $Path" }
    $full = [IO.Path]::GetFullPath($Path).TrimEnd('\', '/')
    if ($full -eq [IO.Path]::GetPathRoot($full)) {
        throw "$Label must not be a volume root: $full"
    }
    return $full
}

function Assert-ChildPath {
    param(
        [Parameter(Mandatory = $true)][string]$Child,
        [Parameter(Mandatory = $true)][string]$Parent,
        [Parameter(Mandatory = $true)][string]$Label
    )
    $childFull = [IO.Path]::GetFullPath($Child)
    $parentFull = [IO.Path]::GetFullPath($Parent).TrimEnd('\', '/')
    if (-not $childFull.StartsWith(
        $parentFull + [IO.Path]::DirectorySeparatorChar,
        [StringComparison]::OrdinalIgnoreCase
    )) { throw "$Label escaped its authority root: $childFull" }
    return $childFull
}

function Assert-RealDirectory {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$Label
    )
    $full = Get-NormalizedAbsolutePath -Path $Path -Label $Label
    if (-not (Test-Path -LiteralPath $full -PathType Container)) {
        throw "$Label is missing: $full"
    }
    $root = [IO.Path]::GetPathRoot($full)
    $segments = @($full.Substring($root.Length).Split(
        [char[]]@(
            [IO.Path]::DirectorySeparatorChar,
            [IO.Path]::AltDirectorySeparatorChar
        ),
        [StringSplitOptions]::RemoveEmptyEntries
    ))
    $current = $root
    foreach ($segment in $segments) {
        $current = Join-Path $current $segment
        $item = Get-Item -LiteralPath $current -Force -ErrorAction Stop
        if (-not $item.PSIsContainer -or
            ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
            throw "$Label must use real directories only: $current"
        }
    }
    return (Resolve-Path -LiteralPath $full).ProviderPath
}

function Resolve-AccountSid {
    param(
        [Parameter(Mandatory = $true)][string]$Account,
        [Parameter(Mandatory = $true)][string]$Label
    )
    try {
        return [string]([Security.Principal.NTAccount]::new($Account).Translate(
            [Security.Principal.SecurityIdentifier]
        ).Value)
    } catch {
        throw "$Label cannot be resolved to a SID: $Account ($($_.Exception.Message))"
    }
}

function Get-CurrentPrincipal {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = [Security.Principal.WindowsPrincipal]::new($identity)
    return [pscustomobject]@{
        Account = [string]$identity.Name
        Sid = [string]$identity.User.Value
        IsAdministrator = $principal.IsInRole(
            [Security.Principal.WindowsBuiltInRole]::Administrator
        )
    }
}

function Assert-InstallAuthority {
    param(
        [Parameter(Mandatory = $true)][string]$ExpectedAccount,
        [Parameter(Mandatory = $true)][string]$ExpectedSid
    )
    $current = Get-CurrentPrincipal
    if ($current.Account -cne $ExpectedAccount -or $current.Sid -cne $ExpectedSid) {
        throw "Installation must run as the exact target user. Expected=$ExpectedAccount/$ExpectedSid Actual=$($current.Account)/$($current.Sid)"
    }
    if (-not $current.IsAdministrator) {
        throw 'Installation requires one administrator-approved run for ProgramData, ACL, and Task Scheduler publication.'
    }
    return $current
}

function Add-ManagedRule {
    param(
        [Parameter(Mandatory = $true)]$Security,
        [Parameter(Mandatory = $true)][string]$Sid,
        [Parameter(Mandatory = $true)][Security.AccessControl.FileSystemRights]$Rights,
        [Security.AccessControl.InheritanceFlags]$Inheritance =
            [Security.AccessControl.InheritanceFlags]::None,
        [Security.AccessControl.PropagationFlags]$Propagation =
            [Security.AccessControl.PropagationFlags]::None
    )
    $rule = [Security.AccessControl.FileSystemAccessRule]::new(
        [Security.Principal.SecurityIdentifier]::new($Sid),
        $Rights,
        $Inheritance,
        $Propagation,
        [Security.AccessControl.AccessControlType]::Allow
    )
    [void]$Security.AddAccessRule($rule)
}

function New-ManagedDirectorySecurity {
    param(
        [Parameter(Mandatory = $true)][string]$UserSid,
        [Parameter(Mandatory = $true)][string]$SandboxSid,
        [Parameter(Mandatory = $true)]
        [ValidateSet('ReadOnly', 'Requests')]
        [string]$Kind
    )
    $security = [Security.AccessControl.DirectorySecurity]::new()
    $security.SetAccessRuleProtection($true, $false)
    $security.SetOwner([Security.Principal.SecurityIdentifier]::new($UserSid))
    $inherit = [Security.AccessControl.InheritanceFlags]'ContainerInherit, ObjectInherit'
    foreach ($sid in @('S-1-5-18', 'S-1-5-32-544', $UserSid)) {
        Add-ManagedRule -Security $security -Sid $sid `
            -Rights ([Security.AccessControl.FileSystemRights]::FullControl) `
            -Inheritance $inherit
    }
    if ($Kind -eq 'ReadOnly') {
        Add-ManagedRule -Security $security -Sid $SandboxSid `
            -Rights ([Security.AccessControl.FileSystemRights]::ReadAndExecute) `
            -Inheritance $inherit
    } else {
        $folderRights = [Security.AccessControl.FileSystemRights]::ReadAndExecute -bor `
            [Security.AccessControl.FileSystemRights]::WriteData
        Add-ManagedRule -Security $security -Sid $SandboxSid -Rights $folderRights
        Add-ManagedRule -Security $security -Sid $SandboxSid `
            -Rights ([Security.AccessControl.FileSystemRights]::Modify) `
            -Inheritance $inherit `
            -Propagation ([Security.AccessControl.PropagationFlags]::InheritOnly)
    }
    return $security
}

function New-ManagedFileSecurity {
    param(
        [Parameter(Mandatory = $true)][string]$UserSid,
        [Parameter(Mandatory = $true)][string]$SandboxSid
    )
    $security = [Security.AccessControl.FileSecurity]::new()
    $security.SetAccessRuleProtection($true, $false)
    $security.SetOwner([Security.Principal.SecurityIdentifier]::new($UserSid))
    foreach ($sid in @('S-1-5-18', 'S-1-5-32-544', $UserSid)) {
        Add-ManagedRule -Security $security -Sid $sid `
            -Rights ([Security.AccessControl.FileSystemRights]::FullControl)
    }
    Add-ManagedRule -Security $security -Sid $SandboxSid `
        -Rights ([Security.AccessControl.FileSystemRights]::ReadAndExecute)
    return $security
}

function Assert-ExactSecurity {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)]$Expected,
        [Parameter(Mandatory = $true)][string]$ExpectedOwnerSid,
        [Parameter(Mandatory = $true)][string]$Label
    )
    $actual = Get-Acl -LiteralPath $Path -ErrorAction Stop
    $actualOwner = [string]([Security.Principal.NTAccount]::new(
        [string]$actual.Owner
    ).Translate([Security.Principal.SecurityIdentifier]).Value)
    $actualAccess = $actual.GetSecurityDescriptorSddlForm(
        [Security.AccessControl.AccessControlSections]::Access
    )
    $expectedAccess = $Expected.GetSecurityDescriptorSddlForm(
        [Security.AccessControl.AccessControlSections]::Access
    )
    if (-not $actual.AreAccessRulesProtected -or
        $actualOwner -cne $ExpectedOwnerSid -or
        $actualAccess -cne $expectedAccess) {
        throw "$Label owner/DACL mismatch. ExpectedOwner=$ExpectedOwnerSid ActualOwner=$actualOwner ExpectedDacl=$expectedAccess ActualDacl=$actualAccess"
    }
}

function Test-ExactAllowRule {
    param(
        [Parameter(Mandatory = $true)]$Rule,
        [Parameter(Mandatory = $true)]
        [Security.AccessControl.FileSystemRights]$Rights,
        [Security.AccessControl.InheritanceFlags]$Inheritance =
            [Security.AccessControl.InheritanceFlags]::None,
        [Security.AccessControl.PropagationFlags]$Propagation =
            [Security.AccessControl.PropagationFlags]::None
    )

    $effectiveRights = $Rights -bor `
        [Security.AccessControl.FileSystemRights]::Synchronize
    return (-not $Rule.IsInherited -and
        $Rule.AccessControlType -eq
            [Security.AccessControl.AccessControlType]::Allow -and
        [int]$Rule.FileSystemRights -eq [int]$effectiveRights -and
        $Rule.InheritanceFlags -eq $Inheritance -and
        $Rule.PropagationFlags -eq $Propagation)
}

function Test-CodexSandboxCapabilitySid {
    param(
        [Parameter(Mandatory = $true)]
        [Security.Principal.SecurityIdentifier]$Sid,
        [Parameter(Mandatory = $true)][string]$OwnerSid
    )

    if (-not $Sid.IsAccountSid()) { return $false }
    $owner = [Security.Principal.SecurityIdentifier]::new($OwnerSid)
    $candidateDomain = $Sid.AccountDomainSid
    $ownerDomain = $owner.AccountDomainSid
    if ($null -eq $candidateDomain -or $null -eq $ownerDomain -or
        $candidateDomain.Value -ceq $ownerDomain.Value) {
        return $false
    }
    try {
        [void]$Sid.Translate([Security.Principal.NTAccount])
        return $false
    } catch [Security.Principal.IdentityNotMappedException] {
        return $true
    } catch {
        return $false
    }
}

function Assert-RequestSecurityDescriptorContract {
    param(
        [Parameter(Mandatory = $true)]
        [Security.AccessControl.DirectorySecurity]$Security,
        [Parameter(Mandatory = $true)][string]$ExpectedOwnerSid,
        [Parameter(Mandatory = $true)][string]$SandboxSid,
        [Parameter(Mandatory = $true)][string]$Label
    )

    $actualOwner = $Security.GetOwner(
        [Security.Principal.SecurityIdentifier]
    ).Value
    if (-not $Security.AreAccessRulesProtected -or
        -not $Security.AreAccessRulesCanonical -or
        $actualOwner -cne $ExpectedOwnerSid) {
        throw "$Label must retain its protected canonical owner/DACL."
    }

    $criticalCounts = @{}
    foreach ($sid in @('S-1-5-18', 'S-1-5-32-544', $ExpectedOwnerSid)) {
        $criticalCounts[$sid] = 0
    }
    $folderRights = [Security.AccessControl.FileSystemRights]::ReadAndExecute -bor `
        [Security.AccessControl.FileSystemRights]::WriteData
    $inherit = [Security.AccessControl.InheritanceFlags]'ContainerInherit, ObjectInherit'
    $sandboxFolder = 0
    $sandboxInheritedChildren = 0
    $sandboxManaged = 0
    $capabilityCount = 0
    $rules = @($Security.GetAccessRules(
        $true,
        $false,
        [Security.Principal.SecurityIdentifier]
    ))
    foreach ($rule in $rules) {
        $sidObject = [Security.Principal.SecurityIdentifier]$rule.IdentityReference
        $sid = $sidObject.Value
        if ($criticalCounts.ContainsKey($sid)) {
            if (-not (Test-ExactAllowRule -Rule $rule `
                -Rights ([Security.AccessControl.FileSystemRights]::FullControl) `
                -Inheritance $inherit)) {
                throw "$Label critical ACE drifted for $sid."
            }
            $criticalCounts[$sid] = [int]$criticalCounts[$sid] + 1
            continue
        }
        if ($sid -ceq $SandboxSid) {
            if (Test-ExactAllowRule -Rule $rule -Rights $folderRights) {
                $sandboxFolder++
                continue
            }
            if (Test-ExactAllowRule -Rule $rule `
                -Rights ([Security.AccessControl.FileSystemRights]::Modify) `
                -Inheritance $inherit `
                -Propagation ([Security.AccessControl.PropagationFlags]::InheritOnly)) {
                $sandboxInheritedChildren++
                continue
            }
            if (Test-ExactAllowRule -Rule $rule `
                -Rights ([Security.AccessControl.FileSystemRights]::Modify) `
                -Inheritance $inherit) {
                $sandboxManaged++
                continue
            }
            throw "$Label sandbox-group ACE drifted for $sid."
        }
        if (-not (Test-ExactAllowRule -Rule $rule `
            -Rights ([Security.AccessControl.FileSystemRights]::Modify) `
            -Inheritance $inherit) -or
            -not (Test-CodexSandboxCapabilitySid `
                -Sid $sidObject -OwnerSid $ExpectedOwnerSid)) {
            throw "$Label contains an unauthorized extra ACE for $sid."
        }
        $capabilityCount++
    }
    foreach ($sid in @($criticalCounts.Keys)) {
        if ([int]$criticalCounts[$sid] -ne 1) {
            throw "$Label requires exactly one critical ACE for $sid."
        }
    }
    $baseShape = ($sandboxFolder -eq 1 -and
        $sandboxInheritedChildren -eq 1 -and $sandboxManaged -eq 0 -and
        $capabilityCount -eq 0)
    $managedShape = ($sandboxFolder -eq 0 -and
        $sandboxInheritedChildren -eq 0 -and $sandboxManaged -eq 1 -and
        $capabilityCount -ge 1)
    if (-not ($baseShape -or $managedShape)) {
        throw "$Label sandbox-group ACE set is neither installer-base nor Codex-managed."
    }
}

function Assert-RequestRootSecurity {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$ExpectedOwnerSid,
        [Parameter(Mandatory = $true)][string]$SandboxSid,
        [Parameter(Mandatory = $true)][string]$Label
    )

    $actual = Get-Acl -LiteralPath $Path -ErrorAction Stop
    try {
        Assert-RequestSecurityDescriptorContract `
            -Security $actual -ExpectedOwnerSid $ExpectedOwnerSid `
            -SandboxSid $SandboxSid -Label $Label
    } catch {
        $actualAccess = $actual.GetSecurityDescriptorSddlForm(
            [Security.AccessControl.AccessControlSections]::Access
        )
        throw "$($_.Exception.Message) ActualDacl=$actualAccess"
    }
}

function New-ManagedDirectory {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][Security.AccessControl.DirectorySecurity]$Security,
        [Parameter(Mandatory = $true)][string]$OwnerSid,
        [Parameter(Mandatory = $true)][string]$Label
    )
    $full = Get-NormalizedAbsolutePath -Path $Path -Label $Label
    if (Test-Path -LiteralPath $full) {
        throw "$Label already exists; refuse to adopt a pre-created namespace: $full"
    }
    [void](Assert-RealDirectory -Path (Split-Path -Parent $full) -Label "$Label parent")
    [void][IO.FileSystemAclExtensions]::CreateDirectory($Security, $full)
    [void](Assert-RealDirectory -Path $full -Label $Label)
    Assert-ExactSecurity -Path $full -Expected $Security `
        -ExpectedOwnerSid $OwnerSid -Label $Label
    return $full
}

function Write-BytesAtomic {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][AllowEmptyCollection()][byte[]]$Bytes,
        [switch]$Replace,
        [Security.AccessControl.FileSecurity]$Security
    )
    $parent = Split-Path -Parent $Path
    $temporary = Join-Path $parent (
        '.' + [IO.Path]::GetFileName($Path) + '.stage-' + [Guid]::NewGuid().ToString('N')
    )
    $stream = if ($null -ne $Security) {
        [IO.FileSystemAclExtensions]::Create(
            [IO.FileInfo]::new($temporary),
            [IO.FileMode]::CreateNew,
            [Security.AccessControl.FileSystemRights]::FullControl,
            [IO.FileShare]::None,
            4096,
            [IO.FileOptions]::WriteThrough,
            $Security
        )
    } else {
        [IO.FileStream]::new(
            $temporary,
            [IO.FileMode]::CreateNew,
            [IO.FileAccess]::Write,
            [IO.FileShare]::None,
            4096,
            [IO.FileOptions]::WriteThrough
        )
    }
    try {
        $stream.Write($Bytes, 0, $Bytes.Length)
        $stream.Flush($true)
    } finally { $stream.Dispose() }
    try { [IO.File]::Move($temporary, $Path, [bool]$Replace) } catch {
        Remove-Item -LiteralPath $temporary -Force -ErrorAction SilentlyContinue
        throw
    }
}

function Write-JsonAtomic {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)]$Document,
        [switch]$Replace,
        [Security.AccessControl.FileSecurity]$Security
    )
    $json = $Document | ConvertTo-Json -Depth 12 -Compress
    Write-BytesAtomic `
        -Path $Path `
        -Bytes ([Text.UTF8Encoding]::new($false, $true).GetBytes($json)) `
        -Replace:$Replace -Security $Security
}

function ConvertTo-Utf16LeBom {
    param([Parameter(Mandatory = $true)][string]$Text)
    $body = [Text.UnicodeEncoding]::new($false, $false, $true).GetBytes($Text)
    $bytes = [byte[]]::new($body.Length + 2)
    $bytes[0] = 0xFF
    $bytes[1] = 0xFE
    [Array]::Copy($body, 0, $bytes, 2, $body.Length)
    return $bytes
}

function ConvertTo-XmlText {
    param([Parameter(Mandatory = $true)][string]$Text)
    return [Security.SecurityElement]::Escape($Text)
}

function Get-BrokerTaskArguments {
    param(
        [Parameter(Mandatory = $true)][string]$WorkerPath,
        [Parameter(Mandatory = $true)][string]$BrokerRoot,
        [Parameter(Mandatory = $true)][string]$Requests
    )
    return '-NoProfile -NonInteractive -WindowStyle Hidden ' +
        '-File "' + $WorkerPath + '" -Worker ' +
        '-InstallRoot "' + $BrokerRoot + '" ' +
        '-RequestRoot "' + $Requests + '" -PollMilliseconds 250'
}

function New-BrokerTaskXml {
    param(
        [Parameter(Mandatory = $true)][string]$PowerShellPath,
        [Parameter(Mandatory = $true)][string]$WorkerPath,
        [Parameter(Mandatory = $true)][string]$BrokerRoot,
        [Parameter(Mandatory = $true)][string]$Requests,
        [Parameter(Mandatory = $true)][string]$Account,
        [Parameter(Mandatory = $true)][string]$Name
    )
    if ($Name -notmatch '^\\[A-Za-z0-9._-]+$') {
        throw "Broker task name must be one fixed root task: $Name"
    }
    $arguments = Get-BrokerTaskArguments `
        -WorkerPath $WorkerPath -BrokerRoot $BrokerRoot -Requests $Requests
    $uri = if ($Name.StartsWith('\', [StringComparison]::Ordinal)) {
        $Name
    } else { '\' + $Name }
    return @"
<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.4" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Description>$(ConvertTo-XmlText $script:TaskDescription)</Description>
    <URI>$(ConvertTo-XmlText $uri)</URI>
  </RegistrationInfo>
  <Triggers><LogonTrigger><Enabled>true</Enabled><UserId>$(ConvertTo-XmlText $Account)</UserId></LogonTrigger></Triggers>
  <Principals><Principal id="Author"><UserId>$(ConvertTo-XmlText $Account)</UserId><LogonType>InteractiveToken</LogonType><RunLevel>LeastPrivilege</RunLevel></Principal></Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <StartWhenAvailable>true</StartWhenAvailable>
    <RunOnlyIfNetworkAvailable>false</RunOnlyIfNetworkAvailable>
    <Enabled>true</Enabled><Hidden>true</Hidden><ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <RestartOnFailure><Interval>PT1M</Interval><Count>3</Count></RestartOnFailure>
  </Settings>
  <Actions Context="Author"><Exec>
    <Command>$(ConvertTo-XmlText $PowerShellPath)</Command>
    <Arguments>$(ConvertTo-XmlText $arguments)</Arguments>
    <WorkingDirectory>$(ConvertTo-XmlText $BrokerRoot)</WorkingDirectory>
  </Exec></Actions>
</Task>
"@
}

function Assert-BrokerTaskXmlBinding {
    param(
        [Parameter(Mandatory = $true)][string]$Xml,
        [Parameter(Mandatory = $true)][string]$PowerShellPath,
        [Parameter(Mandatory = $true)][string]$WorkerPath,
        [Parameter(Mandatory = $true)][string]$BrokerRoot,
        [Parameter(Mandatory = $true)][string]$Requests,
        [Parameter(Mandatory = $true)][string]$Account,
        [Parameter(Mandatory = $true)][string]$UserSid
    )
    try { [xml]$document = $Xml } catch { throw "Registered task XML is invalid: $($_.Exception.Message)" }
    $namespace = [Xml.XmlNamespaceManager]::new($document.NameTable)
    $namespace.AddNamespace('t', 'http://schemas.microsoft.com/windows/2004/02/mit/task')
    Assert-TaskXmlStructureContract `
        -Document $document -Namespace $namespace -Label 'Registered task XML'
    $read = {
        param([string]$XPath)
        $nodes = @($document.SelectNodes($XPath, $namespace))
        if ($nodes.Count -ne 1) {
            throw "Registered task XML must contain exactly one $XPath"
        }
        return [string]$nodes[0].InnerText
    }
    $expectedArguments = Get-BrokerTaskArguments `
        -WorkerPath $WorkerPath -BrokerRoot $BrokerRoot -Requests $Requests
    $logonTrigger = $document.SelectSingleNode(
        '/t:Task/t:Triggers/t:LogonTrigger', $namespace
    )
    $principal = $document.SelectSingleNode(
        '/t:Task/t:Principals/t:Principal', $namespace
    )
    Assert-TaskUserIdentityBinding `
        -Actual (Read-UniqueTaskUserId `
            -Parent $logonTrigger -Label 'Registered task LogonTrigger') `
        -ExpectedAccount $Account -ExpectedSid $UserSid `
        -Label 'Registered task LogonTrigger UserId'
    Assert-TaskUserIdentityBinding `
        -Actual (Read-UniqueTaskUserId `
            -Parent $principal -Label 'Registered task Principal') `
        -ExpectedAccount $Account -ExpectedSid $UserSid `
        -Label 'Registered task Principal UserId'
    $checks = @(
        @('/t:Task/t:Principals/t:Principal/t:LogonType', 'InteractiveToken'),
        @('/t:Task/t:Settings/t:MultipleInstancesPolicy', 'IgnoreNew'),
        @('/t:Task/t:Settings/t:Hidden', 'true'),
        @('/t:Task/t:Settings/t:ExecutionTimeLimit', 'PT0S'),
        @('/t:Task/t:Actions/t:Exec/t:Command', $PowerShellPath),
        @('/t:Task/t:Actions/t:Exec/t:Arguments', $expectedArguments),
        @('/t:Task/t:Actions/t:Exec/t:WorkingDirectory', $BrokerRoot)
    )
    foreach ($check in $checks) {
        $actual = & $read $check[0]
        if ($actual -cne $check[1]) {
            throw "Registered task XML mismatch at $($check[0]). Expected=$($check[1]) Actual=$actual"
        }
    }
}

function Read-UniqueTaskUserId {
    param(
        [Parameter(Mandatory = $true)][Xml.XmlElement]$Parent,
        [Parameter(Mandatory = $true)][string]$Label
    )
    $taskNamespace = 'http://schemas.microsoft.com/windows/2004/02/mit/task'
    $nodes = @($Parent.SelectNodes('*[local-name()="UserId"]'))
    if ($nodes.Count -ne 1 -or $nodes[0].NamespaceURI -cne $taskNamespace) {
        throw "$Label must contain exactly one task-namespace UserId."
    }
    return [string]$nodes[0].InnerText
}

function Assert-TaskUserIdentityBinding {
    param(
        [Parameter(Mandatory = $true)][string]$Actual,
        [Parameter(Mandatory = $true)][string]$ExpectedAccount,
        [Parameter(Mandatory = $true)][string]$ExpectedSid,
        [Parameter(Mandatory = $true)][string]$Label
    )
    if ($Actual -ceq $ExpectedAccount) { return }
    try {
        $actualSid = if ($Actual -match '^S-1-(?:\d+-){1,14}\d+$') {
            [string][Security.Principal.SecurityIdentifier]::new($Actual).Value
        } else {
            [string]([Security.Principal.NTAccount]::new($Actual).Translate(
                [Security.Principal.SecurityIdentifier]
            ).Value)
        }
    } catch {
        throw "$Label cannot be resolved to a SID: $Actual ($($_.Exception.Message))"
    }
    if ($actualSid -cne $ExpectedSid) {
        throw "$Label identity mismatch. ExpectedAccount=$ExpectedAccount ExpectedSid=$ExpectedSid Actual=$Actual ActualSid=$actualSid"
    }
}

function Assert-TaskXmlStructureContract {
    param(
        [Parameter(Mandatory = $true)][xml]$Document,
        [Parameter(Mandatory = $true)][Xml.XmlNamespaceManager]$Namespace,
        [Parameter(Mandatory = $true)][string]$Label
    )
    $taskNamespace = 'http://schemas.microsoft.com/windows/2004/02/mit/task'
    $containers = @($Document.SelectNodes('/t:Task/t:Triggers', $Namespace))
    if ($containers.Count -ne 1) {
        throw "$Label must contain exactly one Triggers element."
    }
    $triggers = @($Document.SelectNodes('/t:Task/t:Triggers/*', $Namespace))
    if ($triggers.Count -ne 1 -or
        $triggers[0].LocalName -cne 'LogonTrigger' -or
        $triggers[0].NamespaceURI -cne $taskNamespace) {
        throw "$Label must contain exactly one LogonTrigger and no other triggers."
    }
    $settings = @($Document.SelectNodes('/t:Task/t:Settings', $Namespace))
    if ($settings.Count -ne 1) {
        throw "$Label must contain exactly one Settings element."
    }
    $assertDefaultTrue = {
        param([Xml.XmlElement]$Parent, [string]$FieldLabel)
        $enabledNodes = @($Parent.SelectNodes('*[local-name()="Enabled"]'))
        if ($enabledNodes.Count -gt 1) {
            throw "$Label $FieldLabel contains duplicate Enabled elements."
        }
        if ($enabledNodes.Count -eq 1 -and
            ($enabledNodes[0].NamespaceURI -cne $taskNamespace -or
             [string]$enabledNodes[0].InnerText -cne 'true')) {
            throw "$Label $FieldLabel is explicitly disabled or has an invalid Enabled value."
        }
    }
    & $assertDefaultTrue $triggers[0] 'LogonTrigger'
    & $assertDefaultTrue $settings[0] 'Settings'

    $principalContainers = @($Document.SelectNodes('/t:Task/t:Principals', $Namespace))
    $principals = @($Document.SelectNodes('/t:Task/t:Principals/*', $Namespace))
    if ($principalContainers.Count -ne 1 -or $principals.Count -ne 1 -or
        $principals[0].LocalName -cne 'Principal' -or
        $principals[0].NamespaceURI -cne $taskNamespace -or
        [string]$principals[0].GetAttribute('id') -cne 'Author') {
        throw "$Label must contain exactly one Author Principal."
    }
    $runLevels = @($principals[0].SelectNodes('*[local-name()="RunLevel"]'))
    if ($runLevels.Count -gt 1) {
        throw "$Label Principal contains duplicate RunLevel elements."
    }
    if ($runLevels.Count -eq 1 -and
        ($runLevels[0].NamespaceURI -cne $taskNamespace -or
         [string]$runLevels[0].InnerText -cne 'LeastPrivilege')) {
        throw "$Label Principal has an elevated or invalid RunLevel value."
    }
    $actionContainers = @($Document.SelectNodes('/t:Task/t:Actions', $Namespace))
    $actions = @($Document.SelectNodes('/t:Task/t:Actions/*', $Namespace))
    if ($actionContainers.Count -ne 1 -or $actions.Count -ne 1 -or
        [string]$actionContainers[0].GetAttribute('Context') -cne 'Author' -or
        $actions[0].LocalName -cne 'Exec' -or
        $actions[0].NamespaceURI -cne $taskNamespace) {
        throw "$Label must contain exactly one Author Exec action."
    }
}

function Get-ExpectedTaskSecurityDescriptor {
    param(
        [Parameter(Mandatory = $true)][string]$UserSid,
        [Parameter(Mandatory = $true)][string]$SandboxSid
    )
    return "O:${UserSid}G:BAD:P" +
        '(A;;GA;;;SY)' +
        '(A;;GA;;;BA)' +
        "(A;;GA;;;${UserSid})" +
        "(A;;GR;;;${SandboxSid})"
}

function Assert-TaskSecurityDescriptorContract {
    param(
        [Parameter(Mandatory = $true)][string]$Sddl,
        [Parameter(Mandatory = $true)][string]$UserSid,
        [Parameter(Mandatory = $true)][string]$SandboxSid,
        [Parameter(Mandatory = $true)][string]$Label
    )
    try {
        $descriptor = [Security.AccessControl.CommonSecurityDescriptor]::new(
            $false, $false, $Sddl
        )
    } catch {
        throw "$Label is not a valid task security descriptor: $($_.Exception.Message)"
    }
    $expectedGroup = 'S-1-5-32-544'
    $actualOwner = if ($null -ne $descriptor.Owner) {
        [string]$descriptor.Owner.Value
    } else { '' }
    $actualGroup = if ($null -ne $descriptor.Group) {
        [string]$descriptor.Group.Value
    } else { '' }
    if ($actualOwner -cne $UserSid -or $actualGroup -cne $expectedGroup) {
        throw "$Label owner/group mismatch. ExpectedOwner=$UserSid ExpectedGroup=$expectedGroup ActualOwner=$actualOwner ActualGroup=$actualGroup"
    }
    $forbiddenControl = [Security.AccessControl.ControlFlags]::DiscretionaryAclAutoInherited -bor `
        [Security.AccessControl.ControlFlags]::DiscretionaryAclAutoInheritRequired
    if (($descriptor.ControlFlags -band $forbiddenControl) -ne 0) {
        throw "$Label must not use an auto-inherited DACL. ControlFlags=$($descriptor.ControlFlags)"
    }
    $expected = @(
        @('S-1-5-18', 0x001F01FF),
        @('S-1-5-32-544', 0x001F01FF),
        @($UserSid, 0x001F01FF),
        @($SandboxSid, 0x00120089)
    )
    $acl = $descriptor.DiscretionaryAcl
    if ($null -eq $acl -or $acl.Count -ne $expected.Count) {
        throw "$Label must contain exactly four explicit allow ACEs. ActualCount=$($acl.Count)"
    }
    for ($index = 0; $index -lt $expected.Count; $index++) {
        $ace = $acl[$index]
        $actualSid = if ($null -ne $ace.SecurityIdentifier) {
            [string]$ace.SecurityIdentifier.Value
        } else { '' }
        if ([string]$ace.AceType -cne 'AccessAllowed' -or
            [string]$ace.AceFlags -cne 'None' -or
            $ace.IsInherited -or
            $actualSid -cne [string]$expected[$index][0] -or
            [int]$ace.AccessMask -ne [int]$expected[$index][1]) {
            throw "$Label ACE[$index] mismatch. ExpectedSid=$($expected[$index][0]) ExpectedMask=0x$('{0:x}' -f [int]$expected[$index][1]) ActualType=$($ace.AceType) ActualFlags=$($ace.AceFlags) ActualSid=$actualSid ActualMask=0x$('{0:x}' -f [int]$ace.AccessMask)"
        }
    }
}

function Set-And-AssertBrokerTaskSecurity {
    param(
        [Parameter(Mandatory = $true)][string]$Name,
        [Parameter(Mandatory = $true)][string]$ExpectedSddl,
        [Parameter(Mandatory = $true)][string]$UserSid,
        [Parameter(Mandatory = $true)][string]$SandboxSid
    )
    $context = Get-BrokerTaskCom -Name $Name
    if ($null -eq $context.Task) { throw "Broker task is missing: $Name" }
    $context.Task.SetSecurityDescriptor($ExpectedSddl, 0)
    $actual = [string]$context.Task.GetSecurityDescriptor(7)
    Assert-TaskSecurityDescriptorContract -Sddl $actual `
        -UserSid $UserSid -SandboxSid $SandboxSid -Label 'Broker task'
}

function Assert-BrokerTaskSecurity {
    param(
        [Parameter(Mandatory = $true)][string]$Name,
        [Parameter(Mandatory = $true)][string]$UserSid,
        [Parameter(Mandatory = $true)][string]$SandboxSid
    )
    $context = Get-BrokerTaskCom -Name $Name
    if ($null -eq $context.Task) { throw "Broker task is missing: $Name" }
    $actual = [string]$context.Task.GetSecurityDescriptor(7)
    Assert-TaskSecurityDescriptorContract -Sddl $actual `
        -UserSid $UserSid -SandboxSid $SandboxSid -Label 'Broker task'
}

function Invoke-Schtasks {
    param(
        [Parameter(Mandatory = $true)][string[]]$Arguments
    )
    $application = Join-Path $env:WINDIR 'System32\schtasks.exe'
    if (-not (Test-Path -LiteralPath $application -PathType Leaf)) {
        throw "System schtasks.exe is missing: $application"
    }
    $output = & $application @Arguments 2>&1 | Out-String
    $exitCode = $LASTEXITCODE
    if ($exitCode -ne 0) {
        throw "schtasks.exe failed with exit code ${exitCode}: $($output.Trim())"
    }
    return [pscustomobject]@{ ExitCode = $exitCode; Output = $output.Trim() }
}

function Test-TaskNotFoundException {
    param([Parameter(Mandatory = $true)][Exception]$Exception)
    $notFoundHResults = @(
        [IO.FileNotFoundException]::new().HResult,
        # Root GetFolder already succeeded; path-not-found is accepted only
        # for the fixed root name and is cross-checked by GetTasks below.
        [IO.DirectoryNotFoundException]::new().HResult
    )
    $cursor = $Exception
    while ($null -ne $cursor) {
        if ($notFoundHResults -contains [int]$cursor.HResult) { return $true }
        $cursor = $cursor.InnerException
    }
    return $false
}

function Get-RegisteredTaskCollectionItems {
    param([Parameter(Mandatory = $true)][AllowEmptyCollection()]$Collection)
    if ($Collection -is [Array]) { return @($Collection) }
    try { $count = [int]$Collection.Count }
    catch { throw "Task Scheduler collection count failed: $($_.Exception.Message)" }
    $items = [Collections.Generic.List[object]]::new()
    for ($index = 1; $index -le $count; $index++) {
        try { $items.Add($Collection.Item($index)) }
        catch {
            throw "Task Scheduler collection item ${index} failed: $($_.Exception.Message)"
        }
    }
    return @($items)
}

function Resolve-BrokerTaskFromFolder {
    param(
        [Parameter(Mandatory = $true)]$Folder,
        [Parameter(Mandatory = $true)][string]$Name
    )
    if ($Name -notmatch '^\\[A-Za-z0-9._-]+$') {
        throw "Task query name must identify one fixed root task: $Name"
    }
    try {
        $task = $Folder.GetTask($Name)
    } catch {
        if (-not (Test-TaskNotFoundException -Exception $_.Exception)) {
            throw "Task Scheduler GetTask query failed for ${Name}: $($_.Exception.Message)"
        }
        try {
            $collection = $Folder.GetTasks(1)
            $tasks = @(Get-RegisteredTaskCollectionItems -Collection $collection)
            $matches = @(
                $tasks | Where-Object {
                    [string]$_.Path -ieq $Name
                }
            )
        } catch {
            throw "Task Scheduler absence confirmation failed for ${Name}: $($_.Exception.Message)"
        }
        if ($matches.Count -eq 0) { return $null }
        throw "Task Scheduler GetTask reported not-found but enumeration still contains ${Name}."
    }
    if ($null -eq $task) {
        throw "Task Scheduler GetTask returned null without a not-found error: $Name"
    }
    try { $actualPath = [string]$task.Path }
    catch { throw "Task Scheduler task path read failed for ${Name}: $($_.Exception.Message)" }
    if ($actualPath -ine $Name) {
        throw "Task Scheduler returned the wrong task. Expected=$Name Actual=$actualPath"
    }
    return $task
}

function Read-BrokerTaskXmlSnapshot {
    param(
        [Parameter(Mandatory = $true)]$Task,
        [Parameter(Mandatory = $true)][string]$Name
    )
    try { $xml = [string]$Task.Xml }
    catch { throw "Task Scheduler XML query failed for ${Name}: $($_.Exception.Message)" }
    if ([string]::IsNullOrWhiteSpace($xml)) {
        throw "Task Scheduler returned empty XML for $Name"
    }
    return $xml
}

function Get-TaskXmlSnapshot {
    param([Parameter(Mandatory = $true)][string]$Name)
    $context = Get-BrokerTaskCom -Name $Name
    if ($null -eq $context.Task) { return $null }
    return Read-BrokerTaskXmlSnapshot -Task $context.Task -Name $Name
}

function Register-BrokerTask {
    param(
        [Parameter(Mandatory = $true)][string]$Name,
        [Parameter(Mandatory = $true)][string]$Xml,
        [Parameter(Mandatory = $true)][string]$StagingRoot,
        [Parameter(Mandatory = $true)][Security.AccessControl.FileSecurity]$FileSecurity
    )
    $xmlPath = Join-Path $StagingRoot (
        '.task-' + [Guid]::NewGuid().ToString('N') + '.xml'
    )
    Write-BytesAtomic -Path $xmlPath -Bytes (ConvertTo-Utf16LeBom -Text $Xml) `
        -Security $FileSecurity
    try {
        [void](Invoke-Schtasks -Arguments @('/Create', '/TN', $Name, '/XML', $xmlPath, '/F'))
    } finally {
        Remove-Item -LiteralPath $xmlPath -Force -ErrorAction SilentlyContinue
    }
}

function Start-BrokerTask { param([string]$Name); [void](Invoke-Schtasks -Arguments @('/Run', '/TN', $Name)) }

function Get-BrokerTaskCom {
    param([Parameter(Mandatory = $true)][string]$Name)
    try {
        $service = New-Object -ComObject 'Schedule.Service'
        $service.Connect()
        $folder = $service.GetFolder('\')
    } catch {
        throw "Task Scheduler connection or root-folder query failed: $($_.Exception.Message)"
    }
    $task = Resolve-BrokerTaskFromFolder -Folder $folder -Name $Name
    return [pscustomobject]@{ Service = $service; Folder = $folder; Task = $task }
}

function Stop-BrokerTaskStrict {
    param(
        [Parameter(Mandatory = $true)][string]$Name,
        [switch]$AllowMissing
    )
    $context = Get-BrokerTaskCom -Name $Name
    if ($null -eq $context.Task) {
        if ($AllowMissing) { return $false }
        throw "Broker task is missing: $Name"
    }
    foreach ($instance in @($context.Task.GetInstances(0))) { $instance.Stop() }
    $deadline = [DateTimeOffset]::UtcNow.AddSeconds(10)
    while ([DateTimeOffset]::UtcNow -lt $deadline) {
        if (@($context.Task.GetInstances(0)).Count -eq 0) { return $true }
        Start-Sleep -Milliseconds 100
    }
    throw "Broker task instances did not stop: $Name"
}

function Remove-BrokerTaskStrict {
    param(
        [Parameter(Mandatory = $true)][string]$Name,
        [switch]$AllowMissing
    )
    $context = Get-BrokerTaskCom -Name $Name
    if ($null -eq $context.Task) {
        if ($AllowMissing) { return $false }
        throw "Broker task is missing: $Name"
    }
    [void](Stop-BrokerTaskStrict -Name $Name)
    $context.Folder.DeleteTask($Name.TrimStart('\'), 0)
    $verify = Get-BrokerTaskCom -Name $Name
    if ($null -ne $verify.Task) { throw "Broker task deletion did not persist: $Name" }
    return $true
}

function Assert-BrokerTaskAbsent {
    param(
        [Parameter(Mandatory = $true)][string]$Name,
        [scriptblock]$LookupAction
    )
    if ($null -eq $LookupAction) {
        $LookupAction = {
            param([string]$TaskName)
            Get-BrokerTaskCom -Name $TaskName
        }
    }
    $context = & $LookupAction $Name
    if ($null -eq $context) {
        throw "Task lookup returned no context while confirming absence: $Name"
    }
    if ($null -ne $context.Task) {
        throw "Broker task is still present: $Name"
    }
    return $true
}

function Invoke-AfterBrokerTaskAbsence {
    param(
        [Parameter(Mandatory = $true)][string]$Name,
        [Parameter(Mandatory = $true)][scriptblock]$Action,
        [scriptblock]$LookupAction
    )
    [void](Assert-BrokerTaskAbsent -Name $Name -LookupAction $LookupAction)
    & $Action
}

function Invoke-BrokerUninstallDestructivePhase {
    param(
        [Parameter(Mandatory = $true)][string]$Name,
        [Parameter(Mandatory = $true)][scriptblock]$QueryXmlAction,
        [Parameter(Mandatory = $true)][scriptblock]$ValidateTaskAction,
        [Parameter(Mandatory = $true)][scriptblock]$RemoveTaskAction,
        [Parameter(Mandatory = $true)][scriptblock]$RemoveRootAction,
        [scriptblock]$LookupAction
    )
    $taskXml = & $QueryXmlAction $Name
    if ($null -ne $taskXml) {
        [void](& $ValidateTaskAction ([string]$taskXml))
        [void](& $RemoveTaskAction $Name)
    }
    Invoke-AfterBrokerTaskAbsence `
        -Name $Name -LookupAction $LookupAction -Action $RemoveRootAction
}

function Confirm-BrokerTaskSafeForFileRemoval {
    param(
        [Parameter(Mandatory = $true)][bool]$TaskRegistered,
        [Parameter(Mandatory = $true)][string]$Name,
        [scriptblock]$RemoveTaskAction,
        [scriptblock]$LookupAction
    )
    if ($TaskRegistered) {
        if ($null -eq $RemoveTaskAction) {
            $RemoveTaskAction = {
                param([string]$TaskName)
                [void](Remove-BrokerTaskStrict -Name $TaskName -AllowMissing)
            }
        }
        [void](& $RemoveTaskAction $Name)
    }
    [void](Assert-BrokerTaskAbsent -Name $Name -LookupAction $LookupAction)
    return $true
}

function Wait-BrokerWorkerLockReleased {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [ValidateRange(1, 60000)][int]$TimeoutMilliseconds = 15000
    )
    $deadline = [DateTimeOffset]::UtcNow.AddMilliseconds($TimeoutMilliseconds)
    while ($true) {
        if (-not (Test-Path -LiteralPath $Path)) { return }
        $stream = $null
        try {
            $stream = [IO.FileStream]::new(
                $Path, [IO.FileMode]::Open, [IO.FileAccess]::ReadWrite,
                [IO.FileShare]::None
            )
            return
        } catch [IO.IOException] {
            if ([DateTimeOffset]::UtcNow -ge $deadline) {
                throw "Broker worker lock did not release before timeout: $Path"
            }
        } finally {
            if ($null -ne $stream) { $stream.Dispose() }
        }
        Start-Sleep -Milliseconds 100
    }
}

function Test-BrokerWorkerLockReleased {
    param([Parameter(Mandatory = $true)][string]$Path)

    if (-not (Test-Path -LiteralPath $Path)) { return $true }
    $stream = $null
    try {
        $stream = [IO.FileStream]::new(
            $Path, [IO.FileMode]::Open, [IO.FileAccess]::ReadWrite,
            [IO.FileShare]::None
        )
        return $true
    } catch [IO.IOException] {
        return $false
    } finally {
        if ($null -ne $stream) { $stream.Dispose() }
    }
}

function Assert-PartialUninstallRemnant {
    param(
        [Parameter(Mandatory = $true)][string]$Root,
        [Parameter(Mandatory = $true)]$ExpectedRootSecurity,
        [Parameter(Mandatory = $true)]$ExpectedFileSecurity,
        [Parameter(Mandatory = $true)][string]$UserSid
    )
    $root = Assert-RealDirectory -Path $Root -Label 'Partial uninstall root'
    $receiptPath = Join-Path $root $script:ReceiptName
    if (Test-Path -LiteralPath $receiptPath) {
        throw 'Partial uninstall recovery requires the protected receipt to be absent.'
    }
    $entries = @(Get-ChildItem -LiteralPath $root -Force -ErrorAction Stop)
    if ($entries.Count -ne 1 -or $entries[0].PSIsContainer -or
        [string]$entries[0].Name -cne 'worker.lock' -or
        ($entries[0].Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0 -or
        $entries[0].Length -ne 0) {
        throw 'Partial uninstall recovery accepts only one zero-byte regular worker.lock.'
    }
    $lockPath = Assert-ChildPath `
        -Child ([string]$entries[0].FullName) -Parent $root `
        -Label 'Partial uninstall worker lock'
    Assert-ExactSecurity -Path $root -Expected $ExpectedRootSecurity `
        -ExpectedOwnerSid $UserSid -Label 'Partial uninstall root'
    Assert-ExactSecurity -Path $lockPath -Expected $ExpectedFileSecurity `
        -ExpectedOwnerSid $UserSid -Label 'Partial uninstall worker lock'
    return [pscustomobject]@{ Root = $root; WorkerLock = $lockPath }
}

function Read-JsonDocument {
    param([string]$Path, [string]$Label)
    $item = Get-Item -LiteralPath $Path -Force -ErrorAction Stop
    if ($item.PSIsContainer -or
        ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0 -or
        $item.Length -le 0 -or $item.Length -gt 64KB) {
        throw "$Label is not a bounded regular file: $Path"
    }
    $text = [Text.UTF8Encoding]::new($false, $true).GetString(
        [IO.File]::ReadAllBytes($Path)
    )
    return $text | ConvertFrom-Json `
        -Depth 12 -NoEnumerate -DateKind String -ErrorAction Stop
}

function Assert-ExactProperties {
    param(
        [Parameter(Mandatory = $true)]$Document,
        [Parameter(Mandatory = $true)][AllowEmptyCollection()][string[]]$Expected,
        [Parameter(Mandatory = $true)][string]$Label
    )
    if ($Document -isnot [pscustomobject]) { throw "$Label must be a JSON object." }
    $actual = @($Document.PSObject.Properties.Name | Sort-Object)
    $wanted = @($Expected | Sort-Object)
    if (($actual -join "`n") -cne ($wanted -join "`n")) {
        throw "$Label has unexpected properties. Expected=$($wanted -join ',') Actual=$($actual -join ',')"
    }
}

function Read-Receipt {
    param(
        [Parameter(Mandatory = $true)][string]$Root,
        [Parameter(Mandatory = $true)][string]$Requests,
        [Parameter(Mandatory = $true)][string]$Name,
        [switch]$SkipTaskValidation
    )
    $receipt = Read-JsonDocument `
        -Path (Join-Path $Root $script:ReceiptName) `
        -Label 'Broker install receipt'
    Assert-ExactProperties -Document $receipt -Label 'Broker install receipt' -Expected @(
        'capabilities', 'install_id', 'install_root', 'installed_at_utc',
        'powershell_path', 'powershell_sha256', 'request_root', 'result_root',
        'sandbox_group', 'sandbox_group_sid', 'schema_version', 'task_name',
        'user_account', 'user_sid', 'worker_path', 'worker_sha256'
    )
    if ($receipt.schema_version -ne $script:SchemaVersion -or
        [string]$receipt.install_root -cne $Root -or
        [string]$receipt.request_root -cne $Requests -or
        [string]$receipt.result_root -cne (Join-Path $Root 'results') -or
        [string]$receipt.task_name -cne $Name -or
        $receipt.capabilities -isnot [array] -or
        @($receipt.capabilities).Count -ne 1 -or
        [string]$receipt.capabilities[0] -cne 'identity_probe') {
        throw 'Broker install receipt is not bound to the requested install root.'
    }
    $installId = [Guid]::Empty
    if (-not [Guid]::TryParseExact([string]$receipt.install_id, 'N', [ref]$installId)) {
        throw 'Broker install receipt has a non-canonical install_id.'
    }
    foreach ($sid in @([string]$receipt.user_sid, [string]$receipt.sandbox_group_sid)) {
        try { [void][Security.Principal.SecurityIdentifier]::new($sid) }
        catch { throw "Broker install receipt has an invalid SID: $sid" }
    }
    $workerPath = Get-NormalizedAbsolutePath `
        -Path ([string]$receipt.worker_path) -Label 'Installed worker'
    $versionRoot = Split-Path -Parent $workerPath
    $versions = Split-Path -Parent $versionRoot
    if ($versions -cne (Join-Path $Root 'versions') -or
        [IO.Path]::GetFileName($versionRoot) -cne [string]$receipt.worker_sha256 -or
        [IO.Path]::GetFileName($workerPath) -cne 'codex-powershell-broker.ps1' -or
        -not (Test-Path -LiteralPath $workerPath -PathType Leaf) -or
        (((Get-Item -LiteralPath $workerPath -Force).Attributes -band
            [IO.FileAttributes]::ReparsePoint) -ne 0) -or
        (Get-FileSha256 -Path $workerPath) -cne [string]$receipt.worker_sha256) {
        throw 'Broker worker does not match its install receipt.'
    }
    $powershellPath = Get-NormalizedAbsolutePath `
        -Path ([string]$receipt.powershell_path) -Label 'Installed PowerShell runtime'
    if (-not (Test-Path -LiteralPath $powershellPath -PathType Leaf) -or
        (((Get-Item -LiteralPath $powershellPath -Force).Attributes -band
            [IO.FileAttributes]::ReparsePoint) -ne 0) -or
        (Get-FileSha256 -Path $powershellPath) -cne
            [string]$receipt.powershell_sha256) {
        throw 'PowerShell runtime does not match its install receipt.'
    }
    $readOnly = New-ManagedDirectorySecurity `
        -UserSid ([string]$receipt.user_sid) `
        -SandboxSid ([string]$receipt.sandbox_group_sid) -Kind ReadOnly
    $fileSecurity = New-ManagedFileSecurity `
        -UserSid ([string]$receipt.user_sid) `
        -SandboxSid ([string]$receipt.sandbox_group_sid)
    foreach ($entry in @(
        @($Root, $readOnly, 'Install root'),
        @((Join-Path $Root 'results'), $readOnly, 'Result root'),
        @($versions, $readOnly, 'Version root'),
        @($versionRoot, $readOnly, 'Worker version directory')
    )) {
        [void](Assert-RealDirectory -Path $entry[0] -Label $entry[2])
        Assert-ExactSecurity -Path $entry[0] -Expected $entry[1] `
            -ExpectedOwnerSid ([string]$receipt.user_sid) -Label $entry[2]
    }
    [void](Assert-RealDirectory -Path $Requests -Label 'Request root')
    Assert-RequestRootSecurity -Path $Requests `
        -ExpectedOwnerSid ([string]$receipt.user_sid) `
        -SandboxSid ([string]$receipt.sandbox_group_sid) -Label 'Request root'
    foreach ($entry in @(
        @((Join-Path $Root $script:ReceiptName), 'Install receipt'),
        @($workerPath, 'Installed worker'),
        @((Join-Path $Root 'worker.lock'), 'Worker lock')
    )) {
        if (-not (Test-Path -LiteralPath $entry[0] -PathType Leaf)) {
            throw "$($entry[1]) is missing: $($entry[0])"
        }
        if ((((Get-Item -LiteralPath $entry[0] -Force).Attributes -band
            [IO.FileAttributes]::ReparsePoint) -ne 0)) {
            throw "$($entry[1]) must not be a reparse point: $($entry[0])"
        }
        Assert-ExactSecurity -Path $entry[0] -Expected $fileSecurity `
            -ExpectedOwnerSid ([string]$receipt.user_sid) -Label $entry[1]
    }
    if (-not $SkipTaskValidation) {
        $taskXml = Get-TaskXmlSnapshot -Name $Name
        if ($null -eq $taskXml) { throw "Broker task is missing: $Name" }
        Assert-BrokerTaskXmlBinding -Xml $taskXml `
            -PowerShellPath $powershellPath -WorkerPath $workerPath `
            -BrokerRoot $Root -Requests $Requests `
            -Account ([string]$receipt.user_account) `
            -UserSid ([string]$receipt.user_sid)
        Assert-BrokerTaskSecurity -Name $Name `
            -UserSid ([string]$receipt.user_sid) `
            -SandboxSid ([string]$receipt.sandbox_group_sid)
    }
    return $receipt
}

function Wait-BrokerHeartbeat {
    param(
        [string]$Root,
        $Receipt,
        [int]$TimeoutSeconds = 20,
        [DateTimeOffset]$NotBefore = [DateTimeOffset]::MinValue
    )
    $heartbeatPath = Join-Path (Join-Path $Root 'results') $script:HeartbeatName
    $deadline = [DateTimeOffset]::UtcNow.AddSeconds($TimeoutSeconds)
    while ([DateTimeOffset]::UtcNow -lt $deadline) {
        if (Test-Path -LiteralPath $heartbeatPath -PathType Leaf) {
            try {
                $heartbeat = Read-JsonDocument -Path $heartbeatPath -Label 'Broker heartbeat'
                Assert-ExactProperties `
                    -Document $heartbeat -Label 'Broker heartbeat' -Expected @(
                        'executor_account', 'executor_sid', 'install_id',
                        'observed_at_utc', 'powershell_sha256', 'process_id',
                        'schema_version', 'session_id', 'worker_sha256'
                    )
                $processId = 0
                $sessionId = -1
                if (-not [int]::TryParse(
                    [string]$heartbeat.process_id,
                    [Globalization.NumberStyles]::None,
                    [Globalization.CultureInfo]::InvariantCulture,
                    [ref]$processId
                ) -or $processId -le 0 -or
                    -not [int]::TryParse(
                        [string]$heartbeat.session_id,
                        [Globalization.NumberStyles]::None,
                        [Globalization.CultureInfo]::InvariantCulture,
                        [ref]$sessionId
                    ) -or $sessionId -lt 0) {
                    throw 'Broker heartbeat process/session identity is invalid.'
                }
                $observed = [DateTimeOffset]::Parse(
                    [string]$heartbeat.observed_at_utc,
                    [Globalization.CultureInfo]::InvariantCulture,
                    [Globalization.DateTimeStyles]::RoundtripKind
                ).ToUniversalTime()
                $now = [DateTimeOffset]::UtcNow
                if ($heartbeat.schema_version -eq $script:SchemaVersion -and
                    [string]$heartbeat.install_id -ceq [string]$Receipt.install_id -and
                    [string]$heartbeat.executor_account -ceq
                        [string]$Receipt.user_account -and
                    [string]$heartbeat.executor_sid -ceq [string]$Receipt.user_sid -and
                    [string]$heartbeat.worker_sha256 -ceq [string]$Receipt.worker_sha256 -and
                    [string]$heartbeat.powershell_sha256 -ceq
                        [string]$Receipt.powershell_sha256 -and
                    $observed -ge $NotBefore -and
                    $observed -le $now.AddSeconds(30) -and
                    ($now - $observed).TotalSeconds -le 15) {
                    return $heartbeat
                }
            } catch { }
        }
        Start-Sleep -Milliseconds 200
    }
    throw 'Broker task did not publish a fresh qinrm-bound heartbeat.'
}

function Assert-ReceiptBoundWorkerProcessRecord {
    param(
        [Parameter(Mandatory = $true)]$Receipt,
        [Parameter(Mandatory = $true)]$Heartbeat,
        [Parameter(Mandatory = $true)][string]$ExecutablePath,
        [Parameter(Mandatory = $true)][string]$CommandLine,
        [Parameter(Mandatory = $true)][string]$OwnerSid,
        [Parameter(Mandatory = $true)][int]$SessionId
    )

    $expectedArguments = Get-BrokerTaskArguments `
        -WorkerPath ([string]$Receipt.worker_path) `
        -BrokerRoot ([string]$Receipt.install_root) `
        -Requests ([string]$Receipt.request_root)
    $quotedCommand = '"' + [string]$Receipt.powershell_path +
        '" ' + $expectedArguments
    $bareCommand = [string]$Receipt.powershell_path + ' ' + $expectedArguments
    if (-not $ExecutablePath.Equals(
            [string]$Receipt.powershell_path,
            [StringComparison]::OrdinalIgnoreCase
        ) -or
        ($CommandLine -cne $quotedCommand -and $CommandLine -cne $bareCommand) -or
        $OwnerSid -cne [string]$Receipt.user_sid -or
        $SessionId -ne [int]$Heartbeat.session_id) {
        throw 'Heartbeat PID is not the exact receipt-bound broker worker process.'
    }
}

function Get-ReceiptBoundWorkerProcess {
    param(
        [Parameter(Mandatory = $true)][string]$Root,
        [Parameter(Mandatory = $true)]$Receipt
    )

    $heartbeat = Wait-BrokerHeartbeat `
        -Root $Root -Receipt $Receipt -TimeoutSeconds 2
    $processId = [int]$heartbeat.process_id
    $process = $null
    try {
        $process = [Diagnostics.Process]::GetProcessById($processId)
        [void]$process.Handle
        if ($process.HasExited) {
            throw "Heartbeat worker process already exited: $processId"
        }
        $processStartUtc = $process.StartTime.ToUniversalTime()
        $processPath = $process.MainModule.FileName
        $processSession = $process.SessionId
        $record = Get-CimInstance -ClassName Win32_Process `
            -Filter "ProcessId = $processId" -ErrorAction Stop
        if ($null -eq $record -or [int]$record.ProcessId -ne $processId) {
            throw "Heartbeat worker process record is missing: $processId"
        }
        if ([int]$record.SessionId -ne $processSession) {
            throw 'Heartbeat worker process session changed during binding.'
        }
        $recordStartUtc = ([DateTime]$record.CreationDate).ToUniversalTime()
        if ([Math]::Abs(($recordStartUtc - $processStartUtc).TotalSeconds) -gt 1) {
            throw 'Heartbeat worker PID was reused during process binding.'
        }
        $owner = Invoke-CimMethod -InputObject $record `
            -MethodName GetOwnerSid -ErrorAction Stop
        if ([int]$owner.ReturnValue -ne 0 -or [string]::IsNullOrWhiteSpace(
            [string]$owner.Sid
        )) {
            throw 'Heartbeat worker owner SID could not be resolved.'
        }
        Assert-ReceiptBoundWorkerProcessRecord `
            -Receipt $Receipt -Heartbeat $heartbeat `
            -ExecutablePath ([string]$record.ExecutablePath) `
            -CommandLine ([string]$record.CommandLine) `
            -OwnerSid ([string]$owner.Sid) -SessionId $processSession
        if (-not $processPath.Equals(
            [string]$Receipt.powershell_path,
            [StringComparison]::OrdinalIgnoreCase
        )) {
            throw 'Held worker process image path does not match the receipt.'
        }
        return [pscustomobject]@{
            Process = $process
            Heartbeat = $heartbeat
            ProcessId = $processId
            StartTimeUtc = $processStartUtc
        }
    } catch {
        if ($null -ne $process) { $process.Dispose() }
        throw
    }
}

function Stop-ReceiptBoundWorkerProcess {
    param(
        [Parameter(Mandatory = $true)]$Binding,
        [Parameter(Mandatory = $true)][string]$LockPath
    )

    $process = $Binding.Process
    if (-not $process.HasExited) {
        $process.Kill($true)
        if (-not $process.WaitForExit(10000)) {
            throw "Receipt-bound worker did not exit: $($Binding.ProcessId)"
        }
    }
    Wait-BrokerWorkerLockReleased -Path $LockPath
}

function Confirm-BrokerWorkerStoppedForRootRemoval {
    param(
        [Parameter(Mandatory = $true)][string]$LockPath,
        $Binding,
        [scriptblock]$TestLockAction,
        [scriptblock]$StopAction
    )

    if ($null -eq $TestLockAction) {
        $TestLockAction = {
            param([string]$Path)
            Test-BrokerWorkerLockReleased -Path $Path
        }
    }
    if (& $TestLockAction $LockPath) { return }
    if ($null -eq $Binding) {
        throw 'Worker lock remained held without a receipt-bound process handle.'
    }
    if ($null -eq $StopAction) {
        $StopAction = {
            param($WorkerBinding, [string]$Path)
            Stop-ReceiptBoundWorkerProcess `
                -Binding $WorkerBinding -LockPath $Path
        }
    }
    & $StopAction $Binding $LockPath
    if (-not (& $TestLockAction $LockPath)) {
        throw 'Receipt-bound worker stopped but its lock remained held.'
    }
}

function Get-InstallationState {
    param([string]$Root, [string]$Requests, [string]$Name)
    $receipt = $null
    $errorText = $null
    $installed = $null
    try {
        $receipt = Read-Receipt -Root $Root -Requests $Requests -Name $Name
        $installed = $true
    } catch {
        $errorText = $_.Exception.Message
        if (-not (Test-Path -LiteralPath $Root)) { $installed = $false }
    }
    $taskRegistered = $null
    try { $taskRegistered = $null -ne (Get-BrokerTaskCom -Name $Name).Task }
    catch { if ($null -eq $errorText) { $errorText = $_.Exception.Message } }
    $heartbeatReady = $false
    if ($null -ne $receipt) {
        try {
            [void](Wait-BrokerHeartbeat -Root $Root -Receipt $receipt -TimeoutSeconds 1)
            $heartbeatReady = $true
        } catch { if ($null -eq $errorText) { $errorText = $_.Exception.Message } }
    }
    return [ordered]@{
        installed = $installed
        task_registered = $taskRegistered
        heartbeat_ready = $heartbeatReady
        install_root = if ($null -ne $receipt) { [string]$receipt.install_root } else { $Root }
        request_root = if ($null -ne $receipt) { [string]$receipt.request_root } else { $Requests }
        task_name = if ($null -ne $receipt) { [string]$receipt.task_name } else { $Name }
        install_id = if ($null -ne $receipt) { [string]$receipt.install_id } else { $null }
        user_account = if ($null -ne $receipt) { [string]$receipt.user_account } else { $null }
        user_sid = if ($null -ne $receipt) { [string]$receipt.user_sid } else { $null }
        worker_sha256 = if ($null -ne $receipt) { [string]$receipt.worker_sha256 } else { $null }
        error = $errorText
    }
}

function Install-Broker {
    param([string]$Root, [string]$Requests, [string]$Name, [string]$Account, [string]$Group)
    if (-not (Test-Path -LiteralPath $script:BrokerSource -PathType Leaf)) {
        throw "Broker source is missing: $script:BrokerSource"
    }
    $sourceItem = Get-Item -LiteralPath $script:BrokerSource -Force
    if (($sourceItem.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
        throw 'Broker source must not be a reparse point.'
    }
    $userSid = Resolve-AccountSid -Account $Account -Label 'Target user'
    $sandboxSid = Resolve-AccountSid -Account $Group -Label 'Sandbox group'
    [void](Assert-InstallAuthority -ExpectedAccount $Account -ExpectedSid $userSid)
    $expectedRequests = Join-Path $Root 'requests'
    if ($Requests -cne $expectedRequests) {
        throw "Request root must be the protected install-root child: $expectedRequests"
    }
    if ($Name -notmatch '^\\[A-Za-z0-9._-]+$') {
        throw "Broker task name must be one fixed root task: $Name"
    }
    $sourceBytes = [IO.File]::ReadAllBytes($script:BrokerSource)
    $workerHash = Get-BytesSha256 -Bytes $sourceBytes
    $runtime = Get-CurrentPowerShellRuntime
    $existingTask = (Get-BrokerTaskCom -Name $Name).Task
    if (Test-Path -LiteralPath $Root) {
        $existing = Read-Receipt -Root $Root -Requests $Requests -Name $Name
        if ([string]$existing.user_account -cne $Account -or
            [string]$existing.user_sid -cne $userSid -or
            [string]$existing.sandbox_group -cne $Group -or
            [string]$existing.sandbox_group_sid -cne $sandboxSid -or
            [string]$existing.worker_sha256 -cne $workerHash -or
            [string]$existing.powershell_path -cne $runtime.Path -or
            [string]$existing.powershell_sha256 -cne $runtime.Sha256) {
            throw 'An existing broker install is not the exact current tuple; uninstall it explicitly before installing new bytes.'
        }
        $heartbeat = Wait-BrokerHeartbeat -Root $Root -Receipt $existing -TimeoutSeconds 2
        return [ordered]@{
            installed = $true
            already_current = $true
            install_id = [string]$existing.install_id
            task_name = $Name
            install_root = $Root
            request_root = $Requests
            result_root = [string]$existing.result_root
            user_account = $Account
            user_sid = $userSid
            sandbox_group_sid = $sandboxSid
            worker_sha256 = $workerHash
            powershell_sha256 = $runtime.Sha256
            heartbeat_executor = [string]$heartbeat.executor_account
            heartbeat_sid = [string]$heartbeat.executor_sid
        }
    }
    if ($null -ne $existingTask) {
        throw "Broker task exists without a protected receipt-bound install root: $Name"
    }

    $readOnlySecurity = New-ManagedDirectorySecurity `
        -UserSid $userSid -SandboxSid $sandboxSid -Kind ReadOnly
    $requestSecurity = New-ManagedDirectorySecurity `
        -UserSid $userSid -SandboxSid $sandboxSid -Kind Requests
    $fileSecurity = New-ManagedFileSecurity -UserSid $userSid -SandboxSid $sandboxSid
    $rootCreated = $false
    $taskRegistered = $false
    $root = $Root
    $requests = $Requests
    $versions = Join-Path $root 'versions'
    $results = Join-Path $root 'results'
    $versionRoot = Join-Path $versions $workerHash
    $workerPath = Join-Path $versionRoot 'codex-powershell-broker.ps1'
    $workerLockPath = Join-Path $root 'worker.lock'
    $receiptPath = Join-Path $root $script:ReceiptName
    $installId = [Guid]::NewGuid().ToString('N')
    $receipt = [ordered]@{
        schema_version = $script:SchemaVersion
        install_id = $installId
        install_root = $root
        installed_at_utc = [DateTimeOffset]::UtcNow.ToString('o')
        task_name = $Name
        user_account = $Account
        user_sid = $userSid
        sandbox_group = $Group
        sandbox_group_sid = $sandboxSid
        worker_path = $workerPath
        worker_sha256 = $workerHash
        powershell_path = $runtime.Path
        powershell_sha256 = $runtime.Sha256
        request_root = $requests
        result_root = $results
        capabilities = @('identity_probe')
    }
    $taskXml = New-BrokerTaskXml `
        -PowerShellPath $runtime.Path -WorkerPath $workerPath -BrokerRoot $root `
        -Requests $requests -Account $Account -Name $Name

    try {
        $root = New-ManagedDirectory -Path $root -Security $readOnlySecurity `
            -OwnerSid $userSid -Label 'Broker install root'
        $rootCreated = $true
        $versions = New-ManagedDirectory -Path $versions -Security $readOnlySecurity `
            -OwnerSid $userSid -Label 'Broker version root'
        $results = New-ManagedDirectory -Path $results -Security $readOnlySecurity `
            -OwnerSid $userSid -Label 'Broker result root'
        $requests = New-ManagedDirectory -Path $requests -Security $requestSecurity `
            -OwnerSid $userSid -Label 'Broker request root'
        $versionRoot = New-ManagedDirectory -Path $versionRoot `
            -Security $readOnlySecurity -OwnerSid $userSid `
            -Label 'Broker version directory'
        Write-BytesAtomic -Path $workerPath -Bytes $sourceBytes -Security $fileSecurity
        Assert-ExactSecurity -Path $workerPath -Expected $fileSecurity `
            -ExpectedOwnerSid $userSid -Label 'Installed worker'
        Write-BytesAtomic -Path $workerLockPath -Bytes ([byte[]]::new(0)) `
            -Security $fileSecurity
        Assert-ExactSecurity -Path $workerLockPath -Expected $fileSecurity `
            -ExpectedOwnerSid $userSid -Label 'Worker lock'
        Write-JsonAtomic -Path $receiptPath -Document $receipt -Security $fileSecurity
        Assert-ExactSecurity -Path $receiptPath -Expected $fileSecurity `
            -ExpectedOwnerSid $userSid -Label 'Install receipt'
        $heartbeatPath = Join-Path $results $script:HeartbeatName
        Remove-Item -LiteralPath $heartbeatPath -Force -ErrorAction SilentlyContinue
        Register-BrokerTask -Name $Name -Xml $taskXml -StagingRoot $root `
            -FileSecurity $fileSecurity
        $taskRegistered = $true
        $taskSecurity = Get-ExpectedTaskSecurityDescriptor `
            -UserSid $userSid -SandboxSid $sandboxSid
        Set-And-AssertBrokerTaskSecurity -Name $Name -ExpectedSddl $taskSecurity `
            -UserSid $userSid -SandboxSid $sandboxSid
        $registeredXml = Get-TaskXmlSnapshot -Name $Name
        if ($null -eq $registeredXml) { throw 'Registered broker task cannot be read back.' }
        Assert-BrokerTaskXmlBinding -Xml $registeredXml `
            -PowerShellPath $runtime.Path -WorkerPath $workerPath `
            -BrokerRoot $root -Requests $requests -Account $Account `
            -UserSid $userSid
        $startedAt = [DateTimeOffset]::UtcNow
        Start-BrokerTask -Name $Name
        $installedReceipt = Read-Receipt -Root $root -Requests $requests -Name $Name
        $heartbeat = Wait-BrokerHeartbeat -Root $root -Receipt $installedReceipt `
            -TimeoutSeconds 20 -NotBefore $startedAt
        return [ordered]@{
            installed = $true
            already_current = $false
            install_id = $installId
            task_name = $Name
            install_root = $root
            request_root = $requests
            result_root = $results
            user_account = $Account
            user_sid = $userSid
            sandbox_group_sid = $sandboxSid
            worker_sha256 = $workerHash
            powershell_sha256 = $runtime.Sha256
            heartbeat_executor = [string]$heartbeat.executor_account
            heartbeat_sid = [string]$heartbeat.executor_sid
        }
    } catch {
        $failure = $_.Exception.Message
        $taskSafeForFileRemoval = $false
        try {
            $taskSafeForFileRemoval = Confirm-BrokerTaskSafeForFileRemoval `
                -TaskRegistered $taskRegistered -Name $Name
        } catch {
            $failure += "; task rollback failed and files were preserved: $($_.Exception.Message)"
        }
        if ($taskSafeForFileRemoval -and $rootCreated -and
            (Test-Path -LiteralPath $root -PathType Container)) {
            try {
                $removeRootAction = {
                    Assert-ExactSecurity -Path $root -Expected $readOnlySecurity `
                        -ExpectedOwnerSid $userSid -Label 'Rollback install root'
                    Wait-BrokerWorkerLockReleased `
                        -Path (Join-Path $root 'worker.lock')
                    Remove-Item -LiteralPath $root -Recurse -Force -ErrorAction Stop
                    if (Test-Path -LiteralPath $root) {
                        throw 'fresh install root still exists after rollback'
                    }
                }.GetNewClosure()
                Invoke-AfterBrokerTaskAbsence `
                    -Name $Name -Action $removeRootAction
            } catch { $failure += "; filesystem rollback failed: $($_.Exception.Message)" }
        }
        throw "Broker installation failed; fresh-install rollback was enforced: $failure"
    }
}

function Uninstall-Broker {
    param([string]$Root, [string]$Requests, [string]$Name, [string]$Account)
    $userSid = Resolve-AccountSid -Account $Account -Label 'Target user'
    [void](Assert-InstallAuthority -ExpectedAccount $Account -ExpectedSid $userSid)
    $root = Assert-RealDirectory -Path $Root -Label 'Broker install root'
    $requests = Assert-RealDirectory -Path $Requests -Label 'Broker request root'
    $receipt = Read-Receipt -Root $root -Requests $requests -Name $Name `
        -SkipTaskValidation
    if ([string]$receipt.user_sid -cne $userSid -or
        [string]$receipt.task_name -cne $Name -or
        [string]$receipt.request_root -cne $requests) {
        throw 'Uninstall arguments do not match the protected install receipt.'
    }
    $workerLockPath = Join-Path $root 'worker.lock'
    $workerBinding = $null
    if (-not (Test-BrokerWorkerLockReleased -Path $workerLockPath)) {
        $workerBinding = Get-ReceiptBoundWorkerProcess `
            -Root $root -Receipt $receipt
    }
    $queryXmlAction = {
        param([string]$TaskName)
        Get-TaskXmlSnapshot -Name $TaskName
    }
    $validateTaskAction = {
        param([string]$TaskXml)
        Assert-BrokerTaskXmlBinding -Xml $TaskXml `
            -PowerShellPath ([string]$receipt.powershell_path) `
            -WorkerPath ([string]$receipt.worker_path) `
            -BrokerRoot $root -Requests $requests `
            -Account ([string]$receipt.user_account) `
            -UserSid ([string]$receipt.user_sid)
        Assert-BrokerTaskSecurity -Name $Name `
            -UserSid ([string]$receipt.user_sid) `
            -SandboxSid ([string]$receipt.sandbox_group_sid)
    }.GetNewClosure()
    $removeTaskAction = {
        param([string]$TaskName)
        [void](Remove-BrokerTaskStrict -Name $TaskName)
    }
    $removeRootAction = {
        if (Test-Path -LiteralPath $root -PathType Container) {
            Confirm-BrokerWorkerStoppedForRootRemoval `
                -LockPath $workerLockPath -Binding $workerBinding
            Wait-BrokerWorkerLockReleased -Path $workerLockPath
            Remove-Item -LiteralPath $root -Recurse -Force -ErrorAction Stop
        }
    }.GetNewClosure()
    try {
        Invoke-BrokerUninstallDestructivePhase -Name $Name `
            -QueryXmlAction $queryXmlAction `
            -ValidateTaskAction $validateTaskAction `
            -RemoveTaskAction $removeTaskAction `
            -RemoveRootAction $removeRootAction
    } finally {
        if ($null -ne $workerBinding) {
            $workerBinding.Process.Dispose()
        }
    }
    if (Test-Path -LiteralPath $root) {
        throw 'Broker uninstall did not remove both the receipt-bound task and install root.'
    }
    [void](Assert-BrokerTaskAbsent -Name $Name)
    return [ordered]@{
        uninstalled = $true
        task_name = $Name
        install_root_removed = -not (Test-Path -LiteralPath $root)
        request_root_removed = -not (Test-Path -LiteralPath $requests)
    }
}

function Recover-PartialUninstall {
    param(
        [string]$Root,
        [string]$Requests,
        [string]$Name,
        [string]$Account,
        [string]$Group
    )
    $userSid = Resolve-AccountSid -Account $Account -Label 'Target user'
    $sandboxSid = Resolve-AccountSid -Account $Group -Label 'Sandbox group'
    [void](Assert-InstallAuthority -ExpectedAccount $Account -ExpectedSid $userSid)
    $root = Assert-RealDirectory -Path $Root -Label 'Partial uninstall root'
    $expectedRequests = Join-Path $root 'requests'
    if ($Requests -cne $expectedRequests) {
        throw "Request root must be the protected install-root child: $expectedRequests"
    }
    if ($Name -notmatch '^\\[A-Za-z0-9._-]+$') {
        throw "Broker task name must be one fixed root task: $Name"
    }
    [void](Assert-BrokerTaskAbsent -Name $Name)
    $rootSecurity = New-ManagedDirectorySecurity `
        -UserSid $userSid -SandboxSid $sandboxSid -Kind ReadOnly
    $fileSecurity = New-ManagedFileSecurity `
        -UserSid $userSid -SandboxSid $sandboxSid
    $remnant = Assert-PartialUninstallRemnant `
        -Root $root -ExpectedRootSecurity $rootSecurity `
        -ExpectedFileSecurity $fileSecurity -UserSid $userSid
    Wait-BrokerWorkerLockReleased -Path $remnant.WorkerLock
    $removeRootAction = {
        $current = Assert-PartialUninstallRemnant `
            -Root $root -ExpectedRootSecurity $rootSecurity `
            -ExpectedFileSecurity $fileSecurity -UserSid $userSid
        Wait-BrokerWorkerLockReleased -Path $current.WorkerLock
        Remove-Item -LiteralPath $root -Recurse -Force -ErrorAction Stop
        if (Test-Path -LiteralPath $root) {
            throw 'Partial uninstall root still exists after recovery.'
        }
    }.GetNewClosure()
    Invoke-AfterBrokerTaskAbsence -Name $Name -Action $removeRootAction
    return [ordered]@{
        recovered_partial_uninstall = $true
        task_name = $Name
        install_root_removed = $true
    }
}

function Invoke-SelfTest {
    $errors = $null
    [void][Management.Automation.Language.Parser]::ParseFile(
        $PSCommandPath, [ref]$null, [ref]$errors
    )
    if (@($errors).Count -ne 0) {
        throw "Installer has PowerShell parse errors: $(@($errors) -join '; ')"
    }
    $workerErrors = $null
    [void][Management.Automation.Language.Parser]::ParseFile(
        $script:BrokerSource, [ref]$null, [ref]$workerErrors
    )
    if (@($workerErrors).Count -ne 0) {
        throw "Broker worker has PowerShell parse errors: $(@($workerErrors) -join '; ')"
    }
    $testWorkerPath = 'C:\ProgramData\Rayman\CodexPowerShellBroker\versions\abc\codex-powershell-broker.' +
        'ps1'
    $xml = New-BrokerTaskXml `
        -PowerShellPath 'C:\Program Files\PowerShell\7\pwsh.exe' `
        -WorkerPath $testWorkerPath `
        -BrokerRoot 'C:\ProgramData\Rayman\CodexPowerShellBroker' `
        -Requests 'C:\ProgramData\Rayman\CodexPowerShellBroker\requests' `
        -Account 'QIN5521\qinrm' `
        -Name '\Rayman-CodexPowerShellBroker'
    $taskBindingArguments = @{
        PowerShellPath = 'C:\Program Files\PowerShell\7\pwsh.exe'
        WorkerPath = $testWorkerPath
        BrokerRoot = 'C:\ProgramData\Rayman\CodexPowerShellBroker'
        Requests = 'C:\ProgramData\Rayman\CodexPowerShellBroker\requests'
        Account = 'QIN5521\qinrm'
        UserSid = 'S-1-5-21-1-2-3-1001'
    }
    Assert-BrokerTaskXmlBinding -Xml $xml @taskBindingArguments
    $logonEnabledPrefix = '<LogonTrigger><Enabled>true</Enabled><UserId>'
    $settingsEnabledPrefix = '<Enabled>true</Enabled><Hidden>true</Hidden>'
    $runLevelElement = '<RunLevel>LeastPrivilege</RunLevel>'
    if (-not $xml.Contains($logonEnabledPrefix) -or
        -not $xml.Contains($settingsEnabledPrefix) -or
        -not $xml.Contains($runLevelElement)) {
        throw 'Task XML self-test fixture lost an explicit schema-default value.'
    }
    $schedulerMaterializedXml = $xml.Replace(
        $logonEnabledPrefix,
        '<LogonTrigger><UserId>'
    ).Replace(
        $settingsEnabledPrefix,
        '<Hidden>true</Hidden>'
    ).Replace(
        $runLevelElement,
        ''
    ).Replace(
        '<Principal id="Author"><UserId>QIN5521\qinrm</UserId>',
        '<Principal id="Author"><UserId>S-1-5-21-1-2-3-1001</UserId>'
    )
    Assert-BrokerTaskXmlBinding -Xml $schedulerMaterializedXml @taskBindingArguments
    $allSidMaterializedXml = $schedulerMaterializedXml.Replace(
        '<LogonTrigger><UserId>QIN5521\qinrm</UserId>',
        '<LogonTrigger><UserId>S-1-5-21-1-2-3-1001</UserId>'
    )
    Assert-BrokerTaskXmlBinding -Xml $allSidMaterializedXml @taskBindingArguments
    $invalidTaskXml = [ordered]@{
        logon_explicitly_disabled = $xml.Replace(
            $logonEnabledPrefix,
            '<LogonTrigger><Enabled>false</Enabled><UserId>'
        )
        logon_duplicate_enabled = $xml.Replace(
            $logonEnabledPrefix,
            '<LogonTrigger><Enabled>true</Enabled><Enabled>true</Enabled><UserId>'
        )
        logon_foreign_enabled = $xml.Replace(
            $logonEnabledPrefix,
            '<LogonTrigger><Enabled xmlns="">true</Enabled><UserId>'
        )
        additional_trigger = $xml.Replace(
            '</LogonTrigger></Triggers>',
            '</LogonTrigger><TimeTrigger><StartBoundary>2026-01-01T00:00:00</StartBoundary></TimeTrigger></Triggers>'
        )
        settings_explicitly_disabled = $xml.Replace(
            $settingsEnabledPrefix,
            '<Enabled>false</Enabled><Hidden>true</Hidden>'
        )
        settings_duplicate_enabled = $xml.Replace(
            $settingsEnabledPrefix,
            '<Enabled>true</Enabled><Enabled>true</Enabled><Hidden>true</Hidden>'
        )
        additional_principal = $xml.Replace(
            '</Principal></Principals>',
            '</Principal><Principal id="Other"><UserId>QIN5521\other</UserId></Principal></Principals>'
        )
        additional_exec = $xml.Replace(
            '</Exec></Actions>',
            '</Exec><Exec><Command>cmd.exe</Command></Exec></Actions>'
        )
        run_level_elevated = $xml.Replace(
            $runLevelElement,
            '<RunLevel>HighestAvailable</RunLevel>'
        )
        run_level_duplicate = $xml.Replace(
            $runLevelElement,
            '<RunLevel>LeastPrivilege</RunLevel><RunLevel>LeastPrivilege</RunLevel>'
        )
        run_level_foreign = $xml.Replace(
            $runLevelElement,
            '<RunLevel xmlns="urn:foreign">LeastPrivilege</RunLevel>'
        )
        run_level_invalid_value = $xml.Replace(
            $runLevelElement,
            '<RunLevel>leastPrivilege</RunLevel>'
        )
        duplicate_logon_user_id = $xml.Replace(
            '<UserId>QIN5521\qinrm</UserId></LogonTrigger>',
            '<UserId>QIN5521\qinrm</UserId><UserId>QIN5521\qinrm</UserId></LogonTrigger>'
        )
        foreign_duplicate_logon_user_id = $xml.Replace(
            '<UserId>QIN5521\qinrm</UserId></LogonTrigger>',
            '<UserId>QIN5521\qinrm</UserId><UserId xmlns="urn:foreign">QIN5521\other</UserId></LogonTrigger>'
        )
        foreign_only_logon_user_id = $xml.Replace(
            '<UserId>QIN5521\qinrm</UserId></LogonTrigger>',
            '<UserId xmlns="urn:foreign">QIN5521\qinrm</UserId></LogonTrigger>'
        )
        duplicate_principal_user_id = $xml.Replace(
            '<Principal id="Author"><UserId>QIN5521\qinrm</UserId>',
            '<Principal id="Author"><UserId>QIN5521\qinrm</UserId><UserId>QIN5521\qinrm</UserId>'
        )
        foreign_duplicate_principal_user_id = $xml.Replace(
            '<Principal id="Author"><UserId>QIN5521\qinrm</UserId>',
            '<Principal id="Author"><UserId>QIN5521\qinrm</UserId><UserId xmlns="urn:foreign">QIN5521\other</UserId>'
        )
        foreign_only_principal_user_id = $xml.Replace(
            '<Principal id="Author"><UserId>QIN5521\qinrm</UserId>',
            '<Principal id="Author"><UserId xmlns="urn:foreign">QIN5521\qinrm</UserId>'
        )
        wrong_principal_sid = $schedulerMaterializedXml.Replace(
            '<Principal id="Author"><UserId>S-1-5-21-1-2-3-1001</UserId>',
            '<Principal id="Author"><UserId>S-1-5-21-1-2-3-9999</UserId>'
        )
        wrong_logon_sid = $allSidMaterializedXml.Replace(
            '<LogonTrigger><UserId>S-1-5-21-1-2-3-1001</UserId>',
            '<LogonTrigger><UserId>S-1-5-21-1-2-3-9999</UserId>'
        )
        unresolvable_principal_identity = $xml.Replace(
            '<Principal id="Author"><UserId>QIN5521\qinrm</UserId>',
            '<Principal id="Author"><UserId>RAYMAN-NO-SUCH-DOMAIN\missing-broker-user</UserId>'
        )
    }
    foreach ($taskCase in $invalidTaskXml.GetEnumerator()) {
        $accepted = $false
        try {
            Assert-BrokerTaskXmlBinding `
                -Xml ([string]$taskCase.Value) @taskBindingArguments
            $accepted = $true
        } catch { }
        if ($accepted) {
            throw "Task XML self-test accepted invalid case: $($taskCase.Key)"
        }
    }

    $assertRejected = {
        param([scriptblock]$Action, [string]$Label)
        $accepted = $false
        try {
            & $Action
            $accepted = $true
        } catch { }
        if ($accepted) { throw "Self-test accepted injected failure: $Label" }
    }
    $newTaskFolder = {
        param(
            [string]$Mode,
            [object[]]$Tasks,
            [bool]$EnumerationFails = $false
        )
        $folder = [pscustomobject]@{
            Mode = $Mode
            Tasks = @($Tasks)
            EnumerationFails = $EnumerationFails
            EnumerationCalls = 0
        }
        $folder | Add-Member -MemberType ScriptMethod -Name GetTask -Value {
            param([string]$TaskName)
            switch ($this.Mode) {
                'present' { return $this.Tasks[0] }
                'missing_file' {
                    throw [IO.FileNotFoundException]::new('injected task-not-found')
                }
                'missing_path' {
                    throw [IO.DirectoryNotFoundException]::new('injected task-path-not-found')
                }
                'denied' {
                    throw [UnauthorizedAccessException]::new('injected task access denial')
                }
                'generic' {
                    throw [Runtime.InteropServices.COMException]::new(
                        'injected Task Scheduler failure', -2147467259
                    )
                }
                'service_stopped' {
                    throw [Runtime.InteropServices.COMException]::new(
                        'injected Task Scheduler service failure', -2147216619
                    )
                }
                'cannot_open_task' {
                    throw [Runtime.InteropServices.COMException]::new(
                        'injected cannot-open-task failure', -2147216627
                    )
                }
                'invalid_task' {
                    throw [Runtime.InteropServices.COMException]::new(
                        'injected invalid-task failure', -2147216626
                    )
                }
                'malformed_xml' {
                    throw [Runtime.InteropServices.COMException]::new(
                        'injected malformed-task-XML failure', -2147216614
                    )
                }
                'null' { return $null }
                default { throw "Unknown injected task query mode: $($this.Mode)" }
            }
        }
        $folder | Add-Member -MemberType ScriptMethod -Name GetTasks -Value {
            param([int]$Flags)
            $this.EnumerationCalls++
            if ($Flags -ne 1) { throw 'Task enumeration did not request hidden tasks.' }
            if ($this.EnumerationFails) {
                throw [UnauthorizedAccessException]::new(
                    'injected task enumeration access denial'
                )
            }
            $collection = [pscustomobject]@{
                Items = @($this.Tasks)
                Count = @($this.Tasks).Count
            }
            $collection | Add-Member -MemberType ScriptMethod -Name Item -Value {
                param([int]$Index)
                return $this.Items[$Index - 1]
            }
            return $collection
        }
        return $folder
    }
    $testTaskName = '\Rayman-CodexPowerShellBroker'
    $wrappedNotFound = [InvalidOperationException]::new(
        'injected wrapper', [IO.FileNotFoundException]::new('injected missing task')
    )
    if (-not (Test-TaskNotFoundException `
            -Exception ([IO.FileNotFoundException]::new())) -or
        -not (Test-TaskNotFoundException `
            -Exception ([IO.DirectoryNotFoundException]::new())) -or
        -not (Test-TaskNotFoundException -Exception $wrappedNotFound) -or
        (Test-TaskNotFoundException `
            -Exception ([UnauthorizedAccessException]::new()))) {
        throw 'Task not-found HRESULT classifier self-test failed.'
    }
    $presentTask = [pscustomobject]@{ Path = $testTaskName; Xml = '<Task />' }
    $presentFolder = & $newTaskFolder 'present' @($presentTask)
    $resolvedTask = Resolve-BrokerTaskFromFolder `
        -Folder $presentFolder -Name $testTaskName
    if (-not [object]::ReferenceEquals($resolvedTask, $presentTask) -or
        $presentFolder.EnumerationCalls -ne 0) {
        throw 'Task query self-test did not preserve the exact successful lookup.'
    }
    foreach ($missingMode in @('missing_file', 'missing_path')) {
        $missingFolder = & $newTaskFolder $missingMode @()
        $missingTask = Resolve-BrokerTaskFromFolder `
            -Folder $missingFolder -Name $testTaskName
        if ($null -ne $missingTask -or $missingFolder.EnumerationCalls -ne 1) {
            throw "Task query self-test did not positively confirm absence: $missingMode"
        }
    }
    foreach ($queryFailureMode in @(
        'denied', 'generic', 'service_stopped', 'cannot_open_task',
        'invalid_task', 'malformed_xml', 'null'
    )) {
        $failureFolder = & $newTaskFolder $queryFailureMode @()
        & $assertRejected {
            [void](Resolve-BrokerTaskFromFolder `
                -Folder $failureFolder -Name $testTaskName)
        } "task query $queryFailureMode"
    }
    $enumerationFailureFolder = & $newTaskFolder 'missing_file' @() $true
    & $assertRejected {
        [void](Resolve-BrokerTaskFromFolder `
            -Folder $enumerationFailureFolder -Name $testTaskName)
    } 'not-found followed by failed absence confirmation'
    $inconsistentFolder = & $newTaskFolder 'missing_file' @($presentTask)
    & $assertRejected {
        [void](Resolve-BrokerTaskFromFolder `
            -Folder $inconsistentFolder -Name $testTaskName)
    } 'not-found contradicted by task enumeration'
    $wrongTask = [pscustomobject]@{ Path = '\OtherTask'; Xml = '<Task />' }
    $wrongTaskFolder = & $newTaskFolder 'present' @($wrongTask)
    & $assertRejected {
        [void](Resolve-BrokerTaskFromFolder `
            -Folder $wrongTaskFolder -Name $testTaskName)
    } 'wrong task returned by Task Scheduler'
    & $assertRejected {
        [void](Resolve-BrokerTaskFromFolder `
            -Folder $presentFolder -Name '\Nested\Task')
    } 'non-root task name'
    if ((Read-BrokerTaskXmlSnapshot -Task $presentTask -Name $testTaskName) -cne
        '<Task />') {
        throw 'Task XML query self-test did not preserve exact XML.'
    }
    & $assertRejected {
        [void](Read-BrokerTaskXmlSnapshot `
            -Task ([pscustomobject]@{ Path = $testTaskName; Xml = '' }) `
            -Name $testTaskName)
    } 'empty Task Scheduler XML'
    $failingXmlTask = [pscustomobject]@{ Path = $testTaskName }
    $failingXmlTask | Add-Member -MemberType ScriptProperty -Name Xml -Value {
        throw [UnauthorizedAccessException]::new('injected XML access denial')
    }
    & $assertRejected {
        [void](Read-BrokerTaskXmlSnapshot `
            -Task $failingXmlTask -Name $testTaskName)
    } 'Task Scheduler XML query failure'

    $absentLookup = {
        param([string]$TaskName)
        [pscustomobject]@{ Task = $null }
    }
    $presentLookup = {
        param([string]$TaskName)
        [pscustomobject]@{ Task = $presentTask }
    }.GetNewClosure()
    $failingLookup = {
        param([string]$TaskName)
        throw [UnauthorizedAccessException]::new(
            'injected absence-confirmation failure'
        )
    }
    $protectedFilesPresent = $true
    $taskSafeForFileRemoval = $false
    try {
        $taskSafeForFileRemoval = Confirm-BrokerTaskSafeForFileRemoval `
            -TaskRegistered $true -Name $testTaskName -RemoveTaskAction {
                throw [UnauthorizedAccessException]::new(
                    'injected removal verification failure'
                )
            } -LookupAction $absentLookup
    } catch { }
    if ($taskSafeForFileRemoval) {
        $protectedFilesPresent = $false
    }
    if (-not $protectedFilesPresent) {
        throw 'Rollback self-test deleted files after an unconfirmed task removal.'
    }
    $protectedFilesPresent = $true
    $taskSafeForFileRemoval = $false
    try {
        $taskSafeForFileRemoval = Confirm-BrokerTaskSafeForFileRemoval `
            -TaskRegistered $true -Name $testTaskName `
            -RemoveTaskAction { param([string]$TaskName) } `
            -LookupAction $failingLookup
    } catch { }
    if ($taskSafeForFileRemoval) { $protectedFilesPresent = $false }
    if (-not $protectedFilesPresent) {
        throw 'Rollback self-test deleted files after a failed absence confirmation.'
    }
    if (-not (Confirm-BrokerTaskSafeForFileRemoval `
        -TaskRegistered $false -Name $testTaskName -RemoveTaskAction {
            throw 'Removal action must not run before a task was registered.'
        } -LookupAction $absentLookup)) {
        throw 'Rollback self-test rejected the no-task-registered state.'
    }
    $removedTaskNames = [Collections.Generic.List[string]]::new()
    if (-not (Confirm-BrokerTaskSafeForFileRemoval `
        -TaskRegistered $true -Name $testTaskName -RemoveTaskAction {
            param([string]$TaskName)
            $removedTaskNames.Add($TaskName)
        } -LookupAction $absentLookup) -or $removedTaskNames.Count -ne 1 -or
        $removedTaskNames[0] -cne $testTaskName) {
        throw 'Rollback self-test lost the confirmed task-removal path.'
    }
    & $assertRejected {
        [void](Confirm-BrokerTaskSafeForFileRemoval `
            -TaskRegistered $false -Name $testTaskName `
            -LookupAction $presentLookup)
    } 'task reappeared before rollback file removal'

    foreach ($blockedLookup in @($failingLookup, $presentLookup)) {
        $destructivePhase = [pscustomobject]@{ RemoveRootCalls = 0 }
        try {
            Invoke-AfterBrokerTaskAbsence -Name $testTaskName `
                -LookupAction $blockedLookup -Action {
                    $destructivePhase.RemoveRootCalls++
                }
        } catch { }
        if ($destructivePhase.RemoveRootCalls -ne 0) {
            throw 'Uninstall self-test reached root deletion without confirmed task absence.'
        }
    }
    $confirmedDestructivePhase = [pscustomobject]@{ RemoveRootCalls = 0 }
    Invoke-AfterBrokerTaskAbsence -Name $testTaskName `
        -LookupAction $absentLookup -Action {
            $confirmedDestructivePhase.RemoveRootCalls++
        }
    if ($confirmedDestructivePhase.RemoveRootCalls -ne 1) {
        throw 'Uninstall self-test lost the confirmed root-deletion path.'
    }

    $assertUninstallPhaseBlocked = {
        param(
            [scriptblock]$QueryXmlAction,
            [scriptblock]$ValidateTaskAction,
            [scriptblock]$RemoveTaskAction,
            [scriptblock]$LookupAction,
            [string]$Label
        )
        $phase = [pscustomobject]@{ RemoveRootCalls = 0 }
        $removeRootAction = { $phase.RemoveRootCalls++ }.GetNewClosure()
        $rejected = $false
        try {
            Invoke-BrokerUninstallDestructivePhase -Name $testTaskName `
                -QueryXmlAction $QueryXmlAction `
                -ValidateTaskAction $ValidateTaskAction `
                -RemoveTaskAction $RemoveTaskAction `
                -LookupAction $LookupAction `
                -RemoveRootAction $removeRootAction
        } catch { $rejected = $true }
        if (-not $rejected -or $phase.RemoveRootCalls -ne 0) {
            throw "Uninstall destructive phase did not fail closed: $Label"
        }
    }
    & $assertUninstallPhaseBlocked `
        { param([string]$TaskName); throw 'injected initial task query failure' } `
        { param([string]$TaskXml) } `
        { param([string]$TaskName) } `
        $absentLookup 'initial task query failure'
    & $assertUninstallPhaseBlocked `
        { param([string]$TaskName); '<Task />' } `
        { param([string]$TaskXml); throw 'injected task contract failure' } `
        { param([string]$TaskName) } `
        $absentLookup 'task XML or security validation failure'
    & $assertUninstallPhaseBlocked `
        { param([string]$TaskName); '<Task />' } `
        { param([string]$TaskXml) } `
        {
            param([string]$TaskName)
            throw 'injected post-delete task verification failure'
        } `
        $absentLookup 'task deletion verification failure'
    & $assertUninstallPhaseBlocked `
        { param([string]$TaskName); '<Task />' } `
        { param([string]$TaskXml) } `
        { param([string]$TaskName) } `
        $failingLookup 'final absence query failure'
    & $assertUninstallPhaseBlocked `
        { param([string]$TaskName); return $null } `
        { param([string]$TaskXml) } `
        { param([string]$TaskName) } `
        $presentLookup 'task reappeared before root deletion'

    $uninstallPhase = [pscustomobject]@{
        QueryCalls = 0
        ValidateCalls = 0
        RemoveTaskCalls = 0
        LookupCalls = 0
        RemoveRootCalls = 0
    }
    $queryPresentAction = {
        param([string]$TaskName)
        $uninstallPhase.QueryCalls++
        return '<Task />'
    }.GetNewClosure()
    $validatePresentAction = {
        param([string]$TaskXml)
        $uninstallPhase.ValidateCalls++
    }.GetNewClosure()
    $removePresentAction = {
        param([string]$TaskName)
        $uninstallPhase.RemoveTaskCalls++
    }.GetNewClosure()
    $lookupAfterRemovalAction = {
        param([string]$TaskName)
        $uninstallPhase.LookupCalls++
        return [pscustomobject]@{ Task = $null }
    }.GetNewClosure()
    $removeConfirmedRootAction = {
        $uninstallPhase.RemoveRootCalls++
    }.GetNewClosure()
    Invoke-BrokerUninstallDestructivePhase -Name $testTaskName `
        -QueryXmlAction $queryPresentAction `
        -ValidateTaskAction $validatePresentAction `
        -RemoveTaskAction $removePresentAction `
        -LookupAction $lookupAfterRemovalAction `
        -RemoveRootAction $removeConfirmedRootAction
    if ($uninstallPhase.QueryCalls -ne 1 -or
        $uninstallPhase.ValidateCalls -ne 1 -or
        $uninstallPhase.RemoveTaskCalls -ne 1 -or
        $uninstallPhase.LookupCalls -ne 1 -or
        $uninstallPhase.RemoveRootCalls -ne 1) {
        throw 'Uninstall destructive phase lost its present-task ordering contract.'
    }
    $absentUninstallPhase = [pscustomobject]@{
        ValidateCalls = 0
        RemoveTaskCalls = 0
        RemoveRootCalls = 0
    }
    $alreadyAbsentValidateAction = {
        param([string]$TaskXml)
        $absentUninstallPhase.ValidateCalls++
    }.GetNewClosure()
    $alreadyAbsentRemoveTaskAction = {
        param([string]$TaskName)
        $absentUninstallPhase.RemoveTaskCalls++
    }.GetNewClosure()
    $alreadyAbsentRemoveRootAction = {
        $absentUninstallPhase.RemoveRootCalls++
    }.GetNewClosure()
    Invoke-BrokerUninstallDestructivePhase -Name $testTaskName `
        -QueryXmlAction { param([string]$TaskName); return $null } `
        -ValidateTaskAction $alreadyAbsentValidateAction `
        -RemoveTaskAction $alreadyAbsentRemoveTaskAction `
        -LookupAction $absentLookup `
        -RemoveRootAction $alreadyAbsentRemoveRootAction
    if ($absentUninstallPhase.ValidateCalls -ne 0 -or
        $absentUninstallPhase.RemoveTaskCalls -ne 0 -or
        $absentUninstallPhase.RemoveRootCalls -ne 1) {
        throw 'Uninstall destructive phase lost its already-absent contract.'
    }

    $taskSecurity = Get-ExpectedTaskSecurityDescriptor `
        -UserSid 'S-1-5-21-1-2-3-1001' `
        -SandboxSid 'S-1-5-21-1-2-3-1021'
    if (-not $taskSecurity.Contains('D:P') -or
        -not $taskSecurity.Contains('(A;;GA;;;') -or
        -not $taskSecurity.Contains('(A;;GR;;;')) {
        throw 'Task security setter self-test lost its protected generic-rights contract.'
    }
    $testUserSid = 'S-1-5-21-1-2-3-1001'
    $testSandboxSid = 'S-1-5-21-1-2-3-1021'
    $materializedTaskSecurity = "O:${testUserSid}G:BAD:" +
        '(A;;FA;;;SY)' +
        '(A;;FA;;;BA)' +
        "(A;;FA;;;${testUserSid})" +
        "(A;;FR;;;${testSandboxSid})"
    Assert-TaskSecurityDescriptorContract -Sddl $materializedTaskSecurity `
        -UserSid $testUserSid -SandboxSid $testSandboxSid `
        -Label 'Task Scheduler materialized self-test ACL'
    foreach ($invalid in @(
        $materializedTaskSecurity.Replace("G:BA", ''),
        $materializedTaskSecurity.Replace("O:${testUserSid}", 'O:SY'),
        ($materializedTaskSecurity + '(A;;FR;;;BU)'),
        $materializedTaskSecurity.Replace(
            "(A;;FR;;;${testSandboxSid})",
            "(A;;FW;;;${testSandboxSid})"
        ),
        $materializedTaskSecurity.Replace(
            "(A;;FR;;;${testSandboxSid})",
            "(A;ID;FR;;;${testSandboxSid})"
        )
    )) {
        $accepted = $false
        try {
            Assert-TaskSecurityDescriptorContract -Sddl $invalid `
                -UserSid $testUserSid -SandboxSid $testSandboxSid `
                -Label 'Invalid Task Scheduler self-test ACL'
            $accepted = $true
        } catch { }
        if ($accepted) {
            throw "Task security self-test accepted a descriptor drift: $invalid"
        }
    }
    foreach ($required in @(
        '<LogonType>InteractiveToken</LogonType>',
        '<RunLevel>LeastPrivilege</RunLevel>',
        '<MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>',
        '<Hidden>true</Hidden>',
        '<ExecutionTimeLimit>PT0S</ExecutionTimeLimit>',
        '-NoProfile -NonInteractive -WindowStyle Hidden',
        '-Worker',
        '-PollMilliseconds 250'
    )) {
        if (-not $xml.Contains($required)) { throw "Task XML self-test is missing: $required" }
    }
    foreach ($forbidden in @(
        '<RunLevel>HighestAvailable</RunLevel>', ' -Command ',
        'danger-full-access', 'unelevated'
    )) {
        if ($xml.Contains($forbidden)) { throw "Task XML self-test found a forbidden capability: $forbidden" }
    }
    $workerSource = Get-Content -Raw -LiteralPath $script:BrokerSource
    foreach ($required in @(
        "[ValidateSet('identity_probe')]",
        'Arbitrary commands are never accepted.',
        'Open-ExclusiveRequest',
        'Assert-InstalledAclContract',
        'Broker task storage disappeared; worker is stopping.',
        'worker.lock'
    )) {
        if (-not $workerSource.Contains($required)) {
            throw "Broker source self-test is missing: $required"
        }
    }
    $ambiguousSelector = 'Get-Command ' + 'pwsh.exe'
    if ($workerSource.Contains("'Local\RaymanCodexPowerShellBroker-v1'") -or
        (Get-Content -Raw -LiteralPath $PSCommandPath).Contains($ambiguousSelector)) {
        throw 'Broker self-test found a squattable mutex or ambiguous PowerShell selector.'
    }
    $runtime = Get-CurrentPowerShellRuntime
    if ($runtime.Path -cne [Diagnostics.Process]::GetCurrentProcess().MainModule.FileName -or
        $runtime.Sha256 -cne (Get-FileSha256 -Path $runtime.Path)) {
        throw 'Installer did not bind exactly one current PowerShell runtime.'
    }
    $testProcess = [Diagnostics.Process]::GetCurrentProcess()
    $testProcessReceipt = [pscustomobject]@{
        powershell_path = $runtime.Path
        worker_path = 'C:\ProgramData\Rayman\CodexPowerShellBroker\versions\abc\codex-powershell-broker.ps1'
        install_root = 'C:\ProgramData\Rayman\CodexPowerShellBroker'
        request_root = 'C:\ProgramData\Rayman\CodexPowerShellBroker\requests'
        user_sid = [Security.Principal.WindowsIdentity]::GetCurrent().User.Value
    }
    $testProcessHeartbeat = [pscustomobject]@{
        session_id = $testProcess.SessionId
    }
    $testProcessArguments = Get-BrokerTaskArguments `
        -WorkerPath ([string]$testProcessReceipt.worker_path) `
        -BrokerRoot ([string]$testProcessReceipt.install_root) `
        -Requests ([string]$testProcessReceipt.request_root)
    $testProcessCommand = '"' + $runtime.Path + '" ' + $testProcessArguments
    Assert-ReceiptBoundWorkerProcessRecord `
        -Receipt $testProcessReceipt -Heartbeat $testProcessHeartbeat `
        -ExecutablePath $runtime.Path -CommandLine $testProcessCommand `
        -OwnerSid ([string]$testProcessReceipt.user_sid) `
        -SessionId $testProcess.SessionId
    & $assertRejected {
        Assert-ReceiptBoundWorkerProcessRecord `
            -Receipt $testProcessReceipt -Heartbeat $testProcessHeartbeat `
            -ExecutablePath $runtime.Path `
            -CommandLine ($testProcessCommand + ' -Unexpected') `
            -OwnerSid ([string]$testProcessReceipt.user_sid) `
            -SessionId $testProcess.SessionId
    } 'receipt-bound worker command-line drift'
    & $assertRejected {
        Assert-ReceiptBoundWorkerProcessRecord `
            -Receipt $testProcessReceipt -Heartbeat $testProcessHeartbeat `
            -ExecutablePath $runtime.Path -CommandLine $testProcessCommand `
            -OwnerSid 'S-1-5-18' -SessionId $testProcess.SessionId
    } 'receipt-bound worker owner drift'
    & $assertRejected {
        Assert-ReceiptBoundWorkerProcessRecord `
            -Receipt $testProcessReceipt -Heartbeat $testProcessHeartbeat `
            -ExecutablePath $runtime.Path -CommandLine $testProcessCommand `
            -OwnerSid ([string]$testProcessReceipt.user_sid) `
            -SessionId ($testProcess.SessionId + 1)
    } 'receipt-bound worker session drift'

    Confirm-BrokerWorkerStoppedForRootRemoval `
        -LockPath 'C:\fake\worker.lock' `
        -TestLockAction { param([string]$Path); return $true } `
        -StopAction { throw 'Stop action must not run for a released lock.' }
    & $assertRejected {
        Confirm-BrokerWorkerStoppedForRootRemoval `
            -LockPath 'C:\fake\worker.lock' `
            -TestLockAction { param([string]$Path); return $false }
    } 'held worker lock without a receipt-bound process'
    $lockStates = [Collections.Generic.Queue[bool]]::new()
    $lockStates.Enqueue($false)
    $lockStates.Enqueue($true)
    $stopState = [pscustomobject]@{ Calls = 0 }
    $testLockAction = {
        param([string]$Path)
        return $lockStates.Dequeue()
    }.GetNewClosure()
    $testStopAction = {
        param($Binding, [string]$Path)
        $stopState.Calls++
    }.GetNewClosure()
    Confirm-BrokerWorkerStoppedForRootRemoval `
        -LockPath 'C:\fake\worker.lock' `
        -Binding ([pscustomobject]@{ ProcessId = 42 }) `
        -TestLockAction $testLockAction -StopAction $testStopAction
    if ($stopState.Calls -ne 1 -or $lockStates.Count -ne 0) {
        throw 'Receipt-bound orphan-worker stop self-test lost its ordering.'
    }
    $repoRoot = Split-Path -Parent $PSScriptRoot
    $managedRoot = Assert-RealDirectory -Path (Join-Path $repoRoot '.RaymanCodingSkill\tmp') `
        -Label 'Installer self-test managed root'
    $testRoot = Assert-ChildPath `
        -Child (Join-Path $managedRoot (
            'codex-powershell-broker-installer-selftest-' + [Guid]::NewGuid().ToString('N')
        )) `
        -Parent $managedRoot -Label 'Installer self-test root'
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $testUserSid = [string]$identity.User.Value
    $testSandboxSid = 'S-1-5-32-545'
    $readOnlySecurity = New-ManagedDirectorySecurity `
        -UserSid $testUserSid -SandboxSid $testSandboxSid -Kind ReadOnly
    $requestSecurity = New-ManagedDirectorySecurity `
        -UserSid $testUserSid -SandboxSid $testSandboxSid -Kind Requests
    $fileSecurity = New-ManagedFileSecurity `
        -UserSid $testUserSid -SandboxSid $testSandboxSid
    $testCapabilitySid = 'S-1-5-21-9-8-7-6'
    $newManagedRequestSecurity = {
        param(
            [string]$CapabilitySid = $testCapabilitySid,
            [Security.AccessControl.FileSystemRights]$CapabilityRights =
                [Security.AccessControl.FileSystemRights]::Modify,
            [Security.AccessControl.FileSystemRights]$SandboxRights =
                [Security.AccessControl.FileSystemRights]::Modify
        )
        $security = [Security.AccessControl.DirectorySecurity]::new()
        $security.SetAccessRuleProtection($true, $false)
        $security.SetOwner(
            [Security.Principal.SecurityIdentifier]::new($testUserSid)
        )
        $inherit = [Security.AccessControl.InheritanceFlags]'ContainerInherit, ObjectInherit'
        foreach ($sid in @('S-1-5-18', 'S-1-5-32-544', $testUserSid)) {
            Add-ManagedRule -Security $security -Sid $sid `
                -Rights ([Security.AccessControl.FileSystemRights]::FullControl) `
                -Inheritance $inherit
        }
        Add-ManagedRule -Security $security -Sid $testSandboxSid `
            -Rights $SandboxRights -Inheritance $inherit
        Add-ManagedRule -Security $security -Sid $CapabilitySid `
            -Rights $CapabilityRights -Inheritance $inherit
        return $security
    }
    Assert-RequestSecurityDescriptorContract `
        -Security $requestSecurity -ExpectedOwnerSid $testUserSid `
        -SandboxSid $testSandboxSid -Label 'Installer-base request ACL'
    $managedRequestSecurity = & $newManagedRequestSecurity
    Assert-RequestSecurityDescriptorContract `
        -Security $managedRequestSecurity -ExpectedOwnerSid $testUserSid `
        -SandboxSid $testSandboxSid -Label 'Codex-managed request ACL'
    $testUserDomain = (
        [Security.Principal.SecurityIdentifier]::new($testUserSid)
    ).AccountDomainSid.Value
    & $assertRejected {
        $sameDomain = & $newManagedRequestSecurity `
            -CapabilitySid ($testUserDomain + '-4242')
        Assert-RequestSecurityDescriptorContract `
            -Security $sameDomain -ExpectedOwnerSid $testUserSid `
            -SandboxSid $testSandboxSid -Label 'Same-domain extra ACE'
    } 'same-domain request ACL extra ACE'
    & $assertRejected {
        $broadPrincipal = & $newManagedRequestSecurity `
            -CapabilitySid 'S-1-1-0'
        Assert-RequestSecurityDescriptorContract `
            -Security $broadPrincipal -ExpectedOwnerSid $testUserSid `
            -SandboxSid $testSandboxSid -Label 'Broad extra ACE'
    } 'broad-principal request ACL extra ACE'
    & $assertRejected {
        $capabilityFullControl = & $newManagedRequestSecurity `
            -CapabilityRights ([Security.AccessControl.FileSystemRights]::FullControl)
        Assert-RequestSecurityDescriptorContract `
            -Security $capabilityFullControl -ExpectedOwnerSid $testUserSid `
            -SandboxSid $testSandboxSid -Label 'Capability rights drift'
    } 'capability request ACL rights drift'
    & $assertRejected {
        $sandboxFullControl = & $newManagedRequestSecurity `
            -SandboxRights ([Security.AccessControl.FileSystemRights]::FullControl)
        Assert-RequestSecurityDescriptorContract `
            -Security $sandboxFullControl -ExpectedOwnerSid $testUserSid `
            -SandboxSid $testSandboxSid -Label 'Sandbox rights drift'
    } 'sandbox-group request ACL rights drift'
    [void](New-ManagedDirectory -Path $testRoot -Security $readOnlySecurity `
        -OwnerSid $testUserSid -Label 'Installer self-test root')
    try {
        $partialRoot = Join-Path $testRoot 'partial-uninstall'
        [void](New-ManagedDirectory -Path $partialRoot -Security $readOnlySecurity `
            -OwnerSid $testUserSid -Label 'Partial uninstall self-test root')
        $partialLock = Join-Path $partialRoot 'worker.lock'
        Write-BytesAtomic -Path $partialLock -Bytes ([byte[]]::new(0)) `
            -Security $fileSecurity
        $partial = Assert-PartialUninstallRemnant `
            -Root $partialRoot -ExpectedRootSecurity $readOnlySecurity `
            -ExpectedFileSecurity $fileSecurity -UserSid $testUserSid
        if ([string]$partial.WorkerLock -cne $partialLock) {
            throw 'Partial uninstall self-test did not preserve the worker lock path.'
        }
        $heldLock = [IO.FileStream]::new(
            $partialLock, [IO.FileMode]::Open, [IO.FileAccess]::ReadWrite,
            [IO.FileShare]::None
        )
        try {
            & $assertRejected {
                Wait-BrokerWorkerLockReleased `
                    -Path $partialLock -TimeoutMilliseconds 50
            } 'held worker lock timeout'
        } finally {
            $heldLock.Dispose()
        }
        Wait-BrokerWorkerLockReleased -Path $partialLock -TimeoutMilliseconds 50
        $unexpected = Join-Path $partialRoot 'unexpected.txt'
        Write-BytesAtomic -Path $unexpected `
            -Bytes ([Text.UTF8Encoding]::new($false).GetBytes('unexpected')) `
            -Security $fileSecurity
        & $assertRejected {
            [void](Assert-PartialUninstallRemnant `
                -Root $partialRoot -ExpectedRootSecurity $readOnlySecurity `
                -ExpectedFileSecurity $fileSecurity -UserSid $testUserSid)
        } 'partial uninstall root with an extra file'
        Remove-Item -LiteralPath $unexpected -Force
        Write-BytesAtomic -Path $partialLock -Bytes ([byte[]]@(1)) `
            -Replace -Security $fileSecurity
        & $assertRejected {
            [void](Assert-PartialUninstallRemnant `
                -Root $partialRoot -ExpectedRootSecurity $readOnlySecurity `
                -ExpectedFileSecurity $fileSecurity -UserSid $testUserSid)
        } 'partial uninstall root with a non-empty worker lock'
        Write-BytesAtomic -Path $partialLock -Bytes ([byte[]]::new(0)) `
            -Replace -Security $fileSecurity
        [void](Assert-PartialUninstallRemnant `
            -Root $partialRoot -ExpectedRootSecurity $readOnlySecurity `
            -ExpectedFileSecurity $fileSecurity -UserSid $testUserSid)

        $requestTest = Join-Path $testRoot 'requests'
        [void](New-ManagedDirectory -Path $requestTest -Security $requestSecurity `
            -OwnerSid $testUserSid -Label 'Installer self-test request root')
        $target = Join-Path $testRoot 'atomic.txt'
        Write-BytesAtomic -Path $target `
            -Bytes ([Text.UTF8Encoding]::new($false).GetBytes('first')) `
            -Security $fileSecurity
        Write-BytesAtomic -Path $target `
            -Bytes ([Text.UTF8Encoding]::new($false).GetBytes('second')) `
            -Replace -Security $fileSecurity
        if ([IO.File]::ReadAllText($target) -cne 'second') {
            throw 'Installer atomic publication self-test failed.'
        }
        Assert-ExactSecurity -Path $target -Expected $fileSecurity `
            -ExpectedOwnerSid $testUserSid -Label 'Installer self-test file'
        $emptyTarget = Join-Path $testRoot 'worker.lock'
        Write-BytesAtomic -Path $emptyTarget -Bytes ([byte[]]::new(0)) `
            -Security $fileSecurity
        if ((Get-Item -LiteralPath $emptyTarget -Force).Length -ne 0) {
            throw 'Installer zero-byte lock publication self-test failed.'
        }
        Assert-ExactSecurity -Path $emptyTarget -Expected $fileSecurity `
            -ExpectedOwnerSid $testUserSid -Label 'Installer self-test zero-byte lock'
    } finally {
        $verified = Assert-ChildPath -Child $testRoot -Parent $managedRoot `
            -Label 'Installer self-test cleanup root'
        if (Test-Path -LiteralPath $verified -PathType Container) {
            Remove-Item -LiteralPath $verified -Recurse -Force
        }
    }
    Write-Host 'install-codex-powershell-broker.ps1 self-test passed.'
}

if ($SelfTest) { Invoke-SelfTest; return }

$normalizedInstall = Get-NormalizedAbsolutePath -Path $InstallRoot -Label 'Broker install root'
$normalizedRequests = Get-NormalizedAbsolutePath -Path $RequestRoot -Label 'Broker request root'

if ($RecoverPartialUninstall) {
    (Recover-PartialUninstall -Root $normalizedInstall -Requests $normalizedRequests `
        -Name $TaskName -Account $UserAccount -Group $SandboxGroup) |
        ConvertTo-Json -Depth 8
    return
}
if ($Check) {
    (Get-InstallationState -Root $normalizedInstall -Requests $normalizedRequests -Name $TaskName) |
        ConvertTo-Json -Depth 8
    return
}
if ($Install) {
    (Install-Broker -Root $normalizedInstall -Requests $normalizedRequests `
        -Name $TaskName -Account $UserAccount -Group $SandboxGroup) |
        ConvertTo-Json -Depth 8
    return
}
if ($Uninstall) {
    (Uninstall-Broker -Root $normalizedInstall -Requests $normalizedRequests `
        -Name $TaskName -Account $UserAccount) | ConvertTo-Json -Depth 8
}
