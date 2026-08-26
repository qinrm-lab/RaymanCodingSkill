[CmdletBinding(DefaultParameterSetName = 'Invoke')]
param(
    [Parameter(Mandatory = $true, ParameterSetName = 'Invoke')]
    [ValidateSet('identity_probe')]
    [string]$Operation,

    [Parameter(ParameterSetName = 'Invoke')]
    [ValidateRange(1, 120)]
    [int]$TimeoutSeconds = 20,

    [Parameter(Mandatory = $true, ParameterSetName = 'Worker')]
    [switch]$Worker,

    [Parameter(Mandatory = $true, ParameterSetName = 'ProcessOnce')]
    [switch]$ProcessOnce,

    [Parameter(Mandatory = $true, ParameterSetName = 'SelfTest')]
    [switch]$SelfTest,

    [Parameter(ParameterSetName = 'Invoke')]
    [Parameter(ParameterSetName = 'Worker')]
    [Parameter(ParameterSetName = 'ProcessOnce')]
    [string]$InstallRoot = (Join-Path `
        ([Environment]::GetFolderPath('CommonApplicationData')) `
        'Rayman\CodexPowerShellBroker'),

    [Parameter(ParameterSetName = 'Invoke')]
    [Parameter(ParameterSetName = 'Worker')]
    [Parameter(ParameterSetName = 'ProcessOnce')]
    [string]$RequestRoot = (Join-Path `
        ([Environment]::GetFolderPath('CommonApplicationData')) `
        'Rayman\CodexPowerShellBroker\requests'),

    [Parameter(ParameterSetName = 'Worker')]
    [ValidateRange(100, 5000)]
    [int]$PollMilliseconds = 250
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

if ($PSVersionTable.PSVersion.Major -lt 7) {
    throw 'codex-powershell-broker.ps1 requires PowerShell 7+.'
}

$script:SchemaVersion = 2
$script:MaxRequestBytes = 16KB
$script:MaxClockSkewSeconds = 30
$script:MaxLifetimeSeconds = 120
$script:HeartbeatMaximumAgeSeconds = 15
$script:ReceiptName = 'install-receipt.json'
$script:HeartbeatName = 'heartbeat.json'
$script:RequestSuffix = '.request.json'
$script:ResultSuffix = '.result.json'
$script:ResultWriteProbeName = '.codex-sandbox-result-write-probe'

if (-not ('Rayman.CodexBrokerNative' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
using Microsoft.Win32.SafeHandles;

namespace Rayman {
    public static class CodexBrokerNative {
        [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
        public static extern SafeFileHandle CreateFileW(
            string path,
            uint desiredAccess,
            uint shareMode,
            IntPtr securityAttributes,
            uint creationDisposition,
            uint flagsAndAttributes,
            IntPtr templateFile);
    }
}
'@
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

    if (-not [IO.Path]::IsPathRooted($Path)) {
        throw "$Label must be absolute: $Path"
    }
    $full = [IO.Path]::GetFullPath($Path)
    if ($full -eq [IO.Path]::GetPathRoot($full)) {
        throw "$Label must not be a volume root: $full"
    }
    return $full.TrimEnd(
        [IO.Path]::DirectorySeparatorChar,
        [IO.Path]::AltDirectorySeparatorChar
    )
}

function Assert-NoReparseAncestors {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$Label,
        [switch]$RequireLeaf
    )

    $full = Get-NormalizedAbsolutePath -Path $Path -Label $Label
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
        $item = Get-Item -LiteralPath $current -Force -ErrorAction SilentlyContinue
        if ($null -eq $item) {
            if ($RequireLeaf) { throw "$Label is missing: $full" }
            break
        }
        if (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
            throw "$Label must not traverse a reparse point: $current"
        }
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
    )) {
        throw "$Label escaped its authority root: $childFull"
    }
    return $childFull
}

function Write-JsonAtomic {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)]$Document,
        [switch]$Replace,
        [switch]$PassThruHash
    )

    $parent = Split-Path -Parent $Path
    if (-not (Test-Path -LiteralPath $parent -PathType Container)) {
        throw "JSON publication parent is missing: $parent"
    }
    $bytes = [Text.UTF8Encoding]::new($false, $true).GetBytes(
        ($Document | ConvertTo-Json -Depth 16 -Compress)
    )
    $temporary = Join-Path $parent (
        '.' + [IO.Path]::GetFileName($Path) + '.stage-' + [Guid]::NewGuid().ToString('N')
    )
    $stream = [IO.FileStream]::new(
        $temporary,
        [IO.FileMode]::CreateNew,
        [IO.FileAccess]::Write,
        [IO.FileShare]::None,
        4096,
        [IO.FileOptions]::WriteThrough
    )
    try {
        $stream.Write($bytes, 0, $bytes.Length)
        $stream.Flush($true)
    } finally {
        $stream.Dispose()
    }
    try {
        [IO.File]::Move($temporary, $Path, [bool]$Replace)
    } catch {
        Remove-Item -LiteralPath $temporary -Force -ErrorAction SilentlyContinue
        throw
    }
    if ($PassThruHash) { return Get-BytesSha256 -Bytes $bytes }
}

function Read-StrictJsonDocument {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][int64]$MaximumBytes,
        [Parameter(Mandatory = $true)][string]$Label
    )

    $item = Get-Item -LiteralPath $Path -Force -ErrorAction Stop
    if ($item.PSIsContainer -or
        ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0 -or
        $item.Length -le 0 -or $item.Length -gt $MaximumBytes) {
        throw "$Label must be a bounded regular non-reparse file: $Path"
    }
    $bytes = [IO.File]::ReadAllBytes($Path)
    return ConvertFrom-StrictJsonBytes -Bytes $bytes -Label $Label
}

function ConvertFrom-StrictJsonBytes {
    param(
        [Parameter(Mandatory = $true)][byte[]]$Bytes,
        [Parameter(Mandatory = $true)][string]$Label
    )

    try {
        $text = [Text.UTF8Encoding]::new($false, $true).GetString($Bytes)
    } catch {
        throw "$Label is not strict UTF-8: $($_.Exception.Message)"
    }
    if ($text.Contains([char]0)) { throw "$Label contains a NUL byte." }
    try {
        return $text | ConvertFrom-Json `
            -Depth 16 `
            -NoEnumerate `
            -DateKind String `
            -ErrorAction Stop
    } catch {
        throw "$Label is not valid JSON: $($_.Exception.Message)"
    }
}

function Open-ExclusiveRequest {
    param([Parameter(Mandatory = $true)][string]$Path)

    $genericRead = [uint32]2147483648
    $openExisting = [uint32]3
    $openReparsePoint = [uint32]0x00200000
    $sequentialScan = [uint32]0x08000000
    $handle = [Rayman.CodexBrokerNative]::CreateFileW(
        $Path,
        $genericRead,
        0,
        [IntPtr]::Zero,
        $openExisting,
        ($openReparsePoint -bor $sequentialScan),
        [IntPtr]::Zero
    )
    if ($handle.IsInvalid) {
        $errorCode = [Runtime.InteropServices.Marshal]::GetLastWin32Error()
        $handle.Dispose()
        throw [ComponentModel.Win32Exception]::new(
            $errorCode,
            "Cannot claim broker request exclusively: $Path"
        )
    }
    try {
        $attributes = [IO.File]::GetAttributes($handle)
        if (($attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0 -or
            ($attributes -band [IO.FileAttributes]::Directory) -ne 0) {
            throw "Broker request must be a regular non-reparse file: $Path"
        }
        $stream = [IO.FileStream]::new($handle, [IO.FileAccess]::Read)
        $handle = $null
        if ($stream.Length -le 0 -or $stream.Length -gt $script:MaxRequestBytes) {
            $stream.Dispose()
            throw "Broker request must be between 1 and $($script:MaxRequestBytes) bytes: $Path"
        }
        $bytes = [byte[]]::new([int]$stream.Length)
        $offset = 0
        while ($offset -lt $bytes.Length) {
            $read = $stream.Read($bytes, $offset, $bytes.Length - $offset)
            if ($read -le 0) {
                $stream.Dispose()
                throw "Broker request ended before its claimed length: $Path"
            }
            $offset += $read
        }
        return [pscustomobject]@{ Stream = $stream; Bytes = $bytes }
    } catch {
        if ($null -ne $handle) { $handle.Dispose() }
        throw
    }
}

function Assert-ExactProperties {
    param(
        [Parameter(Mandatory = $true)]$Document,
        [Parameter(Mandatory = $true)][AllowEmptyCollection()][string[]]$Expected,
        [Parameter(Mandatory = $true)][string]$Label
    )

    if ($Document -isnot [pscustomobject]) { throw "$Label must be a JSON object." }
    $actual = @(
        $Document.PSObject.Properties |
            ForEach-Object { $_.Name } |
            Sort-Object
    )
    $wanted = @($Expected | Sort-Object)
    if (($actual -join "`n") -cne ($wanted -join "`n")) {
        throw "$Label has unexpected properties. Expected=$($wanted -join ',') Actual=$($actual -join ',')"
    }
}

function Add-ExpectedAccessRule {
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

function New-ExpectedDirectorySecurity {
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
        Add-ExpectedAccessRule -Security $security -Sid $sid `
            -Rights ([Security.AccessControl.FileSystemRights]::FullControl) `
            -Inheritance $inherit
    }
    if ($Kind -eq 'ReadOnly') {
        Add-ExpectedAccessRule -Security $security -Sid $SandboxSid `
            -Rights ([Security.AccessControl.FileSystemRights]::ReadAndExecute) `
            -Inheritance $inherit
    } else {
        $folderRights = [Security.AccessControl.FileSystemRights]::ReadAndExecute -bor `
            [Security.AccessControl.FileSystemRights]::WriteData
        Add-ExpectedAccessRule -Security $security -Sid $SandboxSid -Rights $folderRights
        Add-ExpectedAccessRule -Security $security -Sid $SandboxSid `
            -Rights ([Security.AccessControl.FileSystemRights]::Modify) `
            -Inheritance $inherit `
            -Propagation ([Security.AccessControl.PropagationFlags]::InheritOnly
            )
    }
    return $security
}

function New-ExpectedFileSecurity {
    param(
        [Parameter(Mandatory = $true)][string]$UserSid,
        [Parameter(Mandatory = $true)][string]$SandboxSid
    )

    $security = [Security.AccessControl.FileSecurity]::new()
    $security.SetAccessRuleProtection($true, $false)
    $security.SetOwner([Security.Principal.SecurityIdentifier]::new($UserSid))
    foreach ($sid in @('S-1-5-18', 'S-1-5-32-544', $UserSid)) {
        Add-ExpectedAccessRule -Security $security -Sid $sid `
            -Rights ([Security.AccessControl.FileSystemRights]::FullControl)
    }
    Add-ExpectedAccessRule -Security $security -Sid $SandboxSid `
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

function Assert-InstalledAclContract {
    param(
        [Parameter(Mandatory = $true)]$Paths,
        [Parameter(Mandatory = $true)]$Receipt
    )

    $readOnly = New-ExpectedDirectorySecurity `
        -UserSid ([string]$Receipt.user_sid) `
        -SandboxSid ([string]$Receipt.sandbox_group_sid) -Kind ReadOnly
    $file = New-ExpectedFileSecurity `
        -UserSid ([string]$Receipt.user_sid) `
        -SandboxSid ([string]$Receipt.sandbox_group_sid)
    $versionRoot = Split-Path -Parent ([string]$Receipt.worker_path)
    $versions = Split-Path -Parent $versionRoot
    foreach ($entry in @(
        @($Paths.Install, $readOnly, 'Install root'),
        @($Paths.Results, $readOnly, 'Result root'),
        @($versions, $readOnly, 'Version root'),
        @($versionRoot, $readOnly, 'Worker version directory')
    )) {
        [void](Assert-NoReparseAncestors -Path $entry[0] `
            -Label $entry[2] -RequireLeaf)
        Assert-ExactSecurity -Path $entry[0] -Expected $entry[1] `
            -ExpectedOwnerSid ([string]$Receipt.user_sid) -Label $entry[2]
    }
    [void](Assert-NoReparseAncestors -Path $Paths.Requests `
        -Label 'Request root' -RequireLeaf)
    Assert-RequestRootSecurity -Path $Paths.Requests `
        -ExpectedOwnerSid ([string]$Receipt.user_sid) `
        -SandboxSid ([string]$Receipt.sandbox_group_sid) -Label 'Request root'
    foreach ($entry in @(
        @($Paths.Receipt, 'Install receipt'),
        @([string]$Receipt.worker_path, 'Installed worker'),
        @($Paths.WorkerLock, 'Worker lock')
    )) {
        [void](Assert-NoReparseAncestors -Path $entry[0] `
            -Label $entry[1] -RequireLeaf)
        Assert-ExactSecurity -Path $entry[0] -Expected $file `
            -ExpectedOwnerSid ([string]$Receipt.user_sid) -Label $entry[1]
    }
}

function Assert-TaskSecurityDescriptorContract {
    param(
        [Parameter(Mandatory = $true)][string]$Sddl,
        [Parameter(Mandatory = $true)][string]$UserSid,
        [Parameter(Mandatory = $true)][string]$SandboxSid,
        [Parameter(Mandatory = $true)][string]$Label,
        [switch]$AllowAutoInheritedControl,
        [switch]$RequireProtectedDacl
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
        throw "$Label owner/group mismatch."
    }
    if ($RequireProtectedDacl -and
        ($descriptor.ControlFlags -band
         [Security.AccessControl.ControlFlags]::DiscretionaryAclProtected) -eq 0) {
        throw "$Label must use a protected DACL."
    }
    $forbiddenControl = [Security.AccessControl.ControlFlags]::DiscretionaryAclAutoInheritRequired
    if (-not $AllowAutoInheritedControl) {
        $forbiddenControl = $forbiddenControl -bor `
            [Security.AccessControl.ControlFlags]::DiscretionaryAclAutoInherited
    }
    if (($descriptor.ControlFlags -band $forbiddenControl) -ne 0) {
        throw "$Label must not use an auto-inherited DACL."
    }
    $expected = @(
        @('S-1-5-18', 0x001F01FF),
        @('S-1-5-32-544', 0x001F01FF),
        @($UserSid, 0x001F01FF),
        @($SandboxSid, 0x00120089)
    )
    $acl = $descriptor.DiscretionaryAcl
    if ($null -eq $acl -or $acl.Count -ne $expected.Count) {
        throw "$Label must contain exactly four explicit allow ACEs."
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
            throw "$Label ACE[$index] mismatch."
        }
    }
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

function Get-BrokerTaskFileSnapshot {
    param(
        [Parameter(Mandatory = $true)][string]$Name,
        [string]$TaskRoot = (Join-Path $env:WINDIR 'System32\Tasks')
    )
    if ($Name -notmatch '^\\[A-Za-z0-9._-]+$') {
        throw "Broker task file name must identify one fixed root task: $Name"
    }
    $root = Assert-NoReparseAncestors `
        -Path $TaskRoot -Label 'Task Scheduler storage root' -RequireLeaf
    if (-not (Test-Path -LiteralPath $root -PathType Container)) {
        throw "Task Scheduler storage root is not a directory: $root"
    }
    $path = Assert-ChildPath `
        -Child (Join-Path $root $Name.TrimStart('\')) `
        -Parent $root -Label 'Broker task file'
    $path = Assert-NoReparseAncestors `
        -Path $path -Label 'Broker task file' -RequireLeaf
    $item = Get-Item -LiteralPath $path -Force -ErrorAction Stop
    if ($item.PSIsContainer -or
        ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0 -or
        $item.Length -le 0 -or $item.Length -gt 1MB) {
        throw "Broker task file is not a bounded regular file: $path"
    }
    try { $xml = [IO.File]::ReadAllText($path) }
    catch { throw "Broker task file XML is not readable: $($_.Exception.Message)" }
    if ([string]::IsNullOrWhiteSpace($xml)) {
        throw "Broker task file XML is empty: $path"
    }
    try {
        $sddl = (Get-Acl -LiteralPath $path -ErrorAction Stop).
            GetSecurityDescriptorSddlForm(
                [Security.AccessControl.AccessControlSections]::All
            )
    } catch {
        throw "Broker task file security is not readable: $($_.Exception.Message)"
    }
    return [pscustomobject]@{ Path = $path; Xml = $xml; Sddl = [string]$sddl }
}

function Assert-InstalledTaskContract {
    param(
        [Parameter(Mandatory = $true)]$Paths,
        [Parameter(Mandatory = $true)]$Receipt
    )
    if ([string]$Receipt.task_name -notmatch '^\\[A-Za-z0-9._-]+$') {
        throw 'Broker receipt task name is not one fixed root task.'
    }
    $task = Get-BrokerTaskFileSnapshot -Name ([string]$Receipt.task_name)
    try { [xml]$document = [string]$task.Xml }
    catch { throw "Receipt-bound broker task XML is invalid: $($_.Exception.Message)" }
    $namespace = [Xml.XmlNamespaceManager]::new($document.NameTable)
    $namespace.AddNamespace('t', 'http://schemas.microsoft.com/windows/2004/02/mit/task')
    Assert-TaskXmlStructureContract `
        -Document $document -Namespace $namespace -Label 'Broker task XML'
    $read = {
        param([string]$XPath)
        $nodes = @($document.SelectNodes($XPath, $namespace))
        if ($nodes.Count -ne 1) {
            throw "Broker task XML must contain exactly one $XPath"
        }
        return [string]$nodes[0].InnerText
    }
    $expectedArguments = Get-BrokerTaskArguments `
        -WorkerPath ([string]$Receipt.worker_path) `
        -BrokerRoot $Paths.Install -Requests $Paths.Requests
    $logonTrigger = $document.SelectSingleNode(
        '/t:Task/t:Triggers/t:LogonTrigger', $namespace
    )
    $principal = $document.SelectSingleNode(
        '/t:Task/t:Principals/t:Principal', $namespace
    )
    Assert-TaskUserIdentityBinding `
        -Actual (Read-UniqueTaskUserId `
            -Parent $logonTrigger -Label 'Broker task LogonTrigger') `
        -ExpectedAccount ([string]$Receipt.user_account) `
        -ExpectedSid ([string]$Receipt.user_sid) `
        -Label 'Broker task LogonTrigger UserId'
    Assert-TaskUserIdentityBinding `
        -Actual (Read-UniqueTaskUserId `
            -Parent $principal -Label 'Broker task Principal') `
        -ExpectedAccount ([string]$Receipt.user_account) `
        -ExpectedSid ([string]$Receipt.user_sid) `
        -Label 'Broker task Principal UserId'
    foreach ($check in @(
        @('/t:Task/t:Principals/t:Principal/t:LogonType', 'InteractiveToken'),
        @('/t:Task/t:Settings/t:MultipleInstancesPolicy', 'IgnoreNew'),
        @('/t:Task/t:Settings/t:Hidden', 'true'),
        @('/t:Task/t:Settings/t:ExecutionTimeLimit', 'PT0S'),
        @('/t:Task/t:Actions/t:Exec/t:Command', [string]$Receipt.powershell_path),
        @('/t:Task/t:Actions/t:Exec/t:Arguments', $expectedArguments),
        @('/t:Task/t:Actions/t:Exec/t:WorkingDirectory', $Paths.Install)
    )) {
        $actual = & $read $check[0]
        if ($actual -cne $check[1]) {
            throw "Broker task XML mismatch at $($check[0])."
        }
    }
    Assert-TaskSecurityDescriptorContract -Sddl ([string]$task.Sddl) `
        -UserSid ([string]$Receipt.user_sid) `
        -SandboxSid ([string]$Receipt.sandbox_group_sid) `
        -Label 'Broker task file' `
        -AllowAutoInheritedControl -RequireProtectedDacl
    return $task
}

function Test-AccessDeniedException {
    param([Parameter(Mandatory = $true)]$Exception)
    return $Exception -is [UnauthorizedAccessException] -or
        $Exception -is [Security.SecurityException] -or
        $Exception.InnerException -is [UnauthorizedAccessException] -or
        $Exception.InnerException -is [Security.SecurityException]
}

function Assert-ResultCreateDenied {
    param([Parameter(Mandatory = $true)][string]$ResultRoot)

    $path = Join-Path $ResultRoot $script:ResultWriteProbeName
    if (Test-Path -LiteralPath $path) {
        throw "Result ACL probe path must be absent before verification: $path"
    }
    $stream = $null
    try {
        $stream = [IO.FileStream]::new(
            $path,
            [IO.FileMode]::CreateNew,
            [IO.FileAccess]::Write,
            [IO.FileShare]::None
        )
    } catch {
        if (Test-AccessDeniedException -Exception $_.Exception) { return }
        throw
    } finally {
        if ($null -ne $stream) { $stream.Dispose() }
    }
    Remove-Item -LiteralPath $path -Force -ErrorAction SilentlyContinue
    throw 'Result root unexpectedly allowed the sandbox to create a file.'
}

function Assert-ExistingFileWriteDenied {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$Label
    )

    $stream = $null
    try {
        $stream = [IO.FileStream]::new(
            $Path,
            [IO.FileMode]::Open,
            [IO.FileAccess]::Write,
            [IO.FileShare]::Read
        )
    } catch {
        if (Test-AccessDeniedException -Exception $_.Exception) { return }
        throw
    } finally {
        if ($null -ne $stream) { $stream.Dispose() }
    }
    throw "$Label unexpectedly allowed sandbox write access: $Path"
}

function Assert-RequestDaclWriteDenied {
    param([Parameter(Mandatory = $true)][string]$RequestRoot)

    $writeDac = [uint32]0x00040000
    $shareAll = [uint32]0x00000007
    $openExisting = [uint32]3
    $backupSemantics = [uint32]0x02000000
    $openReparsePoint = [uint32]0x00200000
    $handle = [Rayman.CodexBrokerNative]::CreateFileW(
        $RequestRoot,
        $writeDac,
        $shareAll,
        [IntPtr]::Zero,
        $openExisting,
        ($backupSemantics -bor $openReparsePoint),
        [IntPtr]::Zero
    )
    if ($handle.IsInvalid) {
        $errorCode = [Runtime.InteropServices.Marshal]::GetLastWin32Error()
        $handle.Dispose()
        if ($errorCode -eq 5) { return }
        throw [ComponentModel.Win32Exception]::new(
            $errorCode,
            "Request DACL write probe failed unexpectedly: $RequestRoot"
        )
    }
    $handle.Dispose()
    throw 'Request root unexpectedly allowed the sandbox to open WRITE_DAC.'
}

function Get-BrokerPaths {
    param(
        [Parameter(Mandatory = $true)][string]$Install,
        [Parameter(Mandatory = $true)][string]$Requests
    )

    $installFull = Get-NormalizedAbsolutePath -Path $Install -Label 'Install root'
    $requestFull = Get-NormalizedAbsolutePath -Path $Requests -Label 'Request root'
    $expectedRequests = Join-Path $installFull 'requests'
    if ($requestFull -cne $expectedRequests) {
        throw "Request root must be the protected install-root child: $expectedRequests"
    }
    return [pscustomobject]@{
        Install = $installFull
        Requests = $requestFull
        Results = Join-Path $installFull 'results'
        Receipt = Join-Path $installFull $script:ReceiptName
        Heartbeat = Join-Path (Join-Path $installFull 'results') $script:HeartbeatName
        WorkerLock = Join-Path $installFull 'worker.lock'
    }
}

function Read-BrokerReceipt {
    param([Parameter(Mandatory = $true)]$Paths)

    $receipt = Read-StrictJsonDocument `
        -Path $Paths.Receipt `
        -MaximumBytes 32KB `
        -Label 'Broker install receipt'
    Assert-ExactProperties -Document $receipt -Label 'Broker install receipt' -Expected @(
        'capabilities', 'install_id', 'install_root', 'installed_at_utc',
        'powershell_path', 'powershell_sha256', 'request_root', 'result_root',
        'sandbox_group', 'sandbox_group_sid', 'schema_version', 'task_name',
        'user_account', 'user_sid', 'worker_path', 'worker_sha256'
    )
    if ($receipt.schema_version -ne $script:SchemaVersion -or
        $receipt.capabilities -isnot [array] -or
        @($receipt.capabilities).Count -ne 1 -or
        [string]$receipt.capabilities[0] -cne 'identity_probe') {
        throw 'Broker install receipt has an unsupported schema or capability set.'
    }
    $installId = [Guid]::Empty
    if (-not [Guid]::TryParseExact([string]$receipt.install_id, 'N', [ref]$installId)) {
        throw 'Broker install receipt has a non-canonical install_id.'
    }
    foreach ($sid in @([string]$receipt.user_sid, [string]$receipt.sandbox_group_sid)) {
        try { [void][Security.Principal.SecurityIdentifier]::new($sid) }
        catch { throw "Broker install receipt has an invalid SID: $sid" }
    }
    $receiptInstall = Get-NormalizedAbsolutePath `
        -Path ([string]$receipt.install_root) `
        -Label 'Installed broker root'
    $workerPath = Get-NormalizedAbsolutePath `
        -Path ([string]$receipt.worker_path) `
        -Label 'Installed worker path'
    $requestPath = Get-NormalizedAbsolutePath `
        -Path ([string]$receipt.request_root) `
        -Label 'Installed request root'
    $resultPath = Get-NormalizedAbsolutePath `
        -Path ([string]$receipt.result_root) `
        -Label 'Installed result root'
    if ($receiptInstall -cne $Paths.Install -or
        $requestPath -cne $Paths.Requests -or
        $resultPath -cne $Paths.Results) {
        throw 'Broker install receipt is bound to different queue paths.'
    }
    $workerVersions = Join-Path $Paths.Install 'versions'
    [void](Assert-ChildPath -Child $workerPath -Parent $workerVersions `
        -Label 'Installed worker')
    [void](Assert-NoReparseAncestors -Path $workerPath `
        -Label 'Installed worker' -RequireLeaf)
    if ([IO.Path]::GetFileName($workerPath) -cne 'codex-powershell-broker.ps1' -or
        [IO.Path]::GetFileName((Split-Path -Parent $workerPath)) -cne
            [string]$receipt.worker_sha256) {
        throw 'Installed broker worker path is not version-bound to its receipt hash.'
    }
    if (-not (Test-Path -LiteralPath $workerPath -PathType Leaf) -or
        (Get-FileSha256 -Path $workerPath) -cne [string]$receipt.worker_sha256) {
        throw 'Installed broker worker hash does not match its protected receipt.'
    }
    $powershellPath = Get-NormalizedAbsolutePath `
        -Path ([string]$receipt.powershell_path) -Label 'Installed PowerShell runtime'
    [void](Assert-NoReparseAncestors -Path $powershellPath `
        -Label 'Installed PowerShell runtime' -RequireLeaf)
    if (-not (Test-Path -LiteralPath $powershellPath -PathType Leaf) -or
        (((Get-Item -LiteralPath $powershellPath -Force).Attributes -band
            [IO.FileAttributes]::ReparsePoint) -ne 0) -or
        (Get-FileSha256 -Path $powershellPath) -cne
            [string]$receipt.powershell_sha256) {
        throw 'Installed PowerShell runtime does not match its protected receipt.'
    }
    return $receipt
}

function Get-CurrentIdentityRecord {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $process = [Diagnostics.Process]::GetCurrentProcess()
    return [pscustomobject]@{
        Account = [string]$identity.Name
        Sid = [string]$identity.User.Value
        UserProfile = [string]$env:USERPROFILE
        ProcessId = $PID
        SessionId = $process.SessionId
        PowerShellVersion = [string]$PSVersionTable.PSVersion
        LanguageMode = [string]$ExecutionContext.SessionState.LanguageMode
    }
}

function Assert-WorkerBinding {
    param(
        [Parameter(Mandatory = $true)]$Receipt,
        [Parameter(Mandatory = $true)][string]$WorkerPath
    )

    $identity = Get-CurrentIdentityRecord
    if ($identity.Sid -cne [string]$Receipt.user_sid -or
        $identity.Account -cne [string]$Receipt.user_account) {
        throw "Broker worker identity mismatch. Expected=$($Receipt.user_account)/$($Receipt.user_sid) Actual=$($identity.Account)/$($identity.Sid)"
    }
    if ((Get-FileSha256 -Path $WorkerPath) -cne [string]$Receipt.worker_sha256) {
        throw 'Running broker worker bytes do not match the protected install receipt.'
    }
    $runtime = Get-CurrentPowerShellRuntime
    if ($runtime.Path -cne [string]$Receipt.powershell_path -or
        $runtime.Sha256 -cne [string]$Receipt.powershell_sha256) {
        throw 'Running PowerShell runtime does not match the protected install receipt.'
    }
    return $identity
}

function Assert-ClientSourceBinding {
    param(
        [Parameter(Mandatory = $true)]$Receipt,
        [Parameter(Mandatory = $true)][string]$ClientPath
    )

    if ((Get-FileSha256 -Path $ClientPath) -cne [string]$Receipt.worker_sha256) {
        throw 'Broker client source does not match the installed protected worker.'
    }
}

function Parse-RoundTripTime {
    param(
        [Parameter(Mandatory = $true)][string]$Value,
        [Parameter(Mandatory = $true)][string]$Label
    )

    $parsed = [DateTimeOffset]::MinValue
    if (-not [DateTimeOffset]::TryParseExact(
        $Value,
        'o',
        [Globalization.CultureInfo]::InvariantCulture,
        [Globalization.DateTimeStyles]::RoundtripKind,
        [ref]$parsed
    )) { throw "$Label is not an ISO-8601 round-trip timestamp." }
    return $parsed.ToUniversalTime()
}

function Read-And-ValidateRequest {
    param(
        [Parameter(Mandatory = $true)][byte[]]$Bytes,
        [Parameter(Mandatory = $true)][string]$ExpectedId,
        [Parameter(Mandatory = $true)]$Receipt
    )

    $request = ConvertFrom-StrictJsonBytes -Bytes $Bytes -Label 'Broker request'
    Assert-ExactProperties -Document $request -Label 'Broker request' -Expected @(
        'created_at_utc', 'expires_at_utc', 'install_id', 'operation', 'payload',
        'request_id', 'schema_version'
    )
    if ($request.schema_version -ne $script:SchemaVersion -or
        [string]$request.request_id -cne $ExpectedId -or
        [string]$request.install_id -cne [string]$Receipt.install_id) {
        throw 'Broker request schema or filename binding is invalid.'
    }
    $parsedId = [Guid]::Empty
    if (-not [Guid]::TryParseExact([string]$request.request_id, 'N', [ref]$parsedId)) {
        throw 'Broker request id is not a canonical GUID.'
    }
    $created = Parse-RoundTripTime `
        -Value ([string]$request.created_at_utc) `
        -Label 'created_at_utc'
    $expires = Parse-RoundTripTime `
        -Value ([string]$request.expires_at_utc) `
        -Label 'expires_at_utc'
    $now = [DateTimeOffset]::UtcNow
    if ($created -gt $now.AddSeconds($script:MaxClockSkewSeconds) -or
        $expires -le $now -or
        $expires -le $created -or
        ($expires - $created).TotalSeconds -gt $script:MaxLifetimeSeconds) {
        throw 'Broker request is expired or has an invalid lifetime.'
    }
    if ([string]$request.operation -cne 'identity_probe') {
        throw 'Broker operation is not installed. Arbitrary commands are never accepted.'
    }
    Assert-ExactProperties `
        -Document $request.payload `
        -Expected @() `
        -Label 'identity_probe payload'
    return $request
}

function Invoke-FixedOperation {
    param([Parameter(Mandatory = $true)]$Request)

    if ([string]$Request.operation -cne 'identity_probe') {
        throw 'Broker operation is not installed.'
    }
    $identity = Get-CurrentIdentityRecord
    return [ordered]@{
        account = $identity.Account
        sid = $identity.Sid
        user_profile = $identity.UserProfile
        process_id = $identity.ProcessId
        session_id = $identity.SessionId
        powershell_version = $identity.PowerShellVersion
        language_mode = $identity.LanguageMode
    }
}

function Publish-Heartbeat {
    param(
        [Parameter(Mandatory = $true)]$Paths,
        [Parameter(Mandatory = $true)]$Receipt,
        [Parameter(Mandatory = $true)]$Identity
    )

    Write-JsonAtomic -Path $Paths.Heartbeat -Replace -Document ([ordered]@{
        schema_version = $script:SchemaVersion
        install_id = [string]$Receipt.install_id
        observed_at_utc = [DateTimeOffset]::UtcNow.ToString('o')
        executor_account = $Identity.Account
        executor_sid = $Identity.Sid
        process_id = $Identity.ProcessId
        session_id = $Identity.SessionId
        worker_sha256 = [string]$Receipt.worker_sha256
        powershell_sha256 = [string]$Receipt.powershell_sha256
    })
}

function Publish-RequestResult {
    param(
        [Parameter(Mandatory = $true)]$Paths,
        [Parameter(Mandatory = $true)]$Receipt,
        [Parameter(Mandatory = $true)]$Identity,
        [Parameter(Mandatory = $true)][string]$RequestId,
        [Parameter(Mandatory = $true)][string]$OperationName,
        [Parameter(Mandatory = $true)][string]$RequestSha256,
        [Parameter(Mandatory = $true)][string]$Status,
        [Parameter(Mandatory = $true)][int]$ExitCode,
        $Output,
        [AllowNull()][string]$ErrorMessage,
        [Parameter(Mandatory = $true)][DateTimeOffset]$StartedAt
    )

    $resultPath = Join-Path $Paths.Results ($RequestId + $script:ResultSuffix)
    Write-JsonAtomic -Path $resultPath -Document ([ordered]@{
        schema_version = $script:SchemaVersion
        install_id = [string]$Receipt.install_id
        request_id = $RequestId
        operation = $OperationName
        status = $Status
        exit_code = $ExitCode
        started_at_utc = $StartedAt.ToUniversalTime().ToString('o')
        finished_at_utc = [DateTimeOffset]::UtcNow.ToString('o')
        executor_account = $Identity.Account
        executor_sid = $Identity.Sid
        worker_sha256 = [string]$Receipt.worker_sha256
        powershell_sha256 = [string]$Receipt.powershell_sha256
        request_sha256 = $RequestSha256
        output = $Output
        error = $ErrorMessage
    })
}

function Invoke-WorkerCycle {
    param(
        [Parameter(Mandatory = $true)]$Paths,
        [Parameter(Mandatory = $true)]$Receipt,
        [Parameter(Mandatory = $true)]$Identity
    )

    $processed = 0
    foreach ($requestPath in @(
        Get-ChildItem -LiteralPath $Paths.Requests -File -Force |
            Where-Object { $_.Name.EndsWith($script:RequestSuffix, [StringComparison]::Ordinal) } |
            Sort-Object Name |
            Select-Object -First 32 |
            ForEach-Object FullName
    )) {
        $fileName = [IO.Path]::GetFileName($requestPath)
        $requestId = $fileName.Substring(0, $fileName.Length - $script:RequestSuffix.Length)
        if ($requestId -notmatch '^[0-9a-f]{32}$') { continue }
        $claim = $null
        try { $claim = Open-ExclusiveRequest -Path $requestPath } catch { continue }
        try {
            $resultPath = Join-Path $Paths.Results ($requestId + $script:ResultSuffix)
            if (Test-Path -LiteralPath $resultPath -PathType Leaf) { continue }

            $started = [DateTimeOffset]::UtcNow
            $requestHash = Get-BytesSha256 -Bytes $claim.Bytes
            $operationName = 'invalid'
            try {
                $request = Read-And-ValidateRequest `
                    -Bytes $claim.Bytes -ExpectedId $requestId -Receipt $Receipt
                $operationName = [string]$request.operation
                $output = Invoke-FixedOperation -Request $request
                Publish-RequestResult `
                    -Paths $Paths -Receipt $Receipt -Identity $Identity `
                    -RequestId $requestId -OperationName $operationName `
                    -RequestSha256 $requestHash -Status 'success' -ExitCode 0 `
                    -Output $output -ErrorMessage $null -StartedAt $started
            } catch {
                Publish-RequestResult `
                    -Paths $Paths -Receipt $Receipt -Identity $Identity `
                    -RequestId $requestId -OperationName $operationName `
                    -RequestSha256 $requestHash -Status 'rejected' -ExitCode 2 `
                    -Output $null -ErrorMessage $_.Exception.Message -StartedAt $started
            }
            $processed++
        } finally {
            $claim.Stream.Dispose()
            Remove-Item -LiteralPath $requestPath -Force -ErrorAction SilentlyContinue
        }
    }
    return $processed
}

function Read-And-ValidateHeartbeat {
    param(
        [Parameter(Mandatory = $true)]$Paths,
        [Parameter(Mandatory = $true)]$Receipt
    )

    $heartbeat = Read-StrictJsonDocument `
        -Path $Paths.Heartbeat `
        -MaximumBytes 16KB `
        -Label 'Broker heartbeat'
    Assert-ExactProperties -Document $heartbeat -Label 'Broker heartbeat' -Expected @(
        'executor_account', 'executor_sid', 'install_id', 'observed_at_utc',
        'powershell_sha256', 'process_id', 'schema_version', 'session_id',
        'worker_sha256'
    )
    $observed = Parse-RoundTripTime `
        -Value ([string]$heartbeat.observed_at_utc) `
        -Label 'heartbeat observed_at_utc'
    $now = [DateTimeOffset]::UtcNow
    if ($heartbeat.schema_version -ne $script:SchemaVersion -or
        [string]$heartbeat.install_id -cne [string]$Receipt.install_id -or
        [string]$heartbeat.executor_sid -cne [string]$Receipt.user_sid -or
        [string]$heartbeat.executor_account -cne [string]$Receipt.user_account -or
        [string]$heartbeat.worker_sha256 -cne [string]$Receipt.worker_sha256 -or
        [string]$heartbeat.powershell_sha256 -cne
            [string]$Receipt.powershell_sha256 -or
        $observed -gt $now.AddSeconds($script:MaxClockSkewSeconds) -or
        ($now - $observed).TotalSeconds -gt $script:HeartbeatMaximumAgeSeconds) {
        throw 'Broker heartbeat is stale or not bound to the installed qinrm worker.'
    }
    return $heartbeat
}

function Invoke-BrokerClient {
    param(
        [Parameter(Mandatory = $true)]$Paths,
        [Parameter(Mandatory = $true)]$Receipt,
        [Parameter(Mandatory = $true)][string]$OperationName,
        [Parameter(Mandatory = $true)][int]$WaitSeconds
    )

    Assert-InstalledTaskContract -Paths $Paths -Receipt $Receipt
    Assert-InstalledAclContract -Paths $Paths -Receipt $Receipt
    Assert-RequestDaclWriteDenied -RequestRoot $Paths.Requests
    Assert-ResultCreateDenied -ResultRoot $Paths.Results
    Assert-ExistingFileWriteDenied `
        -Path ([string]$Receipt.worker_path) -Label 'Installed worker'
    Assert-ExistingFileWriteDenied -Path $Paths.Receipt -Label 'Install receipt'
    [void](Read-And-ValidateHeartbeat -Paths $Paths -Receipt $Receipt)
    $requestId = [Guid]::NewGuid().ToString('N')
    $created = [DateTimeOffset]::UtcNow
    $requestPath = Join-Path $Paths.Requests ($requestId + $script:RequestSuffix)
    $requestHash = Write-JsonAtomic -Path $requestPath -PassThruHash -Document ([ordered]@{
        schema_version = $script:SchemaVersion
        install_id = [string]$Receipt.install_id
        request_id = $requestId
        operation = $OperationName
        created_at_utc = $created.ToString('o')
        expires_at_utc = $created.AddSeconds(
            [Math]::Min($WaitSeconds + 30, $script:MaxLifetimeSeconds)
        ).ToString('o')
        payload = [ordered]@{}
    })
    $resultPath = Join-Path $Paths.Results ($requestId + $script:ResultSuffix)
    $deadline = [DateTimeOffset]::UtcNow.AddSeconds($WaitSeconds)
    while ([DateTimeOffset]::UtcNow -lt $deadline) {
        if (Test-Path -LiteralPath $resultPath -PathType Leaf) {
            $result = Read-StrictJsonDocument `
                -Path $resultPath -MaximumBytes 64KB -Label 'Broker result'
            Assert-ExactProperties -Document $result -Label 'Broker result' -Expected @(
                'error', 'executor_account', 'executor_sid', 'exit_code',
                'finished_at_utc', 'install_id', 'operation', 'output',
                'powershell_sha256', 'request_id', 'request_sha256',
                'schema_version', 'started_at_utc', 'status', 'worker_sha256'
            )
            if ($result.schema_version -ne $script:SchemaVersion -or
                [string]$result.install_id -cne [string]$Receipt.install_id -or
                [string]$result.request_id -cne $requestId -or
                [string]$result.operation -cne $OperationName -or
                [string]$result.request_sha256 -cne $requestHash -or
                [string]$result.executor_sid -cne [string]$Receipt.user_sid -or
                [string]$result.executor_account -cne
                    [string]$Receipt.user_account -or
                [string]$result.worker_sha256 -cne [string]$Receipt.worker_sha256 -or
                [string]$result.powershell_sha256 -cne
                    [string]$Receipt.powershell_sha256) {
                throw 'Broker result is not bound to this request and installed worker.'
            }
            if ([string]$result.status -cne 'success' -or [int]$result.exit_code -ne 0) {
                throw "Broker rejected the request: $($result.error)"
            }
            Assert-ExactProperties -Document $result.output -Label 'identity_probe output' `
                -Expected @(
                    'account', 'language_mode', 'powershell_version', 'process_id',
                    'session_id', 'sid', 'user_profile'
                )
            if ([string]$result.output.account -cne [string]$Receipt.user_account -or
                [string]$result.output.sid -cne [string]$Receipt.user_sid) {
                throw 'identity_probe output is not the installed target user.'
            }
            return $result
        }
        Start-Sleep -Milliseconds 100
    }
    throw "Timed out waiting for broker result $requestId."
}

function New-SelfTestRequest {
    param(
        [Parameter(Mandatory = $true)]$Paths,
        [Parameter(Mandatory = $true)][string]$OperationName,
        [Parameter(Mandatory = $true)][DateTimeOffset]$Created,
        [Parameter(Mandatory = $true)][DateTimeOffset]$Expires,
        [Parameter(Mandatory = $true)]$Payload,
        [Parameter(Mandatory = $true)][string]$InstallId,
        [string]$RequestId = ([Guid]::NewGuid().ToString('N'))
    )

    Write-JsonAtomic `
        -Path (Join-Path $Paths.Requests ($RequestId + $script:RequestSuffix)) `
        -Document ([ordered]@{
            schema_version = $script:SchemaVersion
            install_id = $InstallId
            request_id = $RequestId
            operation = $OperationName
            created_at_utc = $Created.ToUniversalTime().ToString('o')
            expires_at_utc = $Expires.ToUniversalTime().ToString('o')
            payload = $Payload
        })
    return $RequestId
}

function Invoke-SelfTest {
    $repoRoot = Split-Path -Parent $PSScriptRoot
    $managedRoot = Join-Path $repoRoot '.RaymanCodingSkill\tmp'
    if (-not (Test-Path -LiteralPath $managedRoot -PathType Container)) {
        New-Item -ItemType Directory -Path $managedRoot | Out-Null
    }
    $testRoot = Assert-ChildPath `
        -Child (Join-Path $managedRoot (
            'codex-powershell-broker-selftest-' + [Guid]::NewGuid().ToString('N')
        )) `
        -Parent $managedRoot `
        -Label 'Broker self-test root'
    New-Item -ItemType Directory -Path $testRoot | Out-Null
    try {
        $taskNamespaceUri = 'http://schemas.microsoft.com/windows/2004/02/mit/task'
        $testTaskAccount = 'QIN5521\qinrm'
        $testTaskSid = 'S-1-5-21-1-2-3-1001'
        $assertRejected = {
            param([scriptblock]$Action, [string]$Label)
            $accepted = $false
            try { & $Action; $accepted = $true } catch { }
            if ($accepted) { throw "Self-test accepted invalid case: $Label" }
        }
        $testAclUserSid = 'S-1-5-21-1-2-3-1001'
        $testAclSandboxSid = 'S-1-5-21-1-2-3-1002'
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
                [Security.Principal.SecurityIdentifier]::new($testAclUserSid)
            )
            $inherit = [Security.AccessControl.InheritanceFlags]'ContainerInherit, ObjectInherit'
            foreach ($sid in @('S-1-5-18', 'S-1-5-32-544', $testAclUserSid)) {
                Add-ExpectedAccessRule -Security $security -Sid $sid `
                    -Rights ([Security.AccessControl.FileSystemRights]::FullControl) `
                    -Inheritance $inherit
            }
            Add-ExpectedAccessRule -Security $security -Sid $testAclSandboxSid `
                -Rights $SandboxRights -Inheritance $inherit
            Add-ExpectedAccessRule -Security $security -Sid $CapabilitySid `
                -Rights $CapabilityRights -Inheritance $inherit
            return $security
        }
        $baseRequestSecurity = New-ExpectedDirectorySecurity `
            -UserSid $testAclUserSid -SandboxSid $testAclSandboxSid `
            -Kind Requests
        Assert-RequestSecurityDescriptorContract `
            -Security $baseRequestSecurity -ExpectedOwnerSid $testAclUserSid `
            -SandboxSid $testAclSandboxSid -Label 'Installer-base request ACL'
        $managedRequestSecurity = & $newManagedRequestSecurity
        Assert-RequestSecurityDescriptorContract `
            -Security $managedRequestSecurity -ExpectedOwnerSid $testAclUserSid `
            -SandboxSid $testAclSandboxSid -Label 'Codex-managed request ACL'
        & $assertRejected {
            $sameDomain = & $newManagedRequestSecurity `
                -CapabilitySid 'S-1-5-21-1-2-3-1999'
            Assert-RequestSecurityDescriptorContract `
                -Security $sameDomain -ExpectedOwnerSid $testAclUserSid `
                -SandboxSid $testAclSandboxSid -Label 'Same-domain extra ACE'
        } 'same-domain request ACL extra ACE'
        & $assertRejected {
            $broadPrincipal = & $newManagedRequestSecurity `
                -CapabilitySid 'S-1-1-0'
            Assert-RequestSecurityDescriptorContract `
                -Security $broadPrincipal -ExpectedOwnerSid $testAclUserSid `
                -SandboxSid $testAclSandboxSid -Label 'Broad extra ACE'
        } 'broad-principal request ACL extra ACE'
        & $assertRejected {
            $capabilityFullControl = & $newManagedRequestSecurity `
                -CapabilityRights ([Security.AccessControl.FileSystemRights]::FullControl)
            Assert-RequestSecurityDescriptorContract `
                -Security $capabilityFullControl -ExpectedOwnerSid $testAclUserSid `
                -SandboxSid $testAclSandboxSid -Label 'Capability rights drift'
        } 'capability request ACL rights drift'
        & $assertRejected {
            $sandboxFullControl = & $newManagedRequestSecurity `
                -SandboxRights ([Security.AccessControl.FileSystemRights]::FullControl)
            Assert-RequestSecurityDescriptorContract `
                -Security $sandboxFullControl -ExpectedOwnerSid $testAclUserSid `
                -SandboxSid $testAclSandboxSid -Label 'Sandbox rights drift'
        } 'sandbox-group request ACL rights drift'
        $assertTaskXml = {
            param([string]$Xml, [string]$Label)
            [xml]$taskDocument = $Xml
            $taskNamespace = [Xml.XmlNamespaceManager]::new($taskDocument.NameTable)
            $taskNamespace.AddNamespace('t', $taskNamespaceUri)
            Assert-TaskXmlStructureContract `
                -Document $taskDocument -Namespace $taskNamespace -Label $Label
            $logonTrigger = $taskDocument.SelectSingleNode(
                '/t:Task/t:Triggers/t:LogonTrigger', $taskNamespace
            )
            $principal = $taskDocument.SelectSingleNode(
                '/t:Task/t:Principals/t:Principal', $taskNamespace
            )
            Assert-TaskUserIdentityBinding `
                -Actual (Read-UniqueTaskUserId `
                    -Parent $logonTrigger -Label "$Label LogonTrigger") `
                -ExpectedAccount $testTaskAccount -ExpectedSid $testTaskSid `
                -Label "$Label LogonTrigger UserId"
            Assert-TaskUserIdentityBinding `
                -Actual (Read-UniqueTaskUserId `
                    -Parent $principal -Label "$Label Principal") `
                -ExpectedAccount $testTaskAccount -ExpectedSid $testTaskSid `
                -Label "$Label Principal UserId"
        }
        $explicitTaskXml = @"
<Task xmlns="$taskNamespaceUri">
  <Triggers><LogonTrigger><Enabled>true</Enabled><UserId>QIN5521\qinrm</UserId></LogonTrigger></Triggers>
  <Principals><Principal id="Author"><UserId>QIN5521\qinrm</UserId><RunLevel>LeastPrivilege</RunLevel></Principal></Principals>
  <Settings><Enabled>true</Enabled></Settings>
  <Actions Context="Author"><Exec><Command>pwsh.exe</Command><Arguments>-NoProfile</Arguments><WorkingDirectory>C:\ProgramData\Rayman</WorkingDirectory></Exec></Actions>
</Task>
"@
        $taskStorageRoot = Join-Path $testRoot 'Tasks'
        New-Item -ItemType Directory -Path $taskStorageRoot | Out-Null
        $taskFilePath = Join-Path $taskStorageRoot 'Rayman-CodexPowerShellBroker'
        [IO.File]::WriteAllText($taskFilePath, $explicitTaskXml)
        $taskFile = Get-BrokerTaskFileSnapshot `
            -Name '\Rayman-CodexPowerShellBroker' -TaskRoot $taskStorageRoot
        if ([string]$taskFile.Xml -cne $explicitTaskXml -or
            [string]$taskFile.Path -cne $taskFilePath) {
            throw 'Task file snapshot self-test did not preserve the fixed task bytes/path.'
        }
        & $assertRejected {
            [void](Get-BrokerTaskFileSnapshot `
                -Name '\Nested\Task' -TaskRoot $taskStorageRoot)
        } 'nested Task Scheduler name'
        [IO.File]::WriteAllText($taskFilePath, '')
        & $assertRejected {
            [void](Get-BrokerTaskFileSnapshot `
                -Name '\Rayman-CodexPowerShellBroker' -TaskRoot $taskStorageRoot)
        } 'empty Task Scheduler XML file'
        [IO.File]::WriteAllBytes($taskFilePath, [byte[]]::new(1MB + 1))
        & $assertRejected {
            [void](Get-BrokerTaskFileSnapshot `
                -Name '\Rayman-CodexPowerShellBroker' -TaskRoot $taskStorageRoot)
        } 'oversized Task Scheduler XML file'
        [IO.File]::WriteAllText($taskFilePath, $explicitTaskXml)
        $runLevelElement = '<RunLevel>LeastPrivilege</RunLevel>'
        $schedulerMaterializedTaskXml = $explicitTaskXml.Replace(
            '<LogonTrigger><Enabled>true</Enabled><UserId>',
            '<LogonTrigger><UserId>'
        ).Replace(
            '<Settings><Enabled>true</Enabled></Settings>',
            '<Settings></Settings>'
        ).Replace(
            $runLevelElement,
            ''
        )
        $principalSidMaterializedTaskXml = $schedulerMaterializedTaskXml.Replace(
            '<Principal id="Author"><UserId>QIN5521\qinrm</UserId>',
            '<Principal id="Author"><UserId>S-1-5-21-1-2-3-1001</UserId>'
        )
        $allSidMaterializedTaskXml = $principalSidMaterializedTaskXml.Replace(
            '<LogonTrigger><UserId>QIN5521\qinrm</UserId>',
            '<LogonTrigger><UserId>S-1-5-21-1-2-3-1001</UserId>'
        )
        foreach ($validTaskXml in @(
            $explicitTaskXml,
            $schedulerMaterializedTaskXml,
            $principalSidMaterializedTaskXml,
            $allSidMaterializedTaskXml
        )) {
            & $assertTaskXml $validTaskXml 'Valid broker task self-test XML'
        }
        $invalidTaskXml = [ordered]@{
            logon_explicitly_disabled = $explicitTaskXml.Replace(
                '<LogonTrigger><Enabled>true</Enabled>',
                '<LogonTrigger><Enabled>false</Enabled>'
            )
            logon_duplicate_enabled = $explicitTaskXml.Replace(
                '<LogonTrigger><Enabled>true</Enabled>',
                '<LogonTrigger><Enabled>true</Enabled><Enabled>true</Enabled>'
            )
            logon_foreign_enabled = $explicitTaskXml.Replace(
                '<LogonTrigger><Enabled>true</Enabled>',
                '<LogonTrigger><Enabled xmlns="">true</Enabled>'
            )
            additional_trigger = $explicitTaskXml.Replace(
                '</LogonTrigger></Triggers>',
                '</LogonTrigger><TimeTrigger><StartBoundary>2026-01-01T00:00:00</StartBoundary></TimeTrigger></Triggers>'
            )
            settings_explicitly_disabled = $explicitTaskXml.Replace(
                '<Settings><Enabled>true</Enabled></Settings>',
                '<Settings><Enabled>false</Enabled></Settings>'
            )
            settings_duplicate_enabled = $explicitTaskXml.Replace(
                '<Settings><Enabled>true</Enabled></Settings>',
                '<Settings><Enabled>true</Enabled><Enabled>true</Enabled></Settings>'
            )
            additional_principal = $explicitTaskXml.Replace(
                '</Principal></Principals>',
                '</Principal><Principal id="Other"><UserId>QIN5521\other</UserId></Principal></Principals>'
            )
            additional_exec = $explicitTaskXml.Replace(
                '</Exec></Actions>',
                '</Exec><Exec><Command>cmd.exe</Command></Exec></Actions>'
            )
            run_level_elevated = $explicitTaskXml.Replace(
                $runLevelElement,
                '<RunLevel>HighestAvailable</RunLevel>'
            )
            run_level_duplicate = $explicitTaskXml.Replace(
                $runLevelElement,
                '<RunLevel>LeastPrivilege</RunLevel><RunLevel>LeastPrivilege</RunLevel>'
            )
            run_level_foreign = $explicitTaskXml.Replace(
                $runLevelElement,
                '<RunLevel xmlns="urn:foreign">LeastPrivilege</RunLevel>'
            )
            run_level_invalid_value = $explicitTaskXml.Replace(
                $runLevelElement,
                '<RunLevel>leastPrivilege</RunLevel>'
            )
            duplicate_logon_user_id = $explicitTaskXml.Replace(
                '<UserId>QIN5521\qinrm</UserId></LogonTrigger>',
                '<UserId>QIN5521\qinrm</UserId><UserId>QIN5521\qinrm</UserId></LogonTrigger>'
            )
            foreign_duplicate_logon_user_id = $explicitTaskXml.Replace(
                '<UserId>QIN5521\qinrm</UserId></LogonTrigger>',
                '<UserId>QIN5521\qinrm</UserId><UserId xmlns="urn:foreign">QIN5521\other</UserId></LogonTrigger>'
            )
            foreign_only_logon_user_id = $explicitTaskXml.Replace(
                '<UserId>QIN5521\qinrm</UserId></LogonTrigger>',
                '<UserId xmlns="urn:foreign">QIN5521\qinrm</UserId></LogonTrigger>'
            )
            duplicate_principal_user_id = $explicitTaskXml.Replace(
                '<Principal id="Author"><UserId>QIN5521\qinrm</UserId>',
                '<Principal id="Author"><UserId>QIN5521\qinrm</UserId><UserId>QIN5521\qinrm</UserId>'
            )
            foreign_duplicate_principal_user_id = $explicitTaskXml.Replace(
                '<Principal id="Author"><UserId>QIN5521\qinrm</UserId>',
                '<Principal id="Author"><UserId>QIN5521\qinrm</UserId><UserId xmlns="urn:foreign">QIN5521\other</UserId>'
            )
            foreign_only_principal_user_id = $explicitTaskXml.Replace(
                '<Principal id="Author"><UserId>QIN5521\qinrm</UserId>',
                '<Principal id="Author"><UserId xmlns="urn:foreign">QIN5521\qinrm</UserId>'
            )
            wrong_principal_sid = $principalSidMaterializedTaskXml.Replace(
                '<Principal id="Author"><UserId>S-1-5-21-1-2-3-1001</UserId>',
                '<Principal id="Author"><UserId>S-1-5-21-1-2-3-9999</UserId>'
            )
            wrong_logon_sid = $allSidMaterializedTaskXml.Replace(
                '<LogonTrigger><UserId>S-1-5-21-1-2-3-1001</UserId>',
                '<LogonTrigger><UserId>S-1-5-21-1-2-3-9999</UserId>'
            )
            unresolvable_principal_identity = $explicitTaskXml.Replace(
                '<Principal id="Author"><UserId>QIN5521\qinrm</UserId>',
                '<Principal id="Author"><UserId>RAYMAN-NO-SUCH-DOMAIN\missing-broker-user</UserId>'
            )
        }
        foreach ($taskCase in $invalidTaskXml.GetEnumerator()) {
            $accepted = $false
            try {
                & $assertTaskXml ([string]$taskCase.Value) `
                    'Invalid broker task self-test XML'
                $accepted = $true
            } catch { }
            if ($accepted) {
                throw "Broker task XML self-test accepted invalid case: $($taskCase.Key)"
            }
        }
        foreach ($validTaskIdentity in @($testTaskAccount, $testTaskSid)) {
            Assert-TaskUserIdentityBinding `
                -Actual $validTaskIdentity `
                -ExpectedAccount $testTaskAccount -ExpectedSid $testTaskSid `
                -Label 'Valid broker task identity self-test'
        }
        $wrongTaskIdentityAccepted = $false
        try {
            Assert-TaskUserIdentityBinding `
                -Actual 'S-1-5-21-1-2-3-9999' `
                -ExpectedAccount $testTaskAccount -ExpectedSid $testTaskSid `
                -Label 'Invalid broker task identity self-test'
            $wrongTaskIdentityAccepted = $true
        } catch { }
        if ($wrongTaskIdentityAccepted) {
            throw 'Broker task identity self-test accepted a different SID.'
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
            -Label 'Broker Task Scheduler materialized self-test ACL'
        $taskFileSecurity = $materializedTaskSecurity.Replace('G:BAD:', 'G:BAD:PAI')
        Assert-TaskSecurityDescriptorContract -Sddl $taskFileSecurity `
            -UserSid $testUserSid -SandboxSid $testSandboxSid `
            -Label 'Broker task file projected self-test ACL' `
            -AllowAutoInheritedControl -RequireProtectedDacl
        & $assertRejected {
            Assert-TaskSecurityDescriptorContract -Sddl $taskFileSecurity `
                -UserSid $testUserSid -SandboxSid $testSandboxSid `
                -Label 'Task file without projection allowance'
        } 'task file auto-inherited projection without explicit allowance'
        & $assertRejected {
            Assert-TaskSecurityDescriptorContract -Sddl $materializedTaskSecurity `
                -UserSid $testUserSid -SandboxSid $testSandboxSid `
                -Label 'Task file without protected DACL' `
                -AllowAutoInheritedControl -RequireProtectedDacl
        } 'task file projection without protected DACL'
        foreach ($invalid in @(
            $materializedTaskSecurity.Replace("G:BA", ''),
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
                    -Label 'Invalid broker Task Scheduler self-test ACL'
                $accepted = $true
            } catch { }
            if ($accepted) {
                throw "Broker task security self-test accepted descriptor drift: $invalid"
            }
        }
        $testInstall = Join-Path $testRoot 'install'
        $testRequests = Join-Path $testInstall 'requests'
        $testResults = Join-Path $testInstall 'results'
        New-Item -ItemType Directory -Path $testInstall | Out-Null
        New-Item -ItemType Directory -Path $testRequests | Out-Null
        New-Item -ItemType Directory -Path $testResults | Out-Null
        $paths = Get-BrokerPaths -Install $testInstall -Requests $testRequests
        $identity = Get-CurrentIdentityRecord
        $runtime = Get-CurrentPowerShellRuntime
        $installId = [Guid]::NewGuid().ToString('N')
        $workerHash = Get-FileSha256 -Path $PSCommandPath
        $testVersion = Join-Path (Join-Path $testInstall 'versions') $workerHash
        New-Item -ItemType Directory -Path $testVersion -Force | Out-Null
        $testWorker = Join-Path $testVersion 'codex-powershell-broker.ps1'
        [IO.File]::WriteAllBytes($testWorker, [IO.File]::ReadAllBytes($PSCommandPath))
        [IO.File]::WriteAllBytes($paths.WorkerLock, [byte[]]::new(0))
        Write-JsonAtomic -Path $paths.Receipt -Document ([ordered]@{
            schema_version = $script:SchemaVersion
            install_id = $installId
            install_root = $paths.Install
            installed_at_utc = [DateTimeOffset]::UtcNow.ToString('o')
            task_name = '\Rayman-CodexPowerShellBroker-SelfTest'
            user_account = $identity.Account
            user_sid = $identity.Sid
            sandbox_group = 'SELFTEST'
            sandbox_group_sid = $identity.Sid
            worker_path = $testWorker
            worker_sha256 = $workerHash
            powershell_path = $runtime.Path
            powershell_sha256 = $runtime.Sha256
            request_root = $paths.Requests
            result_root = $paths.Results
            capabilities = @('identity_probe')
        })
        $receipt = Read-BrokerReceipt -Paths $paths
        $identity = Assert-WorkerBinding -Receipt $receipt -WorkerPath $testWorker
        $now = [DateTimeOffset]::UtcNow

        $claimId = New-SelfTestRequest `
            -Paths $paths -OperationName 'identity_probe' `
            -Created $now -Expires $now.AddSeconds(60) -Payload ([ordered]@{}) `
            -InstallId $installId
        $claimPath = Join-Path $paths.Requests ($claimId + $script:RequestSuffix)
        $claim = Open-ExclusiveRequest -Path $claimPath
        try {
            $writeOpened = $false
            try {
                $competing = [IO.FileStream]::new(
                    $claimPath, [IO.FileMode]::Open, [IO.FileAccess]::Write,
                    [IO.FileShare]::ReadWrite
                )
                $writeOpened = $true
                $competing.Dispose()
            } catch [IO.IOException] { }
            if ($writeOpened) {
                throw 'Self-test exclusive request claim allowed a competing writer.'
            }
        } finally {
            $claim.Stream.Dispose()
            Remove-Item -LiteralPath $claimPath -Force
        }

        Publish-Heartbeat -Paths $paths -Receipt $receipt -Identity $identity
        [void](Read-And-ValidateHeartbeat -Paths $paths -Receipt $receipt)
        Write-JsonAtomic -Path $paths.Heartbeat -Replace -Document ([ordered]@{
            schema_version = $script:SchemaVersion
            install_id = $installId
            observed_at_utc = $now.AddMinutes(5).ToString('o')
            executor_account = $identity.Account
            executor_sid = $identity.Sid
            process_id = $identity.ProcessId
            session_id = $identity.SessionId
            worker_sha256 = $workerHash
            powershell_sha256 = $runtime.Sha256
        })
        $futureHeartbeatAccepted = $false
        try {
            [void](Read-And-ValidateHeartbeat -Paths $paths -Receipt $receipt)
            $futureHeartbeatAccepted = $true
        } catch { }
        if ($futureHeartbeatAccepted) {
            throw 'Self-test accepted a future-dated heartbeat.'
        }
        Publish-Heartbeat -Paths $paths -Receipt $receipt -Identity $identity

        $validId = New-SelfTestRequest `
            -Paths $paths -OperationName 'identity_probe' `
            -Created $now -Expires $now.AddSeconds(60) -Payload ([ordered]@{}) `
            -InstallId $installId
        if ((Invoke-WorkerCycle -Paths $paths -Receipt $receipt -Identity $identity) -ne 1) {
            throw 'Self-test worker did not process the valid request.'
        }
        $validResult = Read-StrictJsonDocument `
            -Path (Join-Path $paths.Results ($validId + $script:ResultSuffix)) `
            -MaximumBytes 64KB -Label 'Self-test valid result'
        $validResultSid = if ($null -ne $validResult.output -and
            $validResult.output.PSObject.Properties.Name -contains 'sid') {
            [string]$validResult.output.sid
        } else { '' }
        if ($validResult.status -cne 'success' -or $validResultSid -cne $identity.Sid) {
            throw "Self-test valid identity result failed. Status=$($validResult.status) ResultSid=$validResultSid Error=$($validResult.error)"
        }

        $sentinel = Join-Path $testRoot 'must-not-exist.txt'
        $unknownId = New-SelfTestRequest `
            -Paths $paths -OperationName 'powershell_command' `
            -Created $now -Expires $now.AddSeconds(60) `
            -Payload ([ordered]@{ command = "New-Item -Path '$sentinel'" }) `
            -InstallId $installId
        [void](Invoke-WorkerCycle -Paths $paths -Receipt $receipt -Identity $identity)
        $unknownResult = Read-StrictJsonDocument `
            -Path (Join-Path $paths.Results ($unknownId + $script:ResultSuffix)) `
            -MaximumBytes 64KB -Label 'Self-test unknown-operation result'
        if ($unknownResult.status -cne 'rejected' -or (Test-Path -LiteralPath $sentinel)) {
            throw 'Self-test arbitrary command was not rejected safely.'
        }

        $expiredId = New-SelfTestRequest `
            -Paths $paths -OperationName 'identity_probe' `
            -Created $now.AddMinutes(-3) -Expires $now.AddMinutes(-2) `
            -Payload ([ordered]@{}) -InstallId $installId
        [void](Invoke-WorkerCycle -Paths $paths -Receipt $receipt -Identity $identity)
        $expiredResult = Read-StrictJsonDocument `
            -Path (Join-Path $paths.Results ($expiredId + $script:ResultSuffix)) `
            -MaximumBytes 64KB -Label 'Self-test expired result'
        if ($expiredResult.status -cne 'rejected') {
            throw 'Self-test expired request was accepted.'
        }

        $wrongInstallId = [Guid]::NewGuid().ToString('N')
        $wrongInstallRequest = New-SelfTestRequest `
            -Paths $paths -OperationName 'identity_probe' `
            -Created $now -Expires $now.AddSeconds(60) `
            -Payload ([ordered]@{}) -InstallId $wrongInstallId
        [void](Invoke-WorkerCycle -Paths $paths -Receipt $receipt -Identity $identity)
        $wrongInstallResult = Read-StrictJsonDocument `
            -Path (Join-Path $paths.Results ($wrongInstallRequest + $script:ResultSuffix)) `
            -MaximumBytes 64KB -Label 'Self-test wrong-install result'
        if ($wrongInstallResult.status -cne 'rejected') {
            throw 'Self-test accepted a request for a different install_id.'
        }

        $validResultPath = Join-Path $paths.Results ($validId + $script:ResultSuffix)
        $replayHash = Get-FileSha256 -Path $validResultPath
        [void](New-SelfTestRequest `
            -Paths $paths -OperationName 'identity_probe' `
            -Created $now -Expires $now.AddSeconds(60) `
            -Payload ([ordered]@{}) -InstallId $installId -RequestId $validId)
        [void](Invoke-WorkerCycle -Paths $paths -Receipt $receipt -Identity $identity)
        if ((Get-FileSha256 -Path $validResultPath) -cne $replayHash) {
            throw 'Self-test replay changed an existing protected result.'
        }
        Write-Host 'codex-powershell-broker.ps1 self-test passed.'
    } finally {
        $verified = Assert-ChildPath `
            -Child $testRoot -Parent $managedRoot -Label 'Broker self-test cleanup root'
        if (Test-Path -LiteralPath $verified -PathType Container) {
            Remove-Item -LiteralPath $verified -Recurse -Force
        }
    }
}

if ($SelfTest) { Invoke-SelfTest; return }

$paths = Get-BrokerPaths -Install $InstallRoot -Requests $RequestRoot
[void](Assert-NoReparseAncestors -Path $paths.Install -Label 'Install root' -RequireLeaf)
[void](Assert-NoReparseAncestors -Path $paths.Requests -Label 'Request root' -RequireLeaf)
[void](Assert-NoReparseAncestors -Path $paths.Results -Label 'Result root' -RequireLeaf)
$receipt = Read-BrokerReceipt -Paths $paths

if ($ProcessOnce) {
    $identity = Assert-WorkerBinding -Receipt $receipt -WorkerPath $PSCommandPath
    [void](Invoke-WorkerCycle -Paths $paths -Receipt $receipt -Identity $identity)
    return
}

if ($Worker) {
    $identity = Assert-WorkerBinding -Receipt $receipt -WorkerPath $PSCommandPath
    $task = Assert-InstalledTaskContract -Paths $paths -Receipt $receipt
    Assert-InstalledAclContract -Paths $paths -Receipt $receipt
    $workerLock = $null
    try {
        $workerLock = [IO.FileStream]::new(
            $paths.WorkerLock,
            [IO.FileMode]::Open,
            [IO.FileAccess]::ReadWrite,
            [IO.FileShare]::None
        )
    } catch [IO.IOException] { return }
    try {
        $lastHeartbeat = [DateTimeOffset]::MinValue
        while ($true) {
            if (-not (Test-Path -LiteralPath ([string]$task.Path) -PathType Leaf)) {
                Write-Verbose 'Broker task storage disappeared; worker is stopping.'
                return
            }
            if (([DateTimeOffset]::UtcNow - $lastHeartbeat).TotalSeconds -ge 2) {
                Publish-Heartbeat -Paths $paths -Receipt $receipt -Identity $identity
                $lastHeartbeat = [DateTimeOffset]::UtcNow
            }
            [void](Invoke-WorkerCycle -Paths $paths -Receipt $receipt -Identity $identity)
            Start-Sleep -Milliseconds $PollMilliseconds
        }
    } finally {
        $workerLock.Dispose()
    }
}

Assert-ClientSourceBinding -Receipt $receipt -ClientPath $PSCommandPath
$result = Invoke-BrokerClient `
    -Paths $paths -Receipt $receipt `
    -OperationName $Operation -WaitSeconds $TimeoutSeconds
$result | ConvertTo-Json -Depth 16
