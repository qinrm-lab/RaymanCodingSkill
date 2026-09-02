[CmdletBinding(DefaultParameterSetName = 'Check')]
param(
    [Parameter(Mandatory = $true, ParameterSetName = 'Check')]
    [switch]$Check,
    [Parameter(Mandatory = $true, ParameterSetName = 'Install')]
    [switch]$Install,
    [Parameter(Mandatory = $true, ParameterSetName = 'Upgrade')]
    [switch]$Upgrade,
    [Parameter(Mandatory = $true, ParameterSetName = 'Uninstall')]
    [switch]$Uninstall,
    [Parameter(Mandatory = $true, ParameterSetName = 'RecoverPartialUninstall')]
    [switch]$RecoverPartialUninstall,
    [Parameter(Mandatory = $true, ParameterSetName = 'SelfTest')]
    [switch]$SelfTest,
    [Parameter(Mandatory = $true, ParameterSetName = 'RecoverySelfTestChild')]
    [switch]$RecoverySelfTestChild,
    [Parameter(Mandatory = $true, ParameterSetName = 'UpgradeCrashSelfTestChild')]
    [switch]$UpgradeCrashSelfTestChild,
    [Parameter(Mandatory = $true, ParameterSetName = 'PrepareUpgradeLauncher')]
    [switch]$PrepareUpgradeLauncher,
    [Parameter(Mandatory = $true, ParameterSetName = 'PrepareInstallLauncher')]
    [switch]$PrepareInstallLauncher,
    [Parameter(Mandatory = $true, ParameterSetName = 'RecoverySelfTestChild')]
    [ValidatePattern('^[0-9a-f]{32}$')]
    [string]$RecoverySelfTestToken,
    [Parameter(Mandatory = $true, ParameterSetName = 'UpgradeCrashSelfTestChild')]
    [ValidatePattern('^[0-9a-f]{32}$')]
    [string]$UpgradeCrashSelfTestToken,
    [Parameter(Mandatory = $true, ParameterSetName = 'UpgradeCrashSelfTestChild')]
    [ValidateSet(
        'staged', 'old_stopped', 'new_receipt_published',
        'new_task_registered', 'new_task_started',
        'new_heartbeat_verified', 'guard_released', 'ready_published'
    )]
    [string]$UpgradeCrashSelfTestPhase,
    [Parameter(Mandatory = $true, ParameterSetName = 'PrepareUpgradeLauncher')]
    [Parameter(Mandatory = $true, ParameterSetName = 'PrepareInstallLauncher')]
    [Parameter(Mandatory = $true, ParameterSetName = 'Upgrade')]
    [Parameter(ParameterSetName = 'Install')]
    [ValidatePattern('^goal_[0-9a-f]{10}$')]
    [string]$ExpectedGoalId,
    [Parameter(Mandatory = $true, ParameterSetName = 'PrepareUpgradeLauncher')]
    [Parameter(Mandatory = $true, ParameterSetName = 'PrepareInstallLauncher')]
    [Parameter(Mandatory = $true, ParameterSetName = 'Upgrade')]
    [Parameter(ParameterSetName = 'Install')]
    [ValidatePattern('^[0-9a-f]{64}$')]
    [string]$ExpectedSourceFingerprint,
    [Parameter(Mandatory = $true, ParameterSetName = 'Upgrade')]
    [string]$UpgradeAuthorityManifestPath,
    [Parameter(Mandatory = $true, ParameterSetName = 'Upgrade')]
    [ValidatePattern('^[0-9a-f]{64}$')]
    [string]$UpgradeAuthorityManifestSha256,
    [Parameter(ParameterSetName = 'Install')]
    [string]$InstallAuthorityManifestPath,
    [Parameter(ParameterSetName = 'Install')]
    [ValidatePattern('^[0-9a-f]{64}$')]
    [string]$InstallAuthorityManifestSha256,
    [Parameter(Mandatory = $true, ParameterSetName = 'Install')]
    [Parameter(Mandatory = $true, ParameterSetName = 'Upgrade')]
    [Parameter(Mandatory = $true, ParameterSetName = 'Uninstall')]
    [Parameter(Mandatory = $true, ParameterSetName = 'RecoverPartialUninstall')]
    [Parameter(Mandatory = $true, ParameterSetName = 'PrepareUpgradeLauncher')]
    [Parameter(Mandatory = $true, ParameterSetName = 'PrepareInstallLauncher')]
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

$script:SchemaVersion = 3
$script:LegacySchemaVersion = 2
$script:BrokerSource = Join-Path $PSScriptRoot 'codex-powershell-broker.ps1'
$script:RepositoryRoot = Split-Path -Parent $PSScriptRoot
if ($script:RepositoryRoot.StartsWith('\\?\', [StringComparison]::Ordinal)) {
    $script:RepositoryRoot = $script:RepositoryRoot.Substring(4)
}
$script:ReceiptName = 'install-receipt.json'
$script:HeartbeatName = 'heartbeat.json'
$script:GitCapabilityId = 'git_local_commit_v1'
$script:GitCapabilityManifestName = 'git-local-commit-v1.json'
$script:GitCapabilityReadyName = 'git-local-commit-v1.ready.json'
$script:GitTransactionDirectoryName = 'git-local-commit-transactions'
$script:UpgradeRecoveryJournalName = 'upgrade-recovery-v1.json'
$script:UpgradeRecoveryJournalSchemaVersion = 2
$script:CurrentUpgradeRecoveryJournalSchemaVersion = 3
$script:UpgradeConsumedNoncePrefix = 'upgrade-launch-consumed-'
$script:UpgradePendingCapabilityKey = 'git-local-commit-v1/schema-v3-uac-upgrade'
$script:InstallPendingCapabilityKey = 'git-local-commit-v1/schema-v3-uac-install'
$script:UpgradePendingBoundaryClass = 'execution-context'
$script:BrokerHeartbeatStartupTimeoutSeconds = 60
$script:UpgradeSourcePaths = @(
    'AGENTS.md',
    'CLAUDE.md',
    'README.md',
    'crates/rayman/assets/repository-gate-inputs.json',
    'crates/rayman/src/goal/validation/cargo_isolation.rs',
    'crates/rayman/src/goal/validation/pytest_isolation.rs',
    'crates/rayman/tests/audit_script.rs',
    'docs/CODEX_POWERSHELL_BROKER.md',
    'governance/first-party-test-inventory.json',
    'governance/test-traceability.json',
    'scripts/check-agent-instructions.ps1',
    'scripts/codex-powershell-broker.ps1',
    'scripts/install-codex-powershell-broker.ps1'
)
$script:UpgradeGitInputCandidates = @(
    '.gitattributes',
    '.git/config',
    '.git/HEAD',
    '.git/index',
    '.git/refs/heads/main',
    '.git/packed-refs',
    '.git/info/exclude',
    '.git/info/attributes'
)
$script:InstallerSelfTestAtomicWriteFaultPhase = $null
$script:InstallerSelfTestReceiptFaultPhase = $null
$script:TaskDescription = 'Rayman fixed-capability Codex PowerShell identity broker'

function Invoke-BrokerPortableStaticSelfTest {
    $installerErrors = $null
    $workerErrors = $null
    [void][Management.Automation.Language.Parser]::ParseFile(
        $PSCommandPath, [ref]$null, [ref]$installerErrors
    )
    [void][Management.Automation.Language.Parser]::ParseFile(
        $script:BrokerSource, [ref]$null, [ref]$workerErrors
    )
    $source = [IO.File]::ReadAllText($PSCommandPath)
    foreach ($required in @(
        'function Assert-BrokerExplicitConfirmation',
        'function Get-BrokerExistingInstallProof',
        'function New-BrokerOwnedTreeSnapshot',
        'PrepareInstallLauncher',
        'Register-BrokerTaskWithContext'
    )) {
        if (-not $source.Contains($required, [StringComparison]::Ordinal)) {
            throw "Non-Windows static broker self-test lost required contract: $required"
        }
    }
    $forbiddenEnvironmentPath = '$env:' + 'WINDIR'
    $forbiddenTaskExecutable = 'sch' + 'tasks.exe'
    if (@($installerErrors).Count -ne 0 -or @($workerErrors).Count -ne 0 -or
        $source.Contains(
            $forbiddenEnvironmentPath,
            [StringComparison]::OrdinalIgnoreCase
        ) -or $source.Contains(
            $forbiddenTaskExecutable,
            [StringComparison]::OrdinalIgnoreCase
        )) {
        throw 'Non-Windows static broker self-test found parse or system-path drift.'
    }
}

if (-not $IsWindows) {
    if (-not $SelfTest) {
        throw 'Codex PowerShell broker installation is supported only on Windows.'
    }
    Invoke-BrokerPortableStaticSelfTest
    Write-Host 'install-codex-powershell-broker.ps1 self-test passed.'
    return
}

if (-not ('Rayman.CodexBrokerInstallerNative' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.IO;
using System.Runtime.InteropServices;
using System.Text.Json;
using System.Threading.Tasks;
using Microsoft.Win32.SafeHandles;

namespace Rayman {
    public static class CodexBrokerInstallerNative {
        [StructLayout(LayoutKind.Sequential)]
        struct FILE_ID_128 {
            [MarshalAs(UnmanagedType.ByValArray, SizeConst = 16)]
            public byte[] Identifier;
        }
        [StructLayout(LayoutKind.Sequential)]
        struct FILE_ID_INFO {
            public ulong VolumeSerialNumber;
            public FILE_ID_128 FileId;
        }
        [StructLayout(LayoutKind.Sequential)]
        struct FILE_ATTRIBUTE_TAG_INFO {
            public uint FileAttributes;
            public uint ReparseTag;
        }
        [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
        static extern SafeFileHandle CreateFileW(
            string path, uint access, uint share, IntPtr security,
            uint disposition, uint flags, IntPtr template);
        [DllImport("kernel32.dll", SetLastError = true)]
        static extern bool GetFileInformationByHandleEx(
            SafeFileHandle handle, int informationClass,
            out FILE_ID_INFO information, uint size);
        [DllImport("kernel32.dll", EntryPoint = "GetFileInformationByHandleEx",
            SetLastError = true)]
        static extern bool GetFileAttributeTagInfo(
            SafeFileHandle handle, int informationClass,
            out FILE_ATTRIBUTE_TAG_INFO information, uint size);
        [StructLayout(LayoutKind.Sequential)]
        struct FILE_DISPOSITION_INFO {
            [MarshalAs(UnmanagedType.Bool)]
            public bool DeleteFile;
        }
        [DllImport("kernel32.dll", SetLastError = true)]
        static extern bool SetFileInformationByHandle(
            SafeFileHandle handle, int informationClass,
            ref FILE_DISPOSITION_INFO information, uint size);

        static string StrongIdentityCore(SafeFileHandle handle) {
            FILE_ID_INFO info;
            if (!GetFileInformationByHandleEx(handle, 18, out info,
                (uint)Marshal.SizeOf<FILE_ID_INFO>()))
                throw new System.ComponentModel.Win32Exception(
                    Marshal.GetLastWin32Error(), "cannot read strong identity");
            return info.VolumeSerialNumber.ToString("x16") + ":" +
                BitConverter.ToString(info.FileId.Identifier).Replace("-", "").ToLowerInvariant();
        }

        static void AssertOrdinaryObject(
            SafeFileHandle handle, bool directory, string label) {
            FILE_ATTRIBUTE_TAG_INFO info;
            if (!GetFileAttributeTagInfo(handle, 9, out info,
                (uint)Marshal.SizeOf<FILE_ATTRIBUTE_TAG_INFO>()))
                throw new System.ComponentModel.Win32Exception(
                    Marshal.GetLastWin32Error(), label + " attributes unavailable");
            const uint Directory = 0x00000010, ReparsePoint = 0x00000400;
            bool isDirectory = (info.FileAttributes & Directory) != 0;
            if ((info.FileAttributes & ReparsePoint) != 0 || isDirectory != directory)
                throw new InvalidDataException(
                    label + " is not the required ordinary object type");
        }

        public static string StrongIdentity(string path, bool directory) {
            const uint Read = 0x80000000, Share = 7, Open = 3;
            const uint Backup = 0x02000000, NoReparse = 0x00200000;
            using (var handle = CreateFileW(path, Read, Share, IntPtr.Zero, Open,
                NoReparse | (directory ? Backup : 0), IntPtr.Zero)) {
                if (handle.IsInvalid)
                    throw new System.ComponentModel.Win32Exception(
                        Marshal.GetLastWin32Error(), "cannot open strong identity target");
                AssertOrdinaryObject(handle, directory, "strong identity target");
                return StrongIdentityCore(handle);
            }
        }

        public static string StrongIdentity(SafeFileHandle handle) {
            if (handle == null || handle.IsInvalid || handle.IsClosed)
                throw new ArgumentException("strong identity handle is invalid");
            return StrongIdentityCore(handle);
        }

        public static SafeFileHandle OpenDeleteHandle(string path) {
            return OpenDeleteHandle(path, false);
        }

        public static SafeFileHandle OpenDeleteHandle(string path, bool directory) {
            const uint Read = 0x80000000, Delete = 0x00010000;
            const uint ReadAttributes = 0x00000080, ShareAll = 7, Open = 3;
            const uint NoReparse = 0x00200000, Backup = 0x02000000;
            var handle = CreateFileW(path, Read | Delete | ReadAttributes,
                ShareAll, IntPtr.Zero, Open,
                NoReparse | (directory ? Backup : 0), IntPtr.Zero);
            if (handle.IsInvalid) {
                handle.Dispose();
                throw new System.ComponentModel.Win32Exception(
                    Marshal.GetLastWin32Error(), "cannot open exact-delete target");
            }
            try {
                AssertOrdinaryObject(handle, directory, "exact-delete target");
                return handle;
            } catch {
                handle.Dispose();
                throw;
            }
        }

        public static SafeFileHandle OpenPinnedFile(string path) {
            const uint Read = 0x80000000, ShareRead = 1, Open = 3;
            const uint OpenReparsePoint = 0x00200000;
            var handle = CreateFileW(path, Read, ShareRead, IntPtr.Zero, Open,
                OpenReparsePoint, IntPtr.Zero);
            if (handle.IsInvalid) {
                handle.Dispose();
                throw new System.ComponentModel.Win32Exception(
                    Marshal.GetLastWin32Error(),
                    "cannot open pinned file entry path=" + path);
            }
            try {
                AssertOrdinaryObject(handle, false, "pinned file entry");
                return handle;
            } catch {
                handle.Dispose();
                throw;
            }
        }

        public static void MarkDelete(SafeFileHandle handle) {
            var information = new FILE_DISPOSITION_INFO { DeleteFile = true };
            if (!SetFileInformationByHandle(handle, 4, ref information,
                (uint)Marshal.SizeOf<FILE_DISPOSITION_INFO>()))
                throw new System.ComponentModel.Win32Exception(
                    Marshal.GetLastWin32Error(), "cannot mark exact handle for deletion");
        }

        public static SafeFileHandle OpenDirectoryGuard(string path) {
            const uint Read = 0x80000000, ShareReadWrite = 3, Open = 3;
            const uint Backup = 0x02000000, NoReparse = 0x00200000;
            var handle = CreateFileW(path, Read, ShareReadWrite, IntPtr.Zero,
                Open, NoReparse | Backup, IntPtr.Zero);
            if (handle.IsInvalid) {
                handle.Dispose();
                throw new System.ComponentModel.Win32Exception(
                    Marshal.GetLastWin32Error(), "cannot open protected directory guard");
            }
            try {
                AssertOrdinaryObject(handle, true, "protected directory guard");
                return handle;
            } catch {
                handle.Dispose();
                throw;
            }
        }

        public static SafeFileHandle OpenOwnedDirectoryDeleteGuard(string path) {
            const uint Read = 0x80000000, Delete = 0x00010000;
            const uint ReadAttributes = 0x00000080;
            const uint ShareReadWrite = 3, Open = 3;
            const uint Backup = 0x02000000, NoReparse = 0x00200000;
            var handle = CreateFileW(path, Read | Delete | ReadAttributes,
                ShareReadWrite, IntPtr.Zero, Open, NoReparse | Backup,
                IntPtr.Zero);
            if (handle.IsInvalid) {
                handle.Dispose();
                throw new System.ComponentModel.Win32Exception(
                    Marshal.GetLastWin32Error(),
                    "cannot open owned-directory delete guard");
            }
            try {
                AssertOrdinaryObject(handle, true, "owned-directory delete guard");
                return handle;
            } catch {
                handle.Dispose();
                throw;
            }
        }

        public static void ProbeFileReplace(string path) {
            const uint Delete = 0x00010000, ReadAttributes = 0x00000080;
            const uint ShareAll = 7, Open = 3, NoReparse = 0x00200000;
            using (var handle = CreateFileW(path, Delete | ReadAttributes,
                ShareAll, IntPtr.Zero, Open, NoReparse, IntPtr.Zero)) {
                if (handle.IsInvalid)
                    throw new System.ComponentModel.Win32Exception(
                        Marshal.GetLastWin32Error(), "receipt replace probe failed");
                AssertOrdinaryObject(handle, false, "receipt replace probe");
            }
        }

        public static void DisposeAfter(IDisposable value, int milliseconds) {
            Task.Run(async () => {
                await Task.Delay(milliseconds).ConfigureAwait(false);
                value.Dispose();
            });
        }

        public static void ValidateUniqueJsonProperties(byte[] bytes) {
            var options = new JsonDocumentOptions {
                AllowTrailingCommas = false,
                CommentHandling = JsonCommentHandling.Disallow,
                MaxDepth = 16
            };
            using (var document = JsonDocument.Parse(bytes, options))
                ValidateElement(document.RootElement, "$", 0);
        }

        static void ValidateElement(JsonElement element, string path, int depth) {
            if (depth > 16) throw new InvalidDataException("JSON nesting exceeds 16");
            if (element.ValueKind == JsonValueKind.Object) {
                var names = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
                foreach (var property in element.EnumerateObject()) {
                    if (!names.Add(property.Name))
                        throw new InvalidDataException(
                            "duplicate or case-colliding JSON property at " + path);
                    ValidateElement(property.Value, path + "." + property.Name, depth + 1);
                }
            } else if (element.ValueKind == JsonValueKind.Array) {
                int index = 0;
                foreach (var item in element.EnumerateArray())
                    ValidateElement(item, path + "[" + (index++) + "]", depth + 1);
            }
        }
    }
}
'@
}

function Get-BytesSha256 {
    param(
        [Parameter(Mandatory = $true)]
        [AllowEmptyCollection()]
        [byte[]]$Bytes
    )
    return [Convert]::ToHexString(
        [Security.Cryptography.SHA256]::HashData($Bytes)
    ).ToLowerInvariant()
}

function Get-FileSha256 {
    param([Parameter(Mandatory = $true)][string]$Path)
    return Get-BytesSha256 -Bytes ([IO.File]::ReadAllBytes($Path))
}

function Get-BrokerStreamBytes {
    param(
        [Parameter(Mandatory = $true)][IO.FileStream]$Stream,
        [Parameter(Mandatory = $true)][long]$MaximumBytes,
        [Parameter(Mandatory = $true)][string]$Label
    )
    if (-not $Stream.CanRead -or -not $Stream.CanSeek -or
        $Stream.Length -lt 0 -or $Stream.Length -gt $MaximumBytes) {
        throw "$Label is not a bounded readable file handle."
    }
    $bytes = [byte[]]::new([int]$Stream.Length)
    $Stream.Position = 0
    $offset = 0
    while ($offset -lt $bytes.Length) {
        $read = $Stream.Read($bytes, $offset, $bytes.Length - $offset)
        if ($read -le 0) { throw "$Label ended before its held length." }
        $offset += $read
    }
    $Stream.Position = 0
    return ,$bytes
}

function Get-BrokerHeldFileMetadata {
    param(
        [Parameter(Mandatory = $true)][IO.FileStream]$Stream,
        [Parameter(Mandatory = $true)][string]$Label,
        [long]$MaximumBytes = 1MB
    )
    $bytes = Get-BrokerStreamBytes `
        -Stream $Stream -MaximumBytes $MaximumBytes -Label $Label
    $security = [IO.FileSystemAclExtensions]::GetAccessControl($Stream)
    $ownerSid = [string]$security.GetOwner(
        [Security.Principal.SecurityIdentifier]
    ).Value
    return [pscustomobject]@{
        Bytes = $bytes
        Sha256 = Get-BytesSha256 -Bytes $bytes
        Identity = [Rayman.CodexBrokerInstallerNative]::StrongIdentity(
            $Stream.SafeFileHandle
        )
        OwnerSid = $ownerSid
        AccessSddl = $security.GetSecurityDescriptorSddlForm(
            [Security.AccessControl.AccessControlSections]::Access
        )
        AccessRulesProtected = [bool]$security.AreAccessRulesProtected
    }
}

function Open-BrokerDirectoryGuardChain {
    param(
        [Parameter(Mandatory = $true)][string]$FilePath,
        [Parameter(Mandatory = $true)][string]$Label
    )
    $directories = [Collections.Generic.List[string]]::new()
    $current = [IO.Path]::GetDirectoryName([IO.Path]::GetFullPath($FilePath))
    while (-not [string]::IsNullOrWhiteSpace($current)) {
        $parent = [IO.Directory]::GetParent($current)
        if ($null -eq $parent) { break }
        $directories.Add($current)
        $current = $parent.FullName
    }
    $guards = [Collections.Generic.List[object]]::new()
    try {
        for ($index = $directories.Count - 1; $index -ge 0; $index--) {
            $directory = Assert-RealDirectory `
                -Path $directories[$index] -Label "$Label ancestor"
            $guards.Add(
                [Rayman.CodexBrokerInstallerNative]::OpenDirectoryGuard(
                    $directory
                )
            )
        }
        return ,$guards
    } catch {
        for ($index = $guards.Count - 1; $index -ge 0; $index--) {
            $guards[$index].Dispose()
        }
        throw
    }
}

function Close-BrokerPinnedFile {
    param([Parameter(Mandatory = $true)]$Held)
    try { $Held.Stream.Dispose() } finally {
        for ($index = $Held.DirectoryGuards.Count - 1; $index -ge 0; $index--) {
            $Held.DirectoryGuards[$index].Dispose()
        }
    }
}

function Open-BrokerPinnedFileHandle {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$Label,
        [ValidateRange(0, 10000)]
        [int]$RetryTimeoutMilliseconds = 3000,
        [ValidateRange(1, 1000)]
        [int]$RetryDelayMilliseconds = 25
    )
    $timer = [Diagnostics.Stopwatch]::StartNew()
    $attempts = 0
    while ($true) {
        $attempts++
        try {
            return [Rayman.CodexBrokerInstallerNative]::OpenPinnedFile($Path)
        } catch {
            $nativeException = $_.Exception
            while ($null -ne $nativeException -and
                $nativeException -isnot [ComponentModel.Win32Exception]) {
                $nativeException = $nativeException.InnerException
            }
            if ($null -eq $nativeException) {
                throw
            }
            $nativeError = [int]$nativeException.NativeErrorCode
            $transient = $nativeError -in @(32, 33)
            if (-not $transient -or
                $timer.ElapsedMilliseconds -ge $RetryTimeoutMilliseconds) {
                $message = "$Label pinned open failed path=$Path " +
                    "win32=$nativeError attempts=$attempts " +
                    "elapsed_ms=$($timer.ElapsedMilliseconds)"
                throw [IO.IOException]::new($message, $_.Exception)
            }
            Start-Sleep -Milliseconds $RetryDelayMilliseconds
        }
    }
}

function Open-BrokerPinnedFile {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$Label,
        [long]$MaximumBytes = 1MB,
        [string]$ExpectedSha256,
        [string]$ExpectedIdentity,
        [string]$ExpectedOwnerSid,
        [string]$ExpectedAccessSddl,
        [switch]$RequireProtectedAcl,
        [scriptblock]$BeforeLeafOpenSelfTestAction,
        [ValidateRange(0, 10000)]
        [int]$OpenRetryTimeoutMilliseconds = 3000,
        [ValidateRange(1, 1000)]
        [int]$OpenRetryDelayMilliseconds = 25
    )
    if ($null -ne $BeforeLeafOpenSelfTestAction -and -not $SelfTest) {
        throw "$Label cannot use the self-test leaf-open action in production."
    }
    $fullPath = [IO.Path]::GetFullPath($Path)
    $directoryGuards = Open-BrokerDirectoryGuardChain `
        -FilePath $fullPath -Label $Label
    $handle = $null
    $stream = $null
    try {
        if ($null -ne $BeforeLeafOpenSelfTestAction) {
            [void](& $BeforeLeafOpenSelfTestAction)
        }
        $handle = Open-BrokerPinnedFileHandle `
            -Path $fullPath -Label $Label `
            -RetryTimeoutMilliseconds $OpenRetryTimeoutMilliseconds `
            -RetryDelayMilliseconds $OpenRetryDelayMilliseconds
        $stream = [IO.FileStream]::new($handle, [IO.FileAccess]::Read)
        $handle = $null
        $metadata = Get-BrokerHeldFileMetadata `
            -Stream $stream -MaximumBytes $MaximumBytes -Label $Label
        if ($ExpectedSha256 -and
            [string]$metadata.Sha256 -cne $ExpectedSha256) {
            throw "$Label hash drift: actual=$($metadata.Sha256) expected=$ExpectedSha256"
        }
        if ($ExpectedIdentity -and
            [string]$metadata.Identity -cne $ExpectedIdentity) {
            throw "$Label identity drift: actual=$($metadata.Identity) expected=$ExpectedIdentity"
        }
        if ($ExpectedOwnerSid -and
            [string]$metadata.OwnerSid -cne $ExpectedOwnerSid) {
            throw "$Label owner drift: actual=$($metadata.OwnerSid) expected=$ExpectedOwnerSid"
        }
        if ($ExpectedAccessSddl -and
            [string]$metadata.AccessSddl -cne $ExpectedAccessSddl) {
            throw "$Label DACL drift."
        }
        if ($RequireProtectedAcl -and -not $metadata.AccessRulesProtected) {
            throw "$Label DACL is not protected."
        }
        return [pscustomobject]@{
            Path = $fullPath
            Stream = $stream
            DirectoryGuards = $directoryGuards
            Bytes = $metadata.Bytes
            Sha256 = $metadata.Sha256
            Identity = $metadata.Identity
            OwnerSid = $metadata.OwnerSid
            AccessSddl = $metadata.AccessSddl
            AccessRulesProtected = $metadata.AccessRulesProtected
        }
    } catch {
        if ($null -ne $stream) { $stream.Dispose() }
        if ($null -ne $handle) { $handle.Dispose() }
        for ($index = $directoryGuards.Count - 1; $index -ge 0; $index--) {
            $directoryGuards[$index].Dispose()
        }
        throw
    }
}

function Get-BrokerFileSnapshot {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$Label,
        [long]$MaximumBytes = 1MB
    )
    $held = Open-BrokerPinnedFile `
        -Path $Path -Label $Label -MaximumBytes $MaximumBytes
    try {
        return [pscustomobject]@{
            Path = $held.Path
            Bytes = $held.Bytes
            Sha256 = $held.Sha256
            Identity = $held.Identity
            OwnerSid = $held.OwnerSid
            AccessSddl = $held.AccessSddl
            AccessRulesProtected = $held.AccessRulesProtected
        }
    } finally { Close-BrokerPinnedFile -Held $held }
}

function Get-BrokerOwnedTreeFileSnapshot {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$Label,
        [long]$MaximumBytes = 1MB
    )
    # New-BrokerOwnedTreeSnapshot already retains every directory ancestor
    # with delete sharing denied. Reopening those directories through the
    # generic guard chain would conflict with the retained DELETE access.
    $handle = Open-BrokerPinnedFileHandle -Path $Path -Label $Label
    $stream = $null
    try {
        $stream = [IO.FileStream]::new($handle, [IO.FileAccess]::Read)
        $handle = $null
        $metadata = Get-BrokerHeldFileMetadata `
            -Stream $stream -MaximumBytes $MaximumBytes -Label $Label
        if ((Get-StrongPathIdentity -Path $Path -Directory $false) -cne
                [string]$metadata.Identity) {
            throw "$Label file namespace changed after held open."
        }
        return [pscustomobject]@{
            Path = [IO.Path]::GetFullPath($Path)
            Bytes = $metadata.Bytes
            Sha256 = $metadata.Sha256
            Identity = $metadata.Identity
            OwnerSid = $metadata.OwnerSid
            AccessSddl = $metadata.AccessSddl
            AccessRulesProtected = $metadata.AccessRulesProtected
        }
    } finally {
        if ($null -ne $stream) { $stream.Dispose() }
        elseif ($null -ne $handle) { $handle.Dispose() }
    }
}

function Remove-BrokerFileExact {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$ExpectedSha256,
        [Parameter(Mandatory = $true)][string]$ExpectedIdentity,
        [Parameter(Mandatory = $true)][string]$ExpectedOwnerSid,
        [Parameter(Mandatory = $true)][string]$ExpectedAccessSddl,
        [Parameter(Mandatory = $true)][string]$Label,
        [long]$MaximumBytes = 1MB
    )
    if (-not (Test-Path -LiteralPath $Path)) { return $false }
    [void](Assert-NoReparseAncestors -Path $Path -Label $Label -RequireLeaf)
    $handle = [Rayman.CodexBrokerInstallerNative]::OpenDeleteHandle($Path)
    $stream = $null
    try {
        $stream = [IO.FileStream]::new($handle, [IO.FileAccess]::Read)
        $metadata = Get-BrokerHeldFileMetadata `
            -Stream $stream -MaximumBytes $MaximumBytes -Label $Label
        if ([string]$metadata.Sha256 -cne $ExpectedSha256 -or
            [string]$metadata.Identity -cne $ExpectedIdentity -or
            [string]$metadata.OwnerSid -cne $ExpectedOwnerSid -or
            [string]$metadata.AccessSddl -cne $ExpectedAccessSddl) {
            throw "$Label exact-delete binding drifted; preserved the current path."
        }
        [Rayman.CodexBrokerInstallerNative]::MarkDelete($stream.SafeFileHandle)
    } finally {
        if ($null -ne $stream) { $stream.Dispose() } else { $handle.Dispose() }
    }
    if (Test-Path -LiteralPath $Path) {
        throw "$Label exact-delete did not remove the bound namespace entry: $Path"
    }
    return $true
}

function Remove-BrokerDirectoryExact {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$ExpectedIdentity,
        [Parameter(Mandatory = $true)][string]$ExpectedOwnerSid,
        [Parameter(Mandatory = $true)][string]$ExpectedAccessSddl,
        [Parameter(Mandatory = $true)][string]$Label
    )
    if (-not (Test-Path -LiteralPath $Path)) { return $false }
    [void](Assert-NoReparseAncestors `
        -Path $Path -Label $Label -RequireLeaf)
    $handle = [Rayman.CodexBrokerInstallerNative]::OpenDeleteHandle(
        $Path, $true
    )
    try {
        $identity = [Rayman.CodexBrokerInstallerNative]::StrongIdentity($handle)
        if ((Get-StrongPathIdentity -Path $Path -Directory $true) -cne
                $identity) {
            throw "$Label directory namespace changed after held open."
        }
        $security = Get-Acl -LiteralPath $Path -ErrorAction Stop
        $ownerSid = [string]$security.GetOwner(
            [Security.Principal.SecurityIdentifier]
        ).Value
        $accessSddl = $security.GetSecurityDescriptorSddlForm(
            [Security.AccessControl.AccessControlSections]::Access
        )
        if ((Get-StrongPathIdentity -Path $Path -Directory $true) -cne
                $identity -or
            $identity -cne $ExpectedIdentity -or
            $ownerSid -cne $ExpectedOwnerSid -or
            $accessSddl -cne $ExpectedAccessSddl) {
            throw "$Label exact-directory binding drifted; preserved the path."
        }
        if (@(Get-ChildItem -LiteralPath $Path -Force -ErrorAction Stop).Count -ne 0) {
            throw "$Label exact-directory target is not empty; preserved the path."
        }
        [Rayman.CodexBrokerInstallerNative]::MarkDelete($handle)
    } finally { $handle.Dispose() }
    if (Test-Path -LiteralPath $Path) {
        throw "$Label exact-directory delete did not remove the bound object."
    }
    return $true
}

function Get-BrokerOwnedDirectoryBinding {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$ExpectedOwnerSid,
        [Parameter(Mandatory = $true)][string]$Label
    )
    $verified = Assert-RealDirectory -Path $Path -Label $Label
    $guard = [Rayman.CodexBrokerInstallerNative]::OpenOwnedDirectoryDeleteGuard(
        $verified
    )
    try {
        $identity = [Rayman.CodexBrokerInstallerNative]::StrongIdentity($guard)
        if ((Get-StrongPathIdentity -Path $verified -Directory $true) -cne
                $identity) {
            throw "$Label directory namespace changed after held open."
        }
        $security = Get-Acl -LiteralPath $verified -ErrorAction Stop
        $ownerSid = [string]$security.GetOwner(
            [Security.Principal.SecurityIdentifier]
        ).Value
        if ($ownerSid -cne $ExpectedOwnerSid) {
            throw "$Label owner drifted: actual=$ownerSid expected=$ExpectedOwnerSid"
        }
        return [pscustomobject]@{
            path = $verified
            type = 'directory'
            sha256 = $null
            identity = $identity
            owner_sid = $ownerSid
            access_sddl = $security.GetSecurityDescriptorSddlForm(
                [Security.AccessControl.AccessControlSections]::Access
            )
            access_rules_protected = [bool]$security.AreAccessRulesProtected
            handle = $guard
        }
    } catch {
        $guard.Dispose()
        throw
    }
}

function New-BrokerOwnedTreeSnapshot {
    param(
        [Parameter(Mandatory = $true)][string]$Root,
        [Parameter(Mandatory = $true)][string]$ExpectedOwnerSid,
        [ValidateRange(1, 4096)][int]$MaximumEntries = 4096,
        [ValidateRange(1, 1073741824)][long]$MaximumTotalBytes = 256MB
    )
    $rootPath = Assert-RealDirectory -Path $Root `
        -Label 'Broker owned-tree root'
    $rootBinding = Get-BrokerOwnedDirectoryBinding -Path $rootPath `
        -ExpectedOwnerSid $ExpectedOwnerSid -Label 'Broker owned-tree root'
    $rootGuard = $rootBinding.handle
    $files = [Collections.Generic.List[object]]::new()
    $directories = [Collections.Generic.List[object]]::new()
    $pending = [Collections.Generic.Stack[string]]::new()
    $pending.Push($rootPath)
    $entryCount = 0
    $totalBytes = [long]0
    try {
        while ($pending.Count -gt 0) {
            $directory = $pending.Pop()
            foreach ($entry in @(Get-ChildItem -LiteralPath $directory `
                    -Force -ErrorAction Stop)) {
                $entryCount++
                if ($entryCount -gt $MaximumEntries) {
                    throw 'Broker owned tree exceeds the bounded entry count.'
                }
                $path = Assert-ChildPath -Child $entry.FullName `
                    -Parent $rootPath -Label 'Broker owned-tree entry'
                if (($entry.Attributes -band
                        [IO.FileAttributes]::ReparsePoint) -ne 0) {
                    throw "Broker owned tree contains a reparse entry: $path"
                }
                if ($entry.PSIsContainer) {
                    $binding = Get-BrokerOwnedDirectoryBinding -Path $path `
                        -ExpectedOwnerSid $ExpectedOwnerSid `
                        -Label 'Broker owned-tree directory'
                    $directories.Add($binding)
                    $pending.Push($path)
                } else {
                    $snapshot = Get-BrokerOwnedTreeFileSnapshot -Path $path `
                        -Label 'Broker owned-tree file' -MaximumBytes 64MB
                    if ([string]$snapshot.OwnerSid -cne $ExpectedOwnerSid) {
                        throw "Broker owned-tree file owner drifted: $path"
                    }
                    $totalBytes += [long]$snapshot.Bytes.Length
                    if ($totalBytes -gt $MaximumTotalBytes) {
                        throw 'Broker owned tree exceeds the bounded byte count.'
                    }
                    $files.Add([pscustomobject]@{
                        path = $snapshot.Path
                        type = 'file'
                        sha256 = $snapshot.Sha256
                        identity = $snapshot.Identity
                        owner_sid = $snapshot.OwnerSid
                        access_sddl = $snapshot.AccessSddl
                        access_rules_protected =
                            $snapshot.AccessRulesProtected
                    })
                }
            }
        }
        return [pscustomobject]@{
            Root = $rootBinding
            RootGuard = $rootGuard
            Files = @($files)
            Directories = @($directories)
            EntryCount = $entryCount
            TotalBytes = $totalBytes
        }
    } catch {
        foreach ($directory in @($directories)) {
            if ($null -ne $directory.handle -and
                -not $directory.handle.IsClosed) {
                $directory.handle.Dispose()
            }
        }
        if ($null -ne $rootGuard -and -not $rootGuard.IsClosed) {
            $rootGuard.Dispose()
        }
        throw
    }
}

function Close-BrokerOwnedTreeSnapshot {
    param([Parameter(Mandatory = $true)]$Snapshot)
    foreach ($directory in @($Snapshot.Directories)) {
        if ($null -ne $directory.handle -and
            -not $directory.handle.IsClosed) {
            $directory.handle.Dispose()
        }
    }
    if ($null -ne $Snapshot.Root.handle -and
        -not $Snapshot.Root.handle.IsClosed) {
        $Snapshot.Root.handle.Dispose()
    }
}

function Remove-BrokerOwnedDirectoryHeld {
    param(
        [Parameter(Mandatory = $true)]$Binding,
        [Parameter(Mandatory = $true)][string]$Label
    )
    $handle = $Binding.handle
    if ($null -eq $handle -or $handle.IsClosed -or $handle.IsInvalid) {
        throw "$Label retained delete handle is unavailable."
    }
    $path = [string]$Binding.path
    $identity = [Rayman.CodexBrokerInstallerNative]::StrongIdentity($handle)
    if ((Get-StrongPathIdentity -Path $path -Directory $true) -cne
            $identity) {
        throw "$Label directory namespace changed while its handle was held."
    }
    $security = Get-Acl -LiteralPath $path -ErrorAction Stop
    $ownerSid = [string]$security.GetOwner(
        [Security.Principal.SecurityIdentifier]
    ).Value
    $accessSddl = $security.GetSecurityDescriptorSddlForm(
        [Security.AccessControl.AccessControlSections]::Access
    )
    if ($identity -cne [string]$Binding.identity -or
        $ownerSid -cne [string]$Binding.owner_sid -or
        $accessSddl -cne [string]$Binding.access_sddl -or
        @(Get-ChildItem -LiteralPath $path -Force -ErrorAction Stop).Count -ne 0) {
        throw "$Label retained directory binding drifted or is not empty; preserved the path."
    }
    [Rayman.CodexBrokerInstallerNative]::MarkDelete($handle)
    $handle.Dispose()
    $Binding.handle = $null
    if (Test-Path -LiteralPath $path) {
        throw "$Label retained-handle delete did not remove the bound directory."
    }
}

function Remove-BrokerOwnedTreeExact {
    param([Parameter(Mandatory = $true)]$Snapshot)
    try {
        foreach ($file in @($Snapshot.Files | Sort-Object `
                @{ Expression = { ([string]$_.path).Length }; Descending = $true })) {
            [void](Remove-BrokerFileExact -Path ([string]$file.path) `
                -ExpectedSha256 ([string]$file.sha256) `
                -ExpectedIdentity ([string]$file.identity) `
                -ExpectedOwnerSid ([string]$file.owner_sid) `
                -ExpectedAccessSddl ([string]$file.access_sddl) `
                -Label 'Broker uninstall owned file' -MaximumBytes 64MB)
        }
        foreach ($directory in @($Snapshot.Directories | Sort-Object `
                @{ Expression = { ([string]$_.path).Length }; Descending = $true })) {
            Remove-BrokerOwnedDirectoryHeld -Binding $directory `
                -Label 'Broker uninstall owned directory'
        }
        Remove-BrokerOwnedDirectoryHeld -Binding $Snapshot.Root `
            -Label 'Broker uninstall owned root'
    } finally {
        Close-BrokerOwnedTreeSnapshot -Snapshot $Snapshot
    }
}

function Get-BrokerInstallationMutexName {
    param([Parameter(Mandatory = $true)][string]$Root)
    $normalized = [IO.Path]::GetFullPath($Root).TrimEnd(
        [IO.Path]::DirectorySeparatorChar,
        [IO.Path]::AltDirectorySeparatorChar
    ).ToUpperInvariant()
    $hash = Get-BytesSha256 -Bytes (
        [Text.UTF8Encoding]::new($false, $true).GetBytes($normalized)
    )
    return 'Global\Rayman.CodexPowerShellBroker.Install.' + $hash
}

function Get-BrokerInstallationMutexTrust {
    $fullControlSids = @('S-1-5-18', 'S-1-5-32-544')
    $participantSids = [Collections.Generic.HashSet[string]]::new(
        [StringComparer]::Ordinal
    )
    $allowedOwnerSids = [Collections.Generic.HashSet[string]]::new(
        [StringComparer]::Ordinal
    )
    foreach ($sid in $fullControlSids) {
        [void]$allowedOwnerSids.Add($sid)
    }
    $ownerAccounts = [Collections.Generic.HashSet[string]]::new(
        [StringComparer]::OrdinalIgnoreCase
    )
    foreach ($account in @(
        'QIN5521\qinrm',
        'QIN5521\CodexSandboxOffline',
        'QIN5521\CodexSandboxOnline'
    )) {
        [void]$ownerAccounts.Add($account)
    }
    foreach ($account in @(
        'QIN5521\qinrm',
        'QIN5521\CodexSandboxOffline',
        'QIN5521\CodexSandboxOnline',
        'QIN5521\CodexSandboxUsers'
    )) {
        try {
            $sid = [string]([Security.Principal.NTAccount]::new(
                $account
            ).Translate([Security.Principal.SecurityIdentifier]).Value)
            [void]$participantSids.Add($sid)
            if ($ownerAccounts.Contains($account)) {
                [void]$allowedOwnerSids.Add($sid)
            }
        } catch [Security.Principal.IdentityNotMappedException] { }
    }
    return [pscustomobject]@{
        FullControlSids = @($fullControlSids)
        ParticipantSids = @($participantSids | Sort-Object)
        AllowedOwnerSids = @($allowedOwnerSids | Sort-Object)
    }
}

function New-BrokerInstallationMutexSecurity {
    $security = [Security.AccessControl.MutexSecurity]::new()
    $security.SetAccessRuleProtection($true, $false)
    $trust = Get-BrokerInstallationMutexTrust
    $entries = [Collections.Generic.List[object]]::new()
    foreach ($sid in @($trust.FullControlSids)) {
        $entries.Add(@($sid, [Security.AccessControl.MutexRights]::FullControl))
    }
    foreach ($sid in @($trust.ParticipantSids)) {
        $entries.Add(@($sid, (
            [Security.AccessControl.MutexRights]::Synchronize -bor
            [Security.AccessControl.MutexRights]::Modify -bor
            [Security.AccessControl.MutexRights]::ReadPermissions
        )))
    }
    foreach ($entry in $entries) {
        $security.AddAccessRule(
            [Security.AccessControl.MutexAccessRule]::new(
                [Security.Principal.SecurityIdentifier]::new($entry[0]),
                [Security.AccessControl.MutexRights]$entry[1],
                [Security.AccessControl.AccessControlType]::Allow
            )
        )
    }
    return $security
}

function Assert-BrokerInstallationMutexSecurityContract {
    param(
        [Parameter(Mandatory = $true)]
        [Security.AccessControl.MutexSecurity]$ActualSecurity,
        [Parameter(Mandatory = $true)]
        [Security.AccessControl.MutexSecurity]$ExpectedSecurity,
        [Parameter(Mandatory = $true)][string]$Name
    )
    $expectedAccess = $ExpectedSecurity.GetSecurityDescriptorSddlForm(
        [Security.AccessControl.AccessControlSections]::Access
    )
    $actualAccess = $ActualSecurity.GetSecurityDescriptorSddlForm(
        [Security.AccessControl.AccessControlSections]::Access
    )
    $actualOwner = [string]$ActualSecurity.GetOwner(
        [Security.Principal.SecurityIdentifier]
    ).Value
    $allowedOwners = [Collections.Generic.HashSet[string]]::new(
        [StringComparer]::Ordinal
    )
    foreach ($sid in @((Get-BrokerInstallationMutexTrust).AllowedOwnerSids)) {
        [void]$allowedOwners.Add([string]$sid)
    }
    if (-not $ActualSecurity.AreAccessRulesProtected) {
        throw "Broker installation mutex dacl_not_protected: $Name"
    }
    if (-not $allowedOwners.Contains($actualOwner)) {
        throw "Broker installation mutex owner_not_allowed actual=$actualOwner`: $Name"
    }
    if ($actualAccess -cne $expectedAccess) {
        throw "Broker installation mutex dacl_mismatch: $Name"
    }
}

function Open-BrokerInstallationGuard {
    param(
        [Parameter(Mandatory = $true)][string]$Root,
        [int]$TimeoutMilliseconds = 30000,
        [switch]$AllowBusy
    )
    $name = Get-BrokerInstallationMutexName -Root $Root
    $expectedSecurity = New-BrokerInstallationMutexSecurity
    $participantRights = [Security.AccessControl.MutexRights]::Synchronize -bor
        [Security.AccessControl.MutexRights]::Modify -bor
        [Security.AccessControl.MutexRights]::ReadPermissions
    $created = $false
    $mutex = $null
    try {
        if (-not [Threading.MutexAcl]::TryOpenExisting(
                $name, $participantRights, [ref]$mutex
            )) {
            $mutex = [Threading.MutexAcl]::Create(
                $false, $name, [ref]$created, $expectedSecurity
            )
        }
    } catch {
        throw "Broker installation mutex creation/open failed closed: $($_.Exception.Message)"
    }
    try {
        $actualSecurity = [Security.AccessControl.MutexSecurity]::new(
            $name, (
                [Security.AccessControl.AccessControlSections]::Access -bor
                [Security.AccessControl.AccessControlSections]::Owner
            )
        )
        Assert-BrokerInstallationMutexSecurityContract `
            -ActualSecurity $actualSecurity -ExpectedSecurity $expectedSecurity `
            -Name $name
    } catch {
        $mutex.Dispose()
        throw
    }
    $acquired = $false
    try {
        try { $acquired = $mutex.WaitOne($TimeoutMilliseconds) }
        catch [Threading.AbandonedMutexException] { $acquired = $true }
        if (-not $acquired -and -not $AllowBusy) {
            throw "Broker installation transaction is already active: $name"
        }
        return [pscustomobject]@{
            Name = $name
            Mutex = $mutex
            Acquired = $acquired
        }
    } catch {
        if ($acquired) { try { $mutex.ReleaseMutex() } catch { } }
        $mutex.Dispose()
        throw
    }
}

function Close-BrokerInstallationGuard {
    param([Parameter(Mandatory = $true)]$Guard)
    try {
        if ([bool]$Guard.Acquired) { $Guard.Mutex.ReleaseMutex() }
    } finally { $Guard.Mutex.Dispose() }
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

function Get-StrongPathIdentity {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][bool]$Directory
    )
    return [Rayman.CodexBrokerInstallerNative]::StrongIdentity($Path, $Directory)
}

function Open-InstallerDirectoryGuard {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$Label
    )
    $verified = Assert-RealDirectory -Path $Path -Label $Label
    return [Rayman.CodexBrokerInstallerNative]::OpenDirectoryGuard($verified)
}

function Get-RegisteredGitExecutable {
    $shim = 'C:\Program Files\Git\cmd\git.exe'
    if (-not (Test-Path -LiteralPath $shim -PathType Leaf)) {
        throw "git_local_commit_v1 fixed Git for Windows shim is missing: $shim"
    }
    $shimFull = Get-NormalizedAbsolutePath -Path $shim -Label 'Git command shim'
    if (-not $shimFull.EndsWith('\Git\cmd\git.exe',
        [StringComparison]::OrdinalIgnoreCase)) {
        throw "git_local_commit_v1 requires the installed Git for Windows cmd shim: $shimFull"
    }
    $gitRoot = Split-Path -Parent (Split-Path -Parent $shimFull)
    $executable = Join-Path $gitRoot 'mingw64\bin\git.exe'
    $item = Get-Item -LiteralPath $executable -Force -ErrorAction Stop
    if ($item.PSIsContainer -or
        ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
        throw 'git_local_commit_v1 direct Git executable is not a regular file.'
    }
    $signature = Get-AuthenticodeSignature -LiteralPath $item.FullName
    if ([string]$signature.Status -cne 'Valid' -or
        $null -eq $signature.SignerCertificate) {
        throw 'git_local_commit_v1 direct Git executable lacks a valid Authenticode signature.'
    }
    return [pscustomobject]@{
        Path = $item.FullName
        Sha256 = Get-FileSha256 -Path $item.FullName
        Identity = Get-StrongPathIdentity -Path $item.FullName -Directory $false
        SignerSubject = [string]$signature.SignerCertificate.Subject
        SignerThumbprint = [string]$signature.SignerCertificate.Thumbprint
    }
}

function Assert-GitLocalConfigSafe {
    param([Parameter(Mandatory = $true)][string]$Path)

    $sections = @{
        core = @{
            repositoryformatversion = '0'; filemode = 'false'; bare = 'false'
            logallrefupdates = 'true'; ignorecase = 'true'
        }
        'remote "origin"' = @{
            url = 'https://github.com/qinrm-lab/RaymanCodingSkill.git'
            fetch = '+refs/heads/*:refs/remotes/origin/*'
        }
        'branch "main"' = @{
            remote = 'origin'; merge = 'refs/heads/main'
            'vscode-merge-base' = 'origin/main'
        }
    }
    $observed = @{}
    $section = $null
    foreach ($line in [IO.File]::ReadAllLines($Path, [Text.UTF8Encoding]::new($false, $true))) {
        if ([string]::IsNullOrWhiteSpace($line)) { continue }
        if ($line -match '^\[([^]]+)\]$') {
            $section = [string]$Matches[1]
            if (-not $sections.ContainsKey($section) -or $observed.ContainsKey($section)) {
                throw "git_local_commit_v1 rejects local Git config section: $section"
            }
            $observed[$section] = @{}
            continue
        }
        if ($null -eq $section -or $line -notmatch '^\s*([A-Za-z0-9.-]+)\s*=\s*(.*)$') {
            throw 'git_local_commit_v1 rejects malformed or continuation local Git config.'
        }
        $key = [string]$Matches[1]
        $value = [string]$Matches[2]
        if (-not $sections[$section].ContainsKey($key) -or
            $observed[$section].ContainsKey($key) -or
            [string]$sections[$section][$key] -cne $value) {
            throw "git_local_commit_v1 rejects local Git config key/value: $section.$key"
        }
        $observed[$section][$key] = $value
    }
    foreach ($name in @($sections.Keys)) {
        if (-not $observed.ContainsKey($name) -or
            $observed[$name].Count -ne $sections[$name].Count) {
            throw "git_local_commit_v1 local Git config is incomplete: $name"
        }
    }
}

function Assert-RepositoryRegistrationSafe {
    param(
        [Parameter(Mandatory = $true)][string]$Root,
        [Parameter(Mandatory = $true)]$Git
    )

    $rootFull = Get-NormalizedAbsolutePath -Path $Root -Label 'Registered repository root'
    $rootItem = Get-Item -LiteralPath $rootFull -Force -ErrorAction Stop
    $gitDir = Join-Path $rootFull '.git'
    $gitItem = Get-Item -LiteralPath $gitDir -Force -ErrorAction Stop
    if (-not $rootItem.PSIsContainer -or -not $gitItem.PSIsContainer -or
        ($rootItem.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0 -or
        ($gitItem.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
        throw 'git_local_commit_v1 repository root and .git must be real directories.'
    }
    foreach ($relative in @(
        'index.lock', 'MERGE_HEAD', 'CHERRY_PICK_HEAD', 'REVERT_HEAD', 'BISECT_LOG',
        'sequencer', 'rebase-apply', 'rebase-merge', 'worktrees', 'shallow',
        'info\grafts', 'info\sparse-checkout', 'objects\info\alternates',
        'refs\replace', 'modules'
    )) {
        if (Test-Path -LiteralPath (Join-Path $gitDir $relative)) {
            throw "git_local_commit_v1 registration rejects Git state: $relative"
        }
    }
    foreach ($hook in @(Get-ChildItem -LiteralPath (Join-Path $gitDir 'hooks') `
        -File -Force -ErrorAction Stop)) {
        if (-not $hook.Name.EndsWith('.sample', [StringComparison]::Ordinal)) {
            throw "git_local_commit_v1 registration rejects active hook: $($hook.Name)"
        }
    }
    $configPath = Join-Path $gitDir 'config'
    $rootAttributes = Join-Path $rootFull '.gitattributes'
    $infoAttributes = Join-Path (Join-Path $gitDir 'info') 'attributes'
    $infoExclude = Join-Path (Join-Path $gitDir 'info') 'exclude'
    foreach ($entry in @(
        @($configPath, 'Git config'),
        @($rootAttributes, '.gitattributes'),
        @($infoExclude, 'info/exclude')
    )) {
        [void](Assert-NoReparseAncestors `
            -Path $entry[0] -Label $entry[1] -RequireLeaf)
    }
    if ($null -ne (Get-Item -LiteralPath $infoAttributes -Force `
            -ErrorAction SilentlyContinue)) {
        throw 'git_local_commit_v1 registration requires info/attributes to be absent.'
    }
    Assert-GitLocalConfigSafe -Path $configPath
    $allowedAttributes = @(
        '* text=auto eol=lf', '*.png binary', '*.jpg binary', '*.jpeg binary',
        '*.gif binary', '*.ico binary', '*.pdf binary', '*.exe binary'
    )
    $actualAttributes = @(
        [IO.File]::ReadAllLines(
            $rootAttributes,
            [Text.UTF8Encoding]::new($false, $true)
        ) | Where-Object { -not [string]::IsNullOrWhiteSpace($_) }
    )
    if (($actualAttributes -join "`n") -cne ($allowedAttributes -join "`n")) {
        throw 'git_local_commit_v1 registration rejects unknown .gitattributes rules.'
    }
    $branchOutput = @(& $Git.Path -C $rootFull symbolic-ref -q HEAD 2>&1)
    $branchExit = $LASTEXITCODE
    $branch = if ($branchOutput.Count -eq 1) {
        [string]$branchOutput[0].Trim()
    } else { $null }
    if ($branchExit -ne 0 -or $branch -cne 'refs/heads/main') {
        throw "git_local_commit_v1 registration requires attached refs/heads/main: exit=$branchExit output=$($branchOutput -join ' | ')"
    }
    $formatOutput = @(& $Git.Path -C $rootFull rev-parse --show-object-format 2>&1)
    $formatExit = $LASTEXITCODE
    $objectFormat = if ($formatOutput.Count -eq 1) {
        [string]$formatOutput[0].Trim()
    } else { $null }
    if ($formatExit -ne 0 -or $objectFormat -cne 'sha1') {
        throw "git_local_commit_v1 supports only SHA-1 object-format repositories in v1: exit=$formatExit output=$($formatOutput -join ' | ')"
    }
    return [pscustomobject]@{
        Root = $rootFull
        RootIdentity = Get-StrongPathIdentity -Path $rootFull -Directory $true
        GitDir = $gitDir
        GitDirIdentity = Get-StrongPathIdentity -Path $gitDir -Directory $true
    }
}

function New-GitCapabilityManifest {
    param(
        [Parameter(Mandatory = $true)][string]$InstallId,
        [Parameter(Mandatory = $true)][string]$HooksRoot,
        [Parameter(Mandatory = $true)][string]$TransactionsRoot
    )

    $git = Get-RegisteredGitExecutable
    $repository = Assert-RepositoryRegistrationSafe `
        -Root $script:RepositoryRoot -Git $git
    $infoExclude = Join-Path (Join-Path $repository.GitDir 'info') 'exclude'
    if (-not (Test-Path -LiteralPath $infoExclude -PathType Leaf)) {
        throw 'git_local_commit_v1 registration requires the regular info/exclude file.'
    }
    return [ordered]@{
        schema_version = 1
        capability_id = $script:GitCapabilityId
        install_id = $InstallId
        repository_id = [Guid]::NewGuid().ToString('N')
        repository_root = [string]$repository.Root
        repository_root_identity = [string]$repository.RootIdentity
        git_dir = [string]$repository.GitDir
        git_dir_identity = [string]$repository.GitDirIdentity
        allowed_ref = 'refs/heads/main'
        git_executable_path = [string]$git.Path
        git_executable_sha256 = [string]$git.Sha256
        git_executable_identity = [string]$git.Identity
        git_signer_subject = [string]$git.SignerSubject
        git_signer_thumbprint = [string]$git.SignerThumbprint
        object_format = 'sha1'
        author_name = 'rayman'
        author_email = '32691594@qq.com'
        hooks_root = $HooksRoot
        transactions_root = $TransactionsRoot
        git_config_sha256 = Get-FileSha256 -Path (Join-Path $repository.GitDir 'config')
        gitattributes_sha256 = Get-FileSha256 -Path (Join-Path $repository.Root '.gitattributes')
        info_attributes_sha256 = $null
        info_exclude_sha256 = Get-FileSha256 -Path $infoExclude
        tracked_only = $true
        allow_untracked = $false
        allow_pre_staged = $false
        authorization_mode = 'persistent_install_grant'
        local_commit_only = $true
        push_allowed = $false
        confirmation_required = $false
        created_at_utc = [DateTimeOffset]::UtcNow.ToString('o')
    }
}

function New-GitCapabilityReadyDocument {
    param(
        [Parameter(Mandatory = $true)][string]$InstallId,
        [Parameter(Mandatory = $true)][string]$WorkerHash,
        [Parameter(Mandatory = $true)][string]$ManifestHash
    )

    return [ordered]@{
        schema_version = 1
        install_id = $InstallId
        worker_sha256 = $WorkerHash
        git_capability_manifest_sha256 = $ManifestHash
        ready_at_utc = [DateTimeOffset]::UtcNow.ToString('o')
    }
}

function Assert-GitCapabilityReadyInstalled {
    param(
        [Parameter(Mandatory = $true)]$Receipt,
        [Parameter(Mandatory = $true)][string]$Root
    )

    $state = Get-BrokerGitCapabilityReadyState -Receipt $Receipt -Root $Root
    if ([string]$state.State -cne 'valid') {
        throw "Git capability ready marker is not valid: $($state.Error)"
    }
    return $state.Document
}

function Get-BrokerGitCapabilityReadyState {
    param(
        [Parameter(Mandatory = $true)]$Receipt,
        [Parameter(Mandatory = $true)][string]$Root
    )
    $path = Join-Path $Root $script:GitCapabilityReadyName
    $item = Get-Item -LiteralPath $path -Force -ErrorAction SilentlyContinue
    if ($null -eq $item) {
        return [pscustomobject]@{
            State = 'absent'; Error = 'ready marker is absent'
            Document = $null; Snapshot = $null
        }
    }
    try {
        if ($item.PSIsContainer -or
            ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
            throw 'ready marker is not an ordinary file'
        }
        $snapshot = Get-BrokerFileSnapshot `
            -Path $path -Label 'Git capability ready marker' -MaximumBytes 64KB
        $fileSecurity = New-ManagedFileSecurity `
            -UserSid ([string]$Receipt.user_sid) `
            -SandboxSid ([string]$Receipt.sandbox_group_sid)
        $expectedAccess = $fileSecurity.GetSecurityDescriptorSddlForm(
            [Security.AccessControl.AccessControlSections]::Access
        )
        if (-not $snapshot.AccessRulesProtected -or
            [string]$snapshot.OwnerSid -cne [string]$Receipt.user_sid -or
            [string]$snapshot.AccessSddl -cne $expectedAccess) {
            throw 'ready marker held-handle owner/DACL drifted'
        }
        $ready = ConvertFrom-StrictJsonBytes `
            -Bytes $snapshot.Bytes -Label 'Git capability ready marker'
        Assert-ExactProperties -Document $ready -Expected @(
            'git_capability_manifest_sha256', 'install_id', 'ready_at_utc',
            'schema_version', 'worker_sha256'
        ) -Label 'Git capability ready marker'
        $readyAt = [DateTimeOffset]::MinValue
        if ([int]$ready.schema_version -ne 1 -or
            [string]$ready.install_id -cne [string]$Receipt.install_id -or
            [string]$ready.worker_sha256 -cne [string]$Receipt.worker_sha256 -or
            [string]$ready.git_capability_manifest_sha256 -cne
                [string]$Receipt.git_capability_manifest_sha256 -or
            -not [DateTimeOffset]::TryParse(
                [string]$ready.ready_at_utc,
                [Globalization.CultureInfo]::InvariantCulture,
                [Globalization.DateTimeStyles]::RoundtripKind,
                [ref]$readyAt
            ) -or $readyAt -gt [DateTimeOffset]::UtcNow.AddSeconds(30)) {
            throw 'ready marker does not match the installed tuple'
        }
        return [pscustomobject]@{
            State = 'valid'; Error = $null
            Document = $ready; Snapshot = $snapshot
        }
    } catch {
        return [pscustomobject]@{
            State = 'invalid'; Error = $_.Exception.Message
            Document = $null; Snapshot = $null
        }
    }
}

function Assert-GitCapabilityManifestInstalled {
    param(
        [Parameter(Mandatory = $true)]$Manifest,
        [Parameter(Mandatory = $true)]$Receipt,
        [Parameter(Mandatory = $true)][string]$Root,
        [switch]$SkipLiveRepository
    )

    Assert-ExactProperties -Document $Manifest `
        -Label 'git_local_commit_v1 capability manifest' -Expected @(
        'allow_pre_staged', 'allow_untracked', 'allowed_ref', 'author_email',
        'author_name', 'authorization_mode', 'capability_id', 'confirmation_required',
        'created_at_utc', 'git_config_sha256', 'git_dir', 'git_dir_identity',
        'git_executable_identity', 'git_executable_path', 'git_executable_sha256',
        'git_signer_subject', 'git_signer_thumbprint', 'gitattributes_sha256',
        'hooks_root', 'info_attributes_sha256', 'info_exclude_sha256', 'install_id',
        'local_commit_only', 'object_format', 'push_allowed', 'repository_id',
        'repository_root', 'repository_root_identity', 'schema_version',
        'tracked_only', 'transactions_root'
    )
    if ([int]$Manifest.schema_version -ne 1 -or
        [string]$Manifest.capability_id -cne $script:GitCapabilityId -or
        [string]$Manifest.install_id -cne [string]$Receipt.install_id -or
        [string]$Manifest.repository_root -cne $script:RepositoryRoot -or
        [string]$Manifest.git_dir -cne (Join-Path $script:RepositoryRoot '.git') -or
        [string]$Manifest.allowed_ref -cne 'refs/heads/main' -or
        [string]$Manifest.author_name -cne 'rayman' -or
        [string]$Manifest.author_email -cne '32691594@qq.com' -or
        [string]$Manifest.authorization_mode -cne 'persistent_install_grant' -or
        [bool]$Manifest.local_commit_only -ne $true -or
        [bool]$Manifest.push_allowed -ne $false -or
        [bool]$Manifest.confirmation_required -ne $false -or
        [bool]$Manifest.tracked_only -ne $true -or
        [bool]$Manifest.allow_untracked -ne $false -or
        [bool]$Manifest.allow_pre_staged -ne $false -or
        $null -ne $Manifest.info_attributes_sha256 -or
        [string]$Manifest.hooks_root -cne (Join-Path $Root 'empty-hooks') -or
        [string]$Manifest.transactions_root -cne
            (Join-Path $Root $script:GitTransactionDirectoryName)) {
        throw 'git_local_commit_v1 capability manifest authority binding is invalid.'
    }
    $repositoryId = [Guid]::Empty
    if (-not [Guid]::TryParseExact(
        [string]$Manifest.repository_id, 'N', [ref]$repositoryId
    )) { throw 'git_local_commit_v1 repository_id is not canonical.' }
    if ($SkipLiveRepository) { return }
    $git = Get-RegisteredGitExecutable
    $repository = Assert-RepositoryRegistrationSafe `
        -Root $script:RepositoryRoot -Git $git
    $infoAttributes = Join-Path (Join-Path $repository.GitDir 'info') 'attributes'
    if ([string]$Manifest.repository_root_identity -cne $repository.RootIdentity -or
        [string]$Manifest.git_dir_identity -cne $repository.GitDirIdentity -or
        [string]$Manifest.git_executable_path -cne $git.Path -or
        [string]$Manifest.git_executable_sha256 -cne $git.Sha256 -or
        [string]$Manifest.git_executable_identity -cne $git.Identity -or
        [string]$Manifest.git_signer_subject -cne $git.SignerSubject -or
        [string]$Manifest.git_signer_thumbprint -cne $git.SignerThumbprint -or
        [string]$Manifest.git_config_sha256 -cne
            (Get-FileSha256 -Path (Join-Path $repository.GitDir 'config')) -or
        [string]$Manifest.gitattributes_sha256 -cne
            (Get-FileSha256 -Path (Join-Path $repository.Root '.gitattributes')) -or
        [string]$Manifest.info_exclude_sha256 -cne
            (Get-FileSha256 -Path (Join-Path (Join-Path $repository.GitDir 'info') 'exclude')) -or
        $null -ne (Get-Item -LiteralPath $infoAttributes -Force `
            -ErrorAction SilentlyContinue)) {
        throw 'git_local_commit_v1 capability manifest live registration drifted.'
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

function Assert-BrokerExplicitConfirmation {
    param(
        [Parameter(Mandatory = $true)][bool]$Confirmed,
        [Parameter(Mandatory = $true)][string]$Action
    )
    if (-not $Confirmed) {
        throw "$Action requires explicit -Yes; -Yes:`$false is not confirmation."
    }
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

function Get-BrokerFileAccessRuleSemanticKey {
    param(
        [Parameter(Mandatory = $true)]
        [Security.AccessControl.FileSystemAccessRule]$Rule
    )

    return ('{0}|{1}|{2}|{3}|{4}' -f
        [string]$Rule.IdentityReference.Value,
        [int]$Rule.AccessControlType,
        [int64]$Rule.FileSystemRights,
        [int]$Rule.InheritanceFlags,
        [int]$Rule.PropagationFlags)
}

function Test-BrokerExactInheritedFileSecurity {
    param(
        [Parameter(Mandatory = $true)][string]$AccessSddl,
        [Parameter(Mandatory = $true)]
        [Security.AccessControl.FileSecurity]$Expected
    )

    try {
        $actual = [Security.AccessControl.FileSecurity]::new()
        $actual.SetSecurityDescriptorSddlForm(
            $AccessSddl,
            [Security.AccessControl.AccessControlSections]::Access
        )
        $actualRules = @($actual.GetAccessRules(
            $true, $true, [Security.Principal.SecurityIdentifier]
        ))
        $expectedRules = @($Expected.GetAccessRules(
            $true, $true, [Security.Principal.SecurityIdentifier]
        ))
        if ($actualRules.Count -eq 0 -or
            $actualRules.Count -ne $expectedRules.Count) {
            return $false
        }
        $actualKeys = [Collections.Generic.List[string]]::new()
        foreach ($rule in $actualRules) {
            if (-not $rule.IsInherited) { return $false }
            [void]$actualKeys.Add((
                Get-BrokerFileAccessRuleSemanticKey -Rule $rule
            ))
        }
        $expectedKeys = [Collections.Generic.List[string]]::new()
        foreach ($rule in $expectedRules) {
            if ($rule.IsInherited) { return $false }
            [void]$expectedKeys.Add((
                Get-BrokerFileAccessRuleSemanticKey -Rule $rule
            ))
        }
        $actualArray = [string[]]$actualKeys.ToArray()
        $expectedArray = [string[]]$expectedKeys.ToArray()
        [Array]::Sort($actualArray, [StringComparer]::Ordinal)
        [Array]::Sort($expectedArray, [StringComparer]::Ordinal)
        for ($index = 0; $index -lt $actualArray.Count; $index++) {
            if ($actualArray[$index] -cne $expectedArray[$index]) {
                return $false
            }
        }
        return $true
    } catch {
        return $false
    }
}

function Assert-TerminalGitArtifactSecurity {
    param(
        [Parameter(Mandatory = $true)]$Snapshot,
        [Parameter(Mandatory = $true)]
        [Security.AccessControl.FileSecurity]$Expected,
        [Parameter(Mandatory = $true)][string]$ExpectedOwnerSid,
        [Parameter(Mandatory = $true)][string]$Label
    )

    $expectedAccess = $Expected.GetSecurityDescriptorSddlForm(
        [Security.AccessControl.AccessControlSections]::Access
    )
    $protectedExact = [bool]$Snapshot.AccessRulesProtected -and
        [string]$Snapshot.AccessSddl -ceq $expectedAccess
    $inheritedExact = -not [bool]$Snapshot.AccessRulesProtected -and
        (Test-BrokerExactInheritedFileSecurity `
            -AccessSddl ([string]$Snapshot.AccessSddl) -Expected $Expected)
    if ([string]$Snapshot.OwnerSid -cne $ExpectedOwnerSid -or
        (-not $protectedExact -and -not $inheritedExact)) {
        throw "$Label owner/DACL mismatch. ExpectedOwner=$ExpectedOwnerSid ActualOwner=$($Snapshot.OwnerSid) ExpectedDacl=$expectedAccess ActualDacl=$($Snapshot.AccessSddl) ActualProtected=$($Snapshot.AccessRulesProtected)"
    }
    if ($protectedExact) { return 'protected_exact' }
    return 'inherited_exact'
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
    $createdIdentity = $null
    try {
        $createdIdentity = Get-StrongPathIdentity -Path $full -Directory $true
        [void](Assert-RealDirectory -Path $full -Label $Label)
        Assert-ExactSecurity -Path $full -Expected $Security `
            -ExpectedOwnerSid $OwnerSid -Label $Label
        return $full
    } catch {
        $failure = $_.Exception
        if ($null -eq $createdIdentity) {
            throw "$Label post-create verification failed and the unbound path was retained: $($failure.Message)"
        }
        try {
            $item = Get-Item -LiteralPath $full -Force -ErrorAction Stop
            if (-not $item.PSIsContainer -or
                ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0 -or
                (Get-StrongPathIdentity -Path $full -Directory $true) -cne
                    $createdIdentity -or
                @(Get-ChildItem -LiteralPath $full -Force -ErrorAction Stop).Count -ne 0) {
                throw 'the created directory identity changed or it is no longer empty'
            }
            Remove-Item -LiteralPath $full -Force -ErrorAction Stop
            if (Test-Path -LiteralPath $full) {
                throw 'the verified empty directory still exists after cleanup'
            }
        } catch {
            throw "$Label post-create verification failed and cleanup could not be proven: $($failure.Message); cleanup=$($_.Exception.Message)"
        }
        throw $failure
    }
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
    try {
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
            if ($SelfTest -and
                [string]$script:InstallerSelfTestAtomicWriteFaultPhase -ceq
                    'after_create') {
                throw 'injected atomic-write failure after staging create'
            }
            $stream.Write($Bytes, 0, $Bytes.Length)
            $stream.Flush($true)
        } finally { $stream.Dispose() }
        [IO.File]::Move($temporary, $Path, [bool]$Replace)
    } catch {
        $failure = $_.Exception
        try {
            if (Test-Path -LiteralPath $temporary) {
                $item = Get-Item -LiteralPath $temporary -Force -ErrorAction Stop
                if ($item.PSIsContainer -or
                    ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
                    throw 'atomic-write staging path is no longer a regular file'
                }
                Remove-Item -LiteralPath $temporary -Force -ErrorAction Stop
            }
        } catch {
            throw "Atomic write failed and staging cleanup could not be proven: $($failure.Message); cleanup=$($_.Exception.Message)"
        }
        throw $failure
    }
}

function Wait-BrokerReceiptReplaceReady {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [int]$TimeoutMilliseconds = 5000
    )
    $deadline = [DateTimeOffset]::UtcNow.AddMilliseconds($TimeoutMilliseconds)
    do {
        try {
            [Rayman.CodexBrokerInstallerNative]::ProbeFileReplace($Path)
            return
        } catch {
            $native = $_.Exception
            while ($null -ne $native -and
                $native -isnot [ComponentModel.Win32Exception]) {
                $native = $native.InnerException
            }
            if ($null -eq $native -or $native.NativeErrorCode -ne 32 -or
                [DateTimeOffset]::UtcNow -ge $deadline) {
                throw
            }
        }
        Start-Sleep -Milliseconds 50
    } while ([DateTimeOffset]::UtcNow -lt $deadline)
    throw "Receipt replace probe did not become ready: $Path"
}

function Test-BrokerReceiptReplaceTransientFailure {
    param([Parameter(Mandatory = $true)]$Exception)
    $current = $Exception
    while ($null -ne $current) {
        if ($current -is [UnauthorizedAccessException]) { return $true }
        if ($current -is [IO.IOException]) {
            $native = $current.HResult -band 0xFFFF
            if ($native -eq 5 -or $native -eq 32) { return $true }
        }
        $current = $current.InnerException
    }
    return $false
}

function Get-BrokerReceiptFileState {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$OldSha256,
        [Parameter(Mandatory = $true)][string]$NewSha256,
        [Parameter(Mandatory = $true)]$Security,
        [Parameter(Mandatory = $true)][string]$OwnerSid
    )
    try {
        $snapshot = Get-BrokerFileSnapshot `
            -Path $Path -Label 'Broker receipt state' -MaximumBytes 64KB
    } catch {
        throw "indeterminate_requires_recovery: formal receipt snapshot failed: $($_.Exception.Message)"
    }
    $expectedAccess = $Security.GetSecurityDescriptorSddlForm(
        [Security.AccessControl.AccessControlSections]::Access
    )
    if (-not $snapshot.AccessRulesProtected -or
        [string]$snapshot.OwnerSid -cne $OwnerSid -or
        [string]$snapshot.AccessSddl -cne $expectedAccess) {
        throw 'indeterminate_requires_recovery: formal receipt held-handle owner/DACL drifted.'
    }
    $hash = [string]$snapshot.Sha256
    $state = if ($hash -ceq $OldSha256) { 'old' }
        elseif ($hash -ceq $NewSha256) { 'new' }
        else { 'unknown' }
    return [pscustomobject]@{
        State = $state
        Sha256 = $hash
        Identity = [string]$snapshot.Identity
        Bytes = $snapshot.Bytes
        OwnerSid = [string]$snapshot.OwnerSid
        AccessSddl = [string]$snapshot.AccessSddl
    }
}

function Publish-BrokerReceiptForward {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][byte[]]$OldBytes,
        [Parameter(Mandatory = $true)][byte[]]$NewBytes,
        [Parameter(Mandatory = $true)]$Security,
        [Parameter(Mandatory = $true)][string]$OwnerSid,
        [int]$ProbeTimeoutMilliseconds = 5000
    )
    $oldHash = Get-BytesSha256 -Bytes $OldBytes
    $newHash = Get-BytesSha256 -Bytes $NewBytes
    $initial = Get-BrokerReceiptFileState -Path $Path `
        -OldSha256 $oldHash -NewSha256 $newHash `
        -Security $Security -OwnerSid $OwnerSid
    if ([string]$initial.State -cne 'old') {
        throw "indeterminate_requires_recovery: forward receipt started in state=$($initial.State)."
    }
    for ($attempt = 1; $attempt -le 3; $attempt++) {
        Wait-BrokerReceiptReplaceReady `
            -Path $Path -TimeoutMilliseconds $ProbeTimeoutMilliseconds
        try {
            if ($SelfTest -and
                [string]$script:InstallerSelfTestReceiptFaultPhase -ceq
                    'forward_old_unchanged') {
                throw [UnauthorizedAccessException]::new(
                    'injected forward failure before receipt replace'
                )
            }
            Write-BytesAtomic -Path $Path -Bytes $NewBytes -Replace `
                -Security $Security
            if ($SelfTest -and
                [string]$script:InstallerSelfTestReceiptFaultPhase -ceq
                    'forward_new_written') {
                throw [UnauthorizedAccessException]::new(
                    'injected forward failure after receipt replace'
                )
            }
            if ($SelfTest -and
                [string]$script:InstallerSelfTestReceiptFaultPhase -ceq
                    'forward_unknown') {
                [IO.File]::WriteAllBytes(
                    $Path, [Text.UTF8Encoding]::new($false).GetBytes('unknown')
                )
                throw [UnauthorizedAccessException]::new(
                    'injected forward failure after unknown receipt bytes'
                )
            }
        } catch {
            $failure = $_.Exception
            $post = Get-BrokerReceiptFileState -Path $Path `
                -OldSha256 $oldHash -NewSha256 $newHash `
                -Security $Security -OwnerSid $OwnerSid
            if ([string]$post.State -ceq 'new') {
                return [pscustomobject]@{
                    State = 'published_new_after_error'; Attempts = $attempt
                    Identity = [string]$post.Identity
                }
            }
            if ([string]$post.State -ceq 'old' -and
                [string]$post.Identity -ceq [string]$initial.Identity) {
                if ($attempt -lt 3 -and
                    (Test-BrokerReceiptReplaceTransientFailure `
                        -Exception $failure)) {
                    Start-Sleep -Milliseconds (100 * $attempt)
                    continue
                }
                throw "Receipt forward replace failed with old bytes unchanged after $attempt attempt(s): $($failure.Message)"
            }
            throw "indeterminate_requires_recovery: forward receipt state=$($post.State) sha256=$($post.Sha256) identity=$($post.Identity) expected_old_identity=$($initial.Identity)"
        }
        $post = Get-BrokerReceiptFileState -Path $Path `
            -OldSha256 $oldHash -NewSha256 $newHash `
            -Security $Security -OwnerSid $OwnerSid
        if ([string]$post.State -cne 'new') {
            throw "indeterminate_requires_recovery: successful forward call ended in state=$($post.State)."
        }
        return [pscustomobject]@{
            State = 'published_new'; Attempts = $attempt
            Identity = [string]$post.Identity
        }
    }
    throw 'Receipt forward replace exhausted its bounded attempts.'
}

function Restore-BrokerReceiptFromJournal {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][byte[]]$OldBytes,
        [Parameter(Mandatory = $true)][byte[]]$NewBytes,
        [Parameter(Mandatory = $true)]$Security,
        [Parameter(Mandatory = $true)][string]$OwnerSid,
        [int]$ProbeTimeoutMilliseconds = 5000
    )
    $oldHash = Get-BytesSha256 -Bytes $OldBytes
    $newHash = Get-BytesSha256 -Bytes $NewBytes
    $initial = Get-BrokerReceiptFileState -Path $Path `
        -OldSha256 $oldHash -NewSha256 $newHash `
        -Security $Security -OwnerSid $OwnerSid
    if ([string]$initial.State -ceq 'old') {
        return [pscustomobject]@{
            State = 'old_already_restored'; Attempts = 0
            Identity = [string]$initial.Identity
        }
    }
    if ([string]$initial.State -cne 'new') {
        throw "indeterminate_requires_recovery: rollback receipt state=$($initial.State) sha256=$($initial.Sha256)."
    }
    for ($attempt = 1; $attempt -le 3; $attempt++) {
        Wait-BrokerReceiptReplaceReady `
            -Path $Path -TimeoutMilliseconds $ProbeTimeoutMilliseconds
        try {
            Write-BytesAtomic -Path $Path -Bytes $OldBytes -Replace `
                -Security $Security
        } catch {
            $failure = $_.Exception
            $post = Get-BrokerReceiptFileState -Path $Path `
                -OldSha256 $oldHash -NewSha256 $newHash `
                -Security $Security -OwnerSid $OwnerSid
            if ([string]$post.State -ceq 'old') {
                return [pscustomobject]@{
                    State = 'old_restored_after_error'; Attempts = $attempt
                    Identity = [string]$post.Identity
                }
            }
            if ([string]$post.State -ceq 'new' -and
                [string]$post.Identity -ceq [string]$initial.Identity -and
                $attempt -lt 3 -and
                (Test-BrokerReceiptReplaceTransientFailure `
                    -Exception $failure)) {
                Start-Sleep -Milliseconds (100 * $attempt)
                continue
            }
            if ([string]$post.State -ceq 'new') {
                throw "known_recovery_pending: receipt remains the journaled new bytes after $attempt attempt(s): $($failure.Message)"
            }
            throw "indeterminate_requires_recovery: rollback receipt state=$($post.State) sha256=$($post.Sha256) identity=$($post.Identity)."
        }
        $post = Get-BrokerReceiptFileState -Path $Path `
            -OldSha256 $oldHash -NewSha256 $newHash `
            -Security $Security -OwnerSid $OwnerSid
        if ([string]$post.State -cne 'old') {
            throw "indeterminate_requires_recovery: successful rollback call ended in state=$($post.State)."
        }
        return [pscustomobject]@{
            State = 'old_restored'; Attempts = $attempt
            Identity = [string]$post.Identity
        }
    }
    throw 'known_recovery_pending: receipt rollback exhausted its bounded attempts.'
}

function Get-BrokerUpgradeReadyFileState {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$OldSha256,
        [Parameter(Mandatory = $true)][string]$NewSha256,
        [Parameter(Mandatory = $true)]$Security,
        [Parameter(Mandatory = $true)][string]$OwnerSid
    )
    if (-not (Test-Path -LiteralPath $Path)) {
        return [pscustomobject]@{
            State = 'absent'
            Sha256 = $null
            Identity = $null
            Bytes = $null
        }
    }
    try {
        $snapshot = Get-BrokerFileSnapshot -Path $Path `
            -Label 'Broker ready state' -MaximumBytes 64KB
    } catch {
        throw "indeterminate_requires_recovery: ready snapshot failed: $($_.Exception.Message)"
    }
    $expectedAccess = $Security.GetSecurityDescriptorSddlForm(
        [Security.AccessControl.AccessControlSections]::Access
    )
    if (-not $snapshot.AccessRulesProtected -or
        [string]$snapshot.OwnerSid -cne $OwnerSid -or
        [string]$snapshot.AccessSddl -cne $expectedAccess) {
        throw 'indeterminate_requires_recovery: ready held-handle owner/DACL drifted.'
    }
    $hash = [string]$snapshot.Sha256
    $state = if ($hash -ceq $OldSha256) { 'old' }
        elseif ($hash -ceq $NewSha256) { 'new' }
        else { 'unknown' }
    return [pscustomobject]@{
        State = $state
        Sha256 = $hash
        Identity = [string]$snapshot.Identity
        Bytes = $snapshot.Bytes
    }
}

function Publish-BrokerReadyForward {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][byte[]]$OldBytes,
        [Parameter(Mandatory = $true)][byte[]]$NewBytes,
        [Parameter(Mandatory = $true)]$Security,
        [Parameter(Mandatory = $true)][string]$OwnerSid,
        [int]$ProbeTimeoutMilliseconds = 5000
    )
    $oldHash = Get-BytesSha256 -Bytes $OldBytes
    $newHash = Get-BytesSha256 -Bytes $NewBytes
    $initial = Get-BrokerUpgradeReadyFileState -Path $Path `
        -OldSha256 $oldHash -NewSha256 $newHash `
        -Security $Security -OwnerSid $OwnerSid
    if ([string]$initial.State -cne 'old') {
        throw "indeterminate_requires_recovery: forward ready started in state=$($initial.State)."
    }
    for ($attempt = 1; $attempt -le 3; $attempt++) {
        Wait-BrokerReceiptReplaceReady -Path $Path `
            -TimeoutMilliseconds $ProbeTimeoutMilliseconds
        try {
            Write-BytesAtomic -Path $Path -Bytes $NewBytes -Replace `
                -Security $Security
        } catch {
            $failure = $_.Exception
            $post = Get-BrokerUpgradeReadyFileState -Path $Path `
                -OldSha256 $oldHash -NewSha256 $newHash `
                -Security $Security -OwnerSid $OwnerSid
            if ([string]$post.State -ceq 'new') {
                return [pscustomobject]@{
                    State = 'published_new_after_error'
                    Attempts = $attempt
                }
            }
            if ([string]$post.State -ceq 'old' -and
                [string]$post.Identity -ceq [string]$initial.Identity -and
                $attempt -lt 3 -and
                (Test-BrokerReceiptReplaceTransientFailure `
                    -Exception $failure)) {
                Start-Sleep -Milliseconds (100 * $attempt)
                continue
            }
            if ([string]$post.State -ceq 'old') {
                throw "Ready forward replace failed with old bytes unchanged after $attempt attempt(s): $($failure.Message)"
            }
            throw "indeterminate_requires_recovery: forward ready state=$($post.State) sha256=$($post.Sha256)."
        }
        $post = Get-BrokerUpgradeReadyFileState -Path $Path `
            -OldSha256 $oldHash -NewSha256 $newHash `
            -Security $Security -OwnerSid $OwnerSid
        if ([string]$post.State -cne 'new') {
            throw "indeterminate_requires_recovery: successful ready forward ended in state=$($post.State)."
        }
        return [pscustomobject]@{
            State = 'published_new'
            Attempts = $attempt
        }
    }
    throw 'Ready forward replace exhausted its bounded attempts.'
}

function Restore-BrokerReadyFromJournal {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][byte[]]$OldBytes,
        [Parameter(Mandatory = $true)][byte[]]$NewBytes,
        [Parameter(Mandatory = $true)]$Security,
        [Parameter(Mandatory = $true)][string]$OwnerSid,
        [int]$ProbeTimeoutMilliseconds = 5000
    )
    $oldHash = Get-BytesSha256 -Bytes $OldBytes
    $newHash = Get-BytesSha256 -Bytes $NewBytes
    $initial = Get-BrokerUpgradeReadyFileState -Path $Path `
        -OldSha256 $oldHash -NewSha256 $newHash `
        -Security $Security -OwnerSid $OwnerSid
    if ([string]$initial.State -ceq 'old') {
        return [pscustomobject]@{
            State = 'old_already_restored'
            Attempts = 0
        }
    }
    if ([string]$initial.State -notin @('new', 'absent')) {
        throw "indeterminate_requires_recovery: rollback ready state=$($initial.State)."
    }
    for ($attempt = 1; $attempt -le 3; $attempt++) {
        try {
            if ([string]$initial.State -ceq 'new') {
                Wait-BrokerReceiptReplaceReady -Path $Path `
                    -TimeoutMilliseconds $ProbeTimeoutMilliseconds
                Write-BytesAtomic -Path $Path -Bytes $OldBytes -Replace `
                    -Security $Security
            } else {
                Write-BytesAtomic -Path $Path -Bytes $OldBytes `
                    -Security $Security
            }
        } catch {
            $failure = $_.Exception
            $post = Get-BrokerUpgradeReadyFileState -Path $Path `
                -OldSha256 $oldHash -NewSha256 $newHash `
                -Security $Security -OwnerSid $OwnerSid
            if ([string]$post.State -ceq 'old') {
                return [pscustomobject]@{
                    State = 'old_restored_after_error'
                    Attempts = $attempt
                }
            }
            if ([string]$post.State -eq [string]$initial.State -and
                $attempt -lt 3 -and
                (Test-BrokerReceiptReplaceTransientFailure `
                    -Exception $failure)) {
                Start-Sleep -Milliseconds (100 * $attempt)
                continue
            }
            if ([string]$post.State -in @('new', 'absent')) {
                throw "known_recovery_pending: ready remains state=$($post.State) after $attempt attempt(s): $($failure.Message)"
            }
            throw "indeterminate_requires_recovery: rollback ready state=$($post.State) sha256=$($post.Sha256)."
        }
        $post = Get-BrokerUpgradeReadyFileState -Path $Path `
            -OldSha256 $oldHash -NewSha256 $newHash `
            -Security $Security -OwnerSid $OwnerSid
        if ([string]$post.State -cne 'old') {
            throw "indeterminate_requires_recovery: successful ready rollback ended in state=$($post.State)."
        }
        return [pscustomobject]@{
            State = 'old_restored'
            Attempts = $attempt
        }
    }
    throw 'known_recovery_pending: ready rollback exhausted its bounded attempts.'
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

function ConvertTo-PowerShellSingleQuotedLiteral {
    param([Parameter(Mandatory = $true)][string]$Value)
    return "'" + $Value.Replace("'", "''") + "'"
}

function Get-BrokerGitBlobOid {
    param([Parameter(Mandatory = $true)][byte[]]$Bytes)
    $hash = [Security.Cryptography.IncrementalHash]::CreateHash(
        [Security.Cryptography.HashAlgorithmName]::SHA1
    )
    try {
        $header = [Text.Encoding]::ASCII.GetBytes("blob $($Bytes.Length)`0")
        $hash.AppendData($header)
        $hash.AppendData($Bytes)
        return [Convert]::ToHexString(
            $hash.GetHashAndReset()
        ).ToLowerInvariant()
    } finally { $hash.Dispose() }
}

function Invoke-BrokerCommittedSourceGitQuery {
    param(
        [Parameter(Mandatory = $true)][string]$GitPath,
        [Parameter(Mandatory = $true)][string]$RepositoryRoot,
        [Parameter(Mandatory = $true)][string[]]$Arguments,
        [Parameter(Mandatory = $true)][string]$Label
    )
    $baseArguments = @(
        '--no-optional-locks', '--no-replace-objects',
        '-c', 'core.fsmonitor=false',
        '-c', 'core.hooksPath=NUL',
        '-c', 'core.excludesFile=NUL',
        '-c', 'core.quotepath=false',
        '-c', "safe.directory=$RepositoryRoot",
        '-C', $RepositoryRoot
    )
    $output = @(& $GitPath @baseArguments @Arguments 2>&1)
    $exitCode = $LASTEXITCODE
    if ($exitCode -ne 0) {
        throw "$Label Git query failed: exit=$exitCode output=$($output -join ' | ')"
    }
    return @($output | ForEach-Object { [string]$_ })
}

function Get-BrokerCommittedSourceBinding {
    param(
        [Parameter(Mandatory = $true)][string]$GitPath,
        [Parameter(Mandatory = $true)][string]$RepositoryRoot,
        [Parameter(Mandatory = $true)][string]$ExpectedHead,
        [Parameter(Mandatory = $true)][string]$RelativePath,
        [Parameter(Mandatory = $true)][byte[]]$Bytes
    )
    if ($ExpectedHead -notmatch '^[0-9a-f]{40}$' -or
        [string]::IsNullOrWhiteSpace($RelativePath) -or
        $RelativePath.Contains('\\') -or $RelativePath.StartsWith('/') -or
        $RelativePath.Contains([char]0)) {
        throw 'Committed-source binding input is invalid.'
    }
    $escapedPath = [regex]::Escape($RelativePath)
    $index = @(Invoke-BrokerCommittedSourceGitQuery `
        -GitPath $GitPath -RepositoryRoot $RepositoryRoot `
        -Arguments @('ls-files', '--stage', '--', $RelativePath) `
        -Label 'Committed-source index')
    $indexMatch = if ($index.Count -eq 1) {
        [regex]::Match(
            $index[0], "^(100644|100755) ([0-9a-f]{40}) 0`t$escapedPath$"
        )
    } else { $null }
    if ($null -eq $indexMatch -or -not $indexMatch.Success) {
        throw "Committed source must have exactly one normal stage-0 index entry: $RelativePath"
    }
    $indexMode = [string]$indexMatch.Groups[1].Value
    $indexOid = [string]$indexMatch.Groups[2].Value
    $tag = @(Invoke-BrokerCommittedSourceGitQuery `
        -GitPath $GitPath -RepositoryRoot $RepositoryRoot `
        -Arguments @('ls-files', '-v', '--', $RelativePath) `
        -Label 'Committed-source index flags')
    if ($tag.Count -ne 1 -or $tag[0] -cne "H $RelativePath") {
        throw "Committed source rejects assume-unchanged, skip-worktree, sparse, removed, or unmerged index state: $RelativePath"
    }
    $tree = @(Invoke-BrokerCommittedSourceGitQuery `
        -GitPath $GitPath -RepositoryRoot $RepositoryRoot `
        -Arguments @('ls-tree', $ExpectedHead, '--', $RelativePath) `
        -Label 'Committed-source HEAD tree')
    $treeMatch = if ($tree.Count -eq 1) {
        [regex]::Match(
            $tree[0], "^(100644|100755) blob ([0-9a-f]{40})`t$escapedPath$"
        )
    } else { $null }
    if ($null -eq $treeMatch -or -not $treeMatch.Success) {
        throw "Committed source is not one ordinary HEAD blob: $RelativePath"
    }
    $headMode = [string]$treeMatch.Groups[1].Value
    $headOid = [string]$treeMatch.Groups[2].Value
    $worktreeOid = Get-BrokerGitBlobOid -Bytes $Bytes
    if ($indexMode -cne $headMode -or $indexOid -cne $headOid -or
        $worktreeOid -cne $headOid) {
        throw "Committed source bytes, index, and HEAD blob disagree: $RelativePath"
    }
    return [pscustomobject]@{
        Mode = $headMode
        BlobOid = $headOid
        IndexTag = 'H'
    }
}

function Get-BrokerGoalFingerprintFromBaseline {
    param(
        [Parameter(Mandatory = $true)]$BaselineFiles,
        [Parameter(Mandatory = $true)][AllowEmptyCollection()][array]$SourceBindings
    )
    $files = [Collections.Generic.Dictionary[string,string]]::new(
        [StringComparer]::Ordinal
    )
    foreach ($property in $BaselineFiles.PSObject.Properties) {
        if ([string]$property.Name -notmatch '^[^\x00]+$' -or
            [string]$property.Value -notmatch '^[0-9a-f]{64}$' -or
            -not $files.TryAdd(
                [string]$property.Name, [string]$property.Value
            )) {
            throw 'Upgrade launcher Goal baseline file map is invalid.'
        }
    }
    foreach ($binding in $SourceBindings) {
        $relative = [string]$binding.relative_path
        if (-not $files.ContainsKey($relative) -or
            [string]$binding.sha256 -notmatch '^[0-9a-f]{64}$') {
            throw "Upgrade launcher source is absent from the Goal baseline: $relative"
        }
        $files[$relative] = [string]$binding.sha256
    }
    $keys = [string[]]@($files.Keys)
    [Array]::Sort($keys, [StringComparer]::Ordinal)
    $utf8 = [Text.UTF8Encoding]::new($false, $true)
    $hasher = [Security.Cryptography.IncrementalHash]::CreateHash(
        [Security.Cryptography.HashAlgorithmName]::SHA256
    )
    try {
        foreach ($key in $keys) {
            $hasher.AppendData($utf8.GetBytes($key))
            $hasher.AppendData([byte[]](0))
            $hasher.AppendData($utf8.GetBytes($files[$key]))
            $hasher.AppendData([byte[]](0))
        }
        return [Convert]::ToHexString(
            $hasher.GetHashAndReset()
        ).ToLowerInvariant()
    } finally { $hasher.Dispose() }
}

function Assert-BrokerUpgradeGoalAuthority {
    param(
        [Parameter(Mandatory = $true)]$Goal,
        [Parameter(Mandatory = $true)][string]$GoalId,
        [Parameter(Mandatory = $true)][string]$SourceFingerprint,
        [Parameter(Mandatory = $true)]
        [ValidateSet('install', 'upgrade')][string]$Action
    )
    $authority = @($Goal.authority_receipts)[-1]
    $review = @($Goal.review_receipts)[-1]
    $runs = @($authority.runs)
    if ([string]$Goal.id -cne $GoalId -or
        [string]$Goal.status -cne 'active' -or
        [string]$Goal.lifecycle -cne 'current' -or
        $null -eq $authority -or [int]$authority.repeat -ne 2 -or
        [string]$authority.workspace_fingerprint -cne $SourceFingerprint -or
        $runs.Count -ne 2 -or
        @($runs | Where-Object {
            [int]$_.exit_code -ne 0 -or
            [string]$_.workspace_fingerprint_before -cne $SourceFingerprint -or
            [string]$_.workspace_fingerprint_after -cne $SourceFingerprint
        }).Count -ne 0 -or
        $null -eq $review -or
        [string]$review.source_fingerprint -cne $SourceFingerprint) {
        throw "$Action authority requires a current Goal with same-fingerprint review and repeat-2 authority."
    }
    $requirements = [Collections.Generic.Dictionary[string,string]]::new(
        [StringComparer]::Ordinal
    )
    foreach ($requirement in @($Goal.requirements)) {
        if (-not $requirements.TryAdd(
                [string]$requirement.id, [string]$requirement.status
            )) {
            throw 'Upgrade authority Goal contains duplicate requirement IDs.'
        }
    }
    if ($requirements.Count -ne 5 -or
        $requirements['req_1'] -cne 'done' -or
        $requirements['req_2'] -cne 'done' -or
        $requirements['req_3'] -cne 'done' -or
        $requirements['req_4'] -notin @('open', 'done') -or
        $requirements['req_5'] -cne 'open') {
        throw "$Action authority requires req1/req2/req3 done, req4 open or done, and req5 open."
    }
}

function Assert-BrokerUpgradePendingBoundary {
    param(
        [Parameter(Mandatory = $true)]$Pending,
        [Parameter(Mandatory = $true)][string]$GoalId,
        [Parameter(Mandatory = $true)][string]$CapabilityKey,
        [Parameter(Mandatory = $true)][string]$BoundaryClass,
        [string]$ExpectedPendingId,
        [string]$ExpectedPackageSha256
    )
    Assert-ExactProperties -Document $Pending `
        -Expected @('items') -Label 'Upgrade authority pending state'
    if ($Pending.items -isnot [array]) {
        throw 'Upgrade authority pending items must be an array.'
    }
    $matches = @($Pending.items | Where-Object {
        [string]$_.goal_id -ceq $GoalId
    })
    if ($matches.Count -ne 1) {
        throw 'Upgrade authority requires exactly one pending item for the Goal.'
    }
    $item = $matches[0]
    if ([int]$item.contract_version -ne 2 -or
        [string]$item.owner -cne 'human' -or
        [string]$item.consultation_timing -cne 'immediate' -or
        [string]$item.capability_key -cne $CapabilityKey -or
        [string]$item.boundary_class -cne $BoundaryClass -or
        [string]$item.package_sha256 -notmatch '^[0-9a-f]{64}$' -or
        ($ExpectedPendingId -and
            [string]$item.id -cne $ExpectedPendingId) -or
        ($ExpectedPackageSha256 -and
            [string]$item.package_sha256 -cne $ExpectedPackageSha256) -or
        $null -ne $item.background_mechanism -or
        $null -ne $item.background_authority_evidence -or
        $null -ne $item.background_isolation_evidence) {
        throw 'Upgrade authority pending capability identity drifted.'
    }
    return $item
}

function Assert-BrokerUpgradeFrontierReport {
    param(
        [Parameter(Mandatory = $true)]$Frontier,
        [Parameter(Mandatory = $true)][string]$GoalId,
        [Parameter(Mandatory = $true)][string]$ExpectedCapabilityKey,
        [Parameter(Mandatory = $true)][string]$ExpectedBoundaryClass
    )
    $blockers = @($Frontier.blockers)
    if ([string]$Frontier.goal_id -cne $GoalId -or
        [string]$Frontier.decision -cne 'ask_user' -or
        [bool]$Frontier.ask_user_allowed -ne $true -or
        [string]$Frontier.execution -cne 'paused_for_user' -or
        [string]$Frontier.consultation -cne 'ready' -or
        [bool]$Frontier.background_execution_allowed -ne $false -or
        $blockers.Count -ne 1 -or
        [int]$blockers[0].contract_version -ne 2 -or
        [string]$blockers[0].goal_id -cne $GoalId -or
        [string]$blockers[0].owner -cne 'human' -or
        [string]$blockers[0].consultation_timing -cne 'immediate' -or
        [string]$blockers[0].capability_key -cne $ExpectedCapabilityKey -or
        [string]$blockers[0].boundary_class -cne $ExpectedBoundaryClass -or
        [string]$blockers[0].package_sha256 -notmatch '^[0-9a-f]{64}$') {
        throw 'Upgrade Goal is not at the exact current human UAC boundary.'
    }
    return $blockers[0]
}

function Invoke-BrokerBoundGoalCli {
    param(
        [Parameter(Mandatory = $true)][string]$RaymanPath,
        [Parameter(Mandatory = $true)][string]$GoalId,
        [Parameter(Mandatory = $true)][string]$ExpectedCapabilityKey,
        [Parameter(Mandatory = $true)][string]$ExpectedBoundaryClass
    )
    $saved = [ordered]@{}
    foreach ($entry in @(Get-ChildItem Env: -ErrorAction Stop | Where-Object {
        $_.Name.StartsWith('RAYMAN_', [StringComparison]::OrdinalIgnoreCase)
    })) {
        $saved[[string]$entry.Name] = [string]$entry.Value
        Remove-Item -LiteralPath ('Env:' + [string]$entry.Name) -ErrorAction Stop
    }
    try {
        $showOutput = @(& $RaymanPath --format json goal show $GoalId)
        if ($LASTEXITCODE -ne 0 -or $showOutput.Count -eq 0) {
            throw 'Bound Rayman CLI rejected the upgrade Goal.'
        }
        $show = ($showOutput -join [char]10) | ConvertFrom-Json -Depth 100 -NoEnumerate -DateKind String
        $frontierOutput = @(& $RaymanPath --format json goal frontier $GoalId)
        if ($LASTEXITCODE -ne 0 -or $frontierOutput.Count -eq 0) {
            throw 'Bound Rayman CLI could not compute the upgrade Goal frontier.'
        }
        $frontier = ($frontierOutput -join [char]10) | ConvertFrom-Json -Depth 20 -NoEnumerate -DateKind String
        [void](Assert-BrokerUpgradeFrontierReport `
            -Frontier $frontier -GoalId $GoalId `
            -ExpectedCapabilityKey $ExpectedCapabilityKey `
            -ExpectedBoundaryClass $ExpectedBoundaryClass)
        return [pscustomobject]@{ Goal = $show; Frontier = $frontier }
    } finally {
        foreach ($name in @($saved.Keys)) {
            [Environment]::SetEnvironmentVariable(
                $name, [string]$saved[$name], [EnvironmentVariableTarget]::Process
            )
        }
    }
}

function Clear-BrokerGitEnvironment {
    foreach ($entry in @(Get-ChildItem Env: -ErrorAction Stop | Where-Object {
        $_.Name.StartsWith('GIT_', [StringComparison]::OrdinalIgnoreCase) -or
        $_.Name -in @('SSH_ASKPASS', 'GCM_INTERACTIVE')
    })) {
        Remove-Item -LiteralPath ('Env:' + [string]$entry.Name) -ErrorAction Stop
    }
    $env:GIT_CONFIG_NOSYSTEM = '1'
    $env:GIT_CONFIG_GLOBAL = 'NUL'
    $env:GIT_CONFIG_SYSTEM = 'NUL'
    $env:GIT_OPTIONAL_LOCKS = '0'
    $env:GIT_TERMINAL_PROMPT = '0'
    $env:GCM_INTERACTIVE = 'never'
}

function New-BrokerUpgradeUserCommandText {
    param(
        [Parameter(Mandatory = $true)]
        [ValidateSet('install', 'upgrade')][string]$Action,
        [Parameter(Mandatory = $true)][string]$PowerShellPath,
        [Parameter(Mandatory = $true)][string]$PowerShellSha256,
        [Parameter(Mandatory = $true)][string]$InstallerPath,
        [Parameter(Mandatory = $true)][string]$InstallerSha256,
        [Parameter(Mandatory = $true)][string]$AuthorityManifestPath,
        [Parameter(Mandatory = $true)][string]$AuthorityManifestSha256,
        [Parameter(Mandatory = $true)][string]$GoalId,
        [Parameter(Mandatory = $true)][string]$SourceFingerprint
    )
    $template = @'
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if ($PSVersionTable.PSVersion.Major -lt 7) {
    throw 'Open PowerShell 7 as administrator before running this command.'
}
$noProfile = $false
foreach ($argument in [Environment]::GetCommandLineArgs()) {
    if ([string]$argument -ieq '-NoProfile') {
        $noProfile = $true
        break
    }
}
if (-not $noProfile) {
    throw 'Trusted broker mutation requires PowerShell 7 launched with -NoProfile.'
}
$action = __ACTION__
$powerShellPath = __POWERSHELL_PATH__
$powerShellSha256 = __POWERSHELL_SHA256__
$installerPath = __INSTALLER_PATH__
$installerSha256 = __INSTALLER_SHA256__
$manifestPath = __MANIFEST_PATH__
$manifestSha256 = __MANIFEST_SHA256__
$goalId = __GOAL_ID__
$sourceFingerprint = __SOURCE_FINGERPRINT__
$openLexicalHandle = [Microsoft.Win32.SafeHandles.SafeFileHandle].GetMethod(
    'Open', [Reflection.BindingFlags]'NonPublic, Static', $null,
    [type[]]@(
        [string], [IO.FileMode], [IO.FileAccess], [IO.FileShare],
        [IO.FileOptions], [long], [Nullable[IO.UnixFileMode]]
    ), $null
)
if ($null -eq $openLexicalHandle) {
    throw 'Bound PowerShell 7 runtime lacks the lexical no-reparse open API.'
}
$held = [Collections.Generic.List[IO.FileStream]]::new()
$heldDirectories = [Collections.Generic.List[object]]::new()
$heldDirectoryPaths = [Collections.Generic.HashSet[string]]::new(
    [StringComparer]::OrdinalIgnoreCase
)
function Open-LexicalHandle(
    [string]$Path,
    [IO.FileAccess]$Access,
    [IO.FileShare]$Share,
    [IO.FileOptions]$Options
) {
    try {
        return $openLexicalHandle.Invoke(
            $null,
            [object[]]@(
                $Path, [IO.FileMode]::Open, $Access, $Share, $Options,
                [long]0, $null
            )
        )
    } catch [Reflection.TargetInvocationException] {
        if ($null -ne $_.Exception.InnerException) {
            throw $_.Exception.InnerException
        }
        throw
    }
}
function Open-Namespace([string]$FilePath) {
    $directories = [Collections.Generic.List[string]]::new()
    $current = [IO.Path]::GetDirectoryName([IO.Path]::GetFullPath($FilePath))
    while (-not [string]::IsNullOrWhiteSpace($current)) {
        $directories.Add($current)
        $parent = [IO.Directory]::GetParent($current)
        if ($null -eq $parent) { break }
        $current = $parent.FullName
    }
    for ($index = $directories.Count - 1; $index -ge 0; $index--) {
        $directory = [IO.Path]::GetFullPath($directories[$index])
        if (-not $heldDirectoryPaths.Add($directory)) { continue }
        $handle = $null
        try {
            $handle = Open-LexicalHandle `
                -Path $directory -Access ([IO.FileAccess]::Read) `
                -Share ([IO.FileShare]::ReadWrite) -Options (
                [IO.FileOptions](0x02000000 -bor 0x00200000)
            )
            $attributes = [IO.File]::GetAttributes($handle)
            if (($attributes -band [IO.FileAttributes]::Directory) -eq 0 -or
                ($attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
                throw ('Pinned launch ancestor is not an ordinary directory: ' +
                    $directory)
            }
            $heldDirectories.Add($handle)
            $handle = $null
        } finally {
            if ($null -ne $handle) { $handle.Dispose() }
        }
    }
}
function Open-Pinned([string]$Path, [string]$ExpectedHash, [long]$MaximumBytes) {
    $fullPath = [IO.Path]::GetFullPath($Path)
    Open-Namespace $fullPath
    $handle = $null
    $stream = $null
    try {
        $handle = Open-LexicalHandle `
            -Path $fullPath -Access ([IO.FileAccess]::Read) `
            -Share ([IO.FileShare]::Read) `
            -Options ([IO.FileOptions]0x00200000)
        $attributes = [IO.File]::GetAttributes($handle)
        if (($attributes -band [IO.FileAttributes]::Directory) -ne 0 -or
            ($attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
            throw ('Pinned launch input is not an ordinary file: ' + $fullPath)
        }
        $length = [IO.RandomAccess]::GetLength($handle)
        if ($length -le 0 -or $length -gt $MaximumBytes) {
            throw ('Pinned launch input is not a bounded file: ' + $fullPath)
        }
        $stream = [IO.FileStream]::new($handle, [IO.FileAccess]::Read)
        $handle = $null
        $sha = [Security.Cryptography.SHA256]::Create()
        try {
            $actual = [BitConverter]::ToString(
                $sha.ComputeHash($stream)
            ).Replace('-', '').ToLowerInvariant()
        } finally { $sha.Dispose() }
        $stream.Position = 0
        if ($actual -cne $ExpectedHash) {
            throw ('Pinned launch input hash drift: ' + $Path)
        }
        $held.Add($stream)
        return $stream
    } catch {
        if ($null -ne $stream) { $stream.Dispose() }
        throw
    } finally {
        if ($null -ne $handle) { $handle.Dispose() }
    }
}
try {
    $currentRuntime = [Diagnostics.Process]::GetCurrentProcess().MainModule.FileName
    if (-not $currentRuntime.Equals(
            $powerShellPath, [StringComparison]::OrdinalIgnoreCase
        )) {
        throw ('PowerShell 7 path drift: ' + $currentRuntime)
    }
    [void](Open-Pinned $powerShellPath $powerShellSha256 67108864)
    [void](Open-Pinned $installerPath $installerSha256 2097152)
    [void](Open-Pinned $manifestPath $manifestSha256 2097152)
    if ($action -ceq 'upgrade') {
        $mutationArguments = @{
            Upgrade = $true
            Yes = $true
            ExpectedGoalId = $goalId
            ExpectedSourceFingerprint = $sourceFingerprint
            UpgradeAuthorityManifestPath = $manifestPath
            UpgradeAuthorityManifestSha256 = $manifestSha256
        }
    } elseif ($action -ceq 'install') {
        $mutationArguments = @{
            Install = $true
            Yes = $true
            ExpectedGoalId = $goalId
            ExpectedSourceFingerprint = $sourceFingerprint
            InstallAuthorityManifestPath = $manifestPath
            InstallAuthorityManifestSha256 = $manifestSha256
        }
    } else {
        throw 'Trusted broker mutation action is invalid.'
    }
    & $installerPath @mutationArguments
} finally {
    for ($index = $held.Count - 1; $index -ge 0; $index--) {
        $held[$index].Dispose()
    }
    for ($index = $heldDirectories.Count - 1; $index -ge 0; $index--) {
        $heldDirectories[$index].Dispose()
    }
}
'@
    $replacements = [ordered]@{
        '__ACTION__' = ConvertTo-PowerShellSingleQuotedLiteral $Action
        '__POWERSHELL_PATH__' = ConvertTo-PowerShellSingleQuotedLiteral $PowerShellPath
        '__POWERSHELL_SHA256__' = ConvertTo-PowerShellSingleQuotedLiteral $PowerShellSha256
        '__INSTALLER_PATH__' = ConvertTo-PowerShellSingleQuotedLiteral $InstallerPath
        '__INSTALLER_SHA256__' = ConvertTo-PowerShellSingleQuotedLiteral $InstallerSha256
        '__MANIFEST_PATH__' = ConvertTo-PowerShellSingleQuotedLiteral $AuthorityManifestPath
        '__MANIFEST_SHA256__' = ConvertTo-PowerShellSingleQuotedLiteral $AuthorityManifestSha256
        '__GOAL_ID__' = ConvertTo-PowerShellSingleQuotedLiteral $GoalId
        '__SOURCE_FINGERPRINT__' = ConvertTo-PowerShellSingleQuotedLiteral $SourceFingerprint
    }
    foreach ($entry in $replacements.GetEnumerator()) {
        $template = $template.Replace([string]$entry.Key, [string]$entry.Value)
    }
    return (($template -replace '\r?\n', [Environment]::NewLine).TrimEnd() +
        [Environment]::NewLine)
}

function New-BrokerUpgradeAuthorityManifest {
    param(
        [Parameter(Mandatory = $true)]
        [ValidateSet('install', 'upgrade')][string]$Action,
        [Parameter(Mandatory = $true)][string]$GoalId,
        [Parameter(Mandatory = $true)][string]$SourceFingerprint
    )
    $account = 'QIN5521\qinrm'
    $group = 'QIN5521\CodexSandboxUsers'
    $pendingCapabilityKey = if ($Action -ceq 'install') {
        $script:InstallPendingCapabilityKey
    } else { $script:UpgradePendingCapabilityKey }
    $goalPath = Assert-ChildPath -Child (
        Join-Path $script:RepositoryRoot ('.RaymanCodingSkill\goals\' + $GoalId + '.json')
    ) -Parent $script:RepositoryRoot -Label 'Upgrade authority Goal'
    $goalSnapshot = Get-BrokerFileSnapshot -Path $goalPath -Label 'Upgrade authority Goal' -MaximumBytes 1MB
    $goal = ConvertFrom-BrokerLauncherGoalBytes -Bytes $goalSnapshot.Bytes
    Assert-BrokerUpgradeGoalAuthority -Goal $goal -GoalId $GoalId `
        -SourceFingerprint $SourceFingerprint -Action $Action
    $pendingPath = Assert-ChildPath -Child (
        Join-Path $script:RepositoryRoot '.RaymanCodingSkill\pending.json'
    ) -Parent $script:RepositoryRoot -Label 'Upgrade authority pending state'
    $pendingSnapshot = Get-BrokerFileSnapshot `
        -Path $pendingPath -Label 'Upgrade authority pending state' `
        -MaximumBytes 1MB
    $pending = ConvertFrom-BrokerPendingBytes -Bytes $pendingSnapshot.Bytes

    $runtime = Get-CurrentPowerShellRuntime
    $fixedPowerShell = 'C:\Program Files\PowerShell\7\pwsh.exe'
    if (-not $runtime.Path.Equals(
            $fixedPowerShell, [StringComparison]::OrdinalIgnoreCase
        )) {
        throw "Upgrade authority requires the fixed PowerShell 7 path: $fixedPowerShell"
    }
    $git = Get-RegisteredGitExecutable
    Clear-BrokerGitEnvironment
    $raymanPath = Assert-ChildPath -Child (
        Join-Path $script:RepositoryRoot 'target\debug\rayman.exe'
    ) -Parent $script:RepositoryRoot -Label 'Upgrade authority Rayman CLI'
    $goalCli = Invoke-BrokerBoundGoalCli -RaymanPath $raymanPath `
        -GoalId $GoalId `
        -ExpectedCapabilityKey $pendingCapabilityKey `
        -ExpectedBoundaryClass $script:UpgradePendingBoundaryClass
    Assert-BrokerUpgradeGoalAuthority -Goal $goalCli.Goal -GoalId $GoalId `
        -SourceFingerprint $SourceFingerprint -Action $Action
    $frontierBlocker = @($goalCli.Frontier.blockers)[0]
    $pendingItem = Assert-BrokerUpgradePendingBoundary `
        -Pending $pending -GoalId $GoalId `
        -CapabilityKey $pendingCapabilityKey `
        -BoundaryClass $script:UpgradePendingBoundaryClass `
        -ExpectedPendingId ([string]$frontierBlocker.id) `
        -ExpectedPackageSha256 ([string]$frontierBlocker.package_sha256)

    $headOutput = @(& $git.Path --no-optional-locks -c core.fsmonitor=false -c core.hooksPath=NUL -c core.excludesFile=NUL -c "safe.directory=$script:RepositoryRoot" -C $script:RepositoryRoot rev-parse HEAD)
    if ($LASTEXITCODE -ne 0 -or $headOutput.Count -ne 1 -or
        [string]$headOutput[0].Trim() -notmatch '^[0-9a-f]{40}$') {
        throw 'Upgrade authority could not bind the exact Git HEAD.'
    }
    $head = [string]$headOutput[0].Trim()
    $statusOutput = @(& $git.Path --no-optional-locks -c core.fsmonitor=false -c core.hooksPath=NUL -c core.excludesFile=NUL -c "safe.directory=$script:RepositoryRoot" -C $script:RepositoryRoot status --porcelain=v1 --untracked-files=all)
    $expectedStatus = @()
    if ($LASTEXITCODE -ne 0 -or $statusOutput.Count -ne 0) {
        throw "$Action authority requires a clean independently committed source: $($statusOutput -join ' | ')"
    }

    $sourceBindings = @($script:UpgradeSourcePaths | ForEach-Object {
        $absolute = Assert-ChildPath -Child (
            Join-Path $script:RepositoryRoot $_
        ) -Parent $script:RepositoryRoot -Label 'Upgrade authority source'
        $snapshot = Get-BrokerFileSnapshot -Path $absolute -Label ('Upgrade authority source ' + $_) -MaximumBytes 4MB
        $committed = Get-BrokerCommittedSourceBinding `
            -GitPath $git.Path -RepositoryRoot $script:RepositoryRoot `
            -ExpectedHead $head -RelativePath $_ -Bytes $snapshot.Bytes
        [ordered]@{
            relative_path = $_
            path = $snapshot.Path
            sha256 = $snapshot.Sha256
            git_mode = [string]$committed.Mode
            git_blob_oid = [string]$committed.BlobOid
            identity = $snapshot.Identity
            owner_sid = $snapshot.OwnerSid
            access_sddl = $snapshot.AccessSddl
            access_rules_protected = $snapshot.AccessRulesProtected
        }
    })
    $baselineFingerprint = Get-BrokerGoalFingerprintFromBaseline -BaselineFiles $goal.baseline.files -SourceBindings @()
    if ($baselineFingerprint -cne [string]$goal.baseline.workspace_fingerprint) {
        throw 'Upgrade authority Goal baseline fingerprint is internally inconsistent.'
    }
    $currentFingerprint = Get-BrokerGoalFingerprintFromBaseline -BaselineFiles $goal.baseline.files -SourceBindings $sourceBindings
    if ($currentFingerprint -cne $SourceFingerprint) {
        throw "Upgrade authority source fingerprint is stale: actual=$currentFingerprint expected=$SourceFingerprint"
    }

    $runtimeSnapshot = Get-BrokerFileSnapshot -Path $runtime.Path -Label 'Upgrade authority PowerShell 7' -MaximumBytes 64MB
    $gitSnapshot = Get-BrokerFileSnapshot -Path $git.Path -Label 'Upgrade authority Git' -MaximumBytes 64MB
    $gitInputs = @($script:UpgradeGitInputCandidates | Where-Object {
        Test-Path -LiteralPath (Join-Path $script:RepositoryRoot $_) -PathType Leaf
    } | ForEach-Object {
        $absolute = Assert-ChildPath -Child (
            Join-Path $script:RepositoryRoot $_
        ) -Parent $script:RepositoryRoot -Label 'Upgrade authority Git input'
        $snapshot = Get-BrokerFileSnapshot `
            -Path $absolute -Label ('Upgrade authority Git input ' + $_) `
            -MaximumBytes 16MB
        [ordered]@{
            relative_path = $_
            path = $snapshot.Path
            sha256 = $snapshot.Sha256
            identity = $snapshot.Identity
            owner_sid = $snapshot.OwnerSid
            access_sddl = $snapshot.AccessSddl
            access_rules_protected = $snapshot.AccessRulesProtected
        }
    })
    $installerSnapshot = @($sourceBindings | Where-Object {
        [string]$_.relative_path -ceq 'scripts/install-codex-powershell-broker.ps1'
    })[0]
    $workerSnapshot = @($sourceBindings | Where-Object {
        [string]$_.relative_path -ceq 'scripts/codex-powershell-broker.ps1'
    })[0]
    $nonce = [Guid]::NewGuid().ToString('N')
    $createdAt = [DateTimeOffset]::UtcNow
    $expiresAt = $createdAt.AddMinutes(30)
    return [ordered]@{
        schema_version = 1
        action = $Action
        nonce = $nonce
        created_at_utc = $createdAt.ToString('o')
        expires_at_utc = $expiresAt.ToString('o')
        repository_root = $script:RepositoryRoot
        install_root = 'C:\ProgramData\Rayman\CodexPowerShellBroker'
        request_root = 'C:\ProgramData\Rayman\CodexPowerShellBroker\requests'
        task_name = '\Rayman-CodexPowerShellBroker'
        user_account = $account
        sandbox_group = $group
        installation_guard_name = Get-BrokerInstallationMutexName -Root 'C:\ProgramData\Rayman\CodexPowerShellBroker'
        goal_id = $GoalId
        goal_path = $goalSnapshot.Path
        goal_sha256 = $goalSnapshot.Sha256
        goal_identity = $goalSnapshot.Identity
        goal_owner_sid = $goalSnapshot.OwnerSid
        goal_access_sddl = $goalSnapshot.AccessSddl
        goal_access_rules_protected = $goalSnapshot.AccessRulesProtected
        pending_path = $pendingSnapshot.Path
        pending_sha256 = $pendingSnapshot.Sha256
        pending_identity = $pendingSnapshot.Identity
        pending_owner_sid = $pendingSnapshot.OwnerSid
        pending_access_sddl = $pendingSnapshot.AccessSddl
        pending_access_rules_protected = $pendingSnapshot.AccessRulesProtected
        pending_id = [string]$pendingItem.id
        pending_package_sha256 = [string]$pendingItem.package_sha256
        pending_capability_key = [string]$pendingItem.capability_key
        pending_boundary_class = [string]$pendingItem.boundary_class
        source_fingerprint = $SourceFingerprint
        expected_head = $head
        expected_status = $expectedStatus
        source_bindings = $sourceBindings
        git_inputs = $gitInputs
        powershell = [ordered]@{
            path = $runtimeSnapshot.Path
            sha256 = $runtimeSnapshot.Sha256
            identity = $runtimeSnapshot.Identity
            owner_sid = $runtimeSnapshot.OwnerSid
            access_sddl = $runtimeSnapshot.AccessSddl
            access_rules_protected = $runtimeSnapshot.AccessRulesProtected
        }
        git = [ordered]@{
            path = $gitSnapshot.Path
            sha256 = $gitSnapshot.Sha256
            identity = $gitSnapshot.Identity
            owner_sid = $gitSnapshot.OwnerSid
            access_sddl = $gitSnapshot.AccessSddl
            access_rules_protected = $gitSnapshot.AccessRulesProtected
        }
        installer_sha256 = [string]$installerSnapshot.sha256
        worker_sha256 = [string]$workerSnapshot.sha256
    }
}

function Assert-BrokerUpgradeTransactionLockReady {
    param(
        [Parameter(Mandatory = $true)][string]$Root,
        [ValidateRange(0, 10000)]
        [int]$RetryTimeoutMilliseconds = 3000
    )

    if (-not (Test-Path -LiteralPath $Root)) { return 'not_present' }
    $rootPath = Assert-RealDirectory -Path $Root `
        -Label 'Upgrade transaction preflight install root'
    $transactionsPath = Assert-ChildPath -Child (
        Join-Path $rootPath $script:GitTransactionDirectoryName
    ) -Parent $rootPath -Label 'Upgrade transaction preflight root'
    if (-not (Test-Path -LiteralPath $transactionsPath -PathType Container)) {
        return 'not_present'
    }
    $transactionsPath = Assert-RealDirectory -Path $transactionsPath `
        -Label 'Upgrade transaction preflight root'
    $lockPath = Assert-ChildPath -Child (
        Join-Path $transactionsPath 'transaction.lock'
    ) -Parent $transactionsPath -Label 'Upgrade transaction preflight lock'
    if (-not (Test-Path -LiteralPath $lockPath -PathType Leaf)) {
        return 'not_present'
    }
    $item = Get-Item -LiteralPath $lockPath -Force -ErrorAction Stop
    if ($item.PSIsContainer -or
        ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0 -or
        $item.Length -ne 0) {
        throw 'Upgrade transaction preflight lock is not an ordinary zero-byte file.'
    }
    $guards = Open-BrokerDirectoryGuardChain `
        -FilePath $lockPath -Label 'Upgrade transaction preflight lock'
    $handle = $null
    try {
        $handle = Open-BrokerPinnedFileHandle `
            -Path $lockPath -Label 'Upgrade transaction preflight lock' `
            -RetryTimeoutMilliseconds $RetryTimeoutMilliseconds `
            -RetryDelayMilliseconds 25
        $attributes = [IO.File]::GetAttributes($handle)
        if (($attributes -band [IO.FileAttributes]::Directory) -ne 0 -or
            ($attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0 -or
            [IO.RandomAccess]::GetLength($handle) -ne 0) {
            throw 'Upgrade transaction preflight held object drifted.'
        }
        return 'available'
    } catch [IO.IOException] {
        throw [IO.IOException]::new(
            "Upgrade preparation is blocked by an active Git transaction: " +
                $_.Exception.Message,
            $_.Exception
        )
    } finally {
        if ($null -ne $handle) { $handle.Dispose() }
        for ($index = $guards.Count - 1; $index -ge 0; $index--) {
            $guards[$index].Dispose()
        }
    }
}

function Prepare-BrokerUpgradeLauncher {
    param(
        [Parameter(Mandatory = $true)]
        [ValidateSet('install', 'upgrade')][string]$Action,
        [Parameter(Mandatory = $true)][string]$SourceFingerprint,
        [Parameter(Mandatory = $true)][string]$GoalId
    )
    if ($Action -ceq 'install') {
        if ((Test-Path -LiteralPath $InstallRoot) -or
            (Test-Path -LiteralPath $RequestRoot)) {
            throw 'Fresh-install launcher requires the fixed install and request roots to be absent.'
        }
        $transactionLockPreflight = 'not_present'
    } else {
        if (-not (Test-Path -LiteralPath $InstallRoot -PathType Container)) {
            throw 'Upgrade launcher requires the fixed protected install root.'
        }
        $transactionLockPreflight = Assert-BrokerUpgradeTransactionLockReady `
            -Root $InstallRoot
    }
    $manifest = New-BrokerUpgradeAuthorityManifest -Action $Action `
        -GoalId $GoalId -SourceFingerprint $SourceFingerprint
    $utf8 = [Text.UTF8Encoding]::new($false, $true)
    $manifestBytes = $utf8.GetBytes(($manifest | ConvertTo-Json -Depth 20 -Compress))
    $managedRoot = Assert-RealDirectory -Path (
        Join-Path $script:RepositoryRoot '.RaymanCodingSkill\tmp'
    ) -Label 'Broker mutation authority managed root'
    $manifestPath = Assert-ChildPath -Child (
        Join-Path $managedRoot ('broker-' + $Action + '-authority-' +
            [string]$manifest.nonce + '.json')
    ) -Parent $managedRoot -Label 'Broker mutation authority manifest'
    Write-BytesAtomic -Path $manifestPath -Bytes $manifestBytes
    $manifestSnapshot = Get-BrokerFileSnapshot -Path $manifestPath `
        -Label 'Broker mutation authority manifest' -MaximumBytes 2MB
    if (-not [Linq.Enumerable]::SequenceEqual(
            [byte[]]$manifestBytes, [byte[]]$manifestSnapshot.Bytes
        )) {
        throw 'Broker mutation authority manifest changed during publication.'
    }
    $runtime = $manifest.powershell
    $installerPath = Assert-ChildPath -Child (
        Join-Path $script:RepositoryRoot 'scripts\install-codex-powershell-broker.ps1'
    ) -Parent $script:RepositoryRoot -Label 'Broker mutation authority installer'
    $command = New-BrokerUpgradeUserCommandText -Action $Action `
        -PowerShellPath ([string]$runtime.path) `
        -PowerShellSha256 ([string]$runtime.sha256) `
        -InstallerPath $installerPath `
        -InstallerSha256 ([string]$manifest.installer_sha256) `
        -AuthorityManifestPath $manifestPath `
        -AuthorityManifestSha256 ([string]$manifestSnapshot.Sha256) `
        -GoalId $GoalId -SourceFingerprint $SourceFingerprint
    return [ordered]@{
        prepared = $true
        action = $Action
        goal_id = $GoalId
        source_fingerprint = $SourceFingerprint
        source_binding_count = @($manifest.source_bindings).Count
        transaction_lock_preflight = $transactionLockPreflight
        expected_head = [string]$manifest.expected_head
        nonce = [string]$manifest.nonce
        expires_at_utc = [string]$manifest.expires_at_utc
        authority_manifest_path = $manifestPath
        authority_manifest_sha256 = [string]$manifestSnapshot.Sha256
        powershell_path = [string]$runtime.path
        powershell_sha256 = [string]$runtime.sha256
        installer_sha256 = [string]$manifest.installer_sha256
        worker_sha256 = [string]$manifest.worker_sha256
        user_command = $command
        user_command_sha256 = Get-BytesSha256 -Bytes $utf8.GetBytes($command)
        mutable_launcher_file_published = $false
    }
}

function Assert-BrokerUpgradeAuthorityLifetime {
    param(
        [Parameter(Mandatory = $true)]$Manifest,
        [DateTimeOffset]$Now = [DateTimeOffset]::UtcNow
    )
    $createdAt = [DateTimeOffset]::MinValue
    $expiresAt = [DateTimeOffset]::MinValue
    $nonce = [Guid]::Empty
    if ([int]$Manifest.schema_version -ne 1 -or
        -not [DateTimeOffset]::TryParse(
            [string]$Manifest.created_at_utc,
            [Globalization.CultureInfo]::InvariantCulture,
            [Globalization.DateTimeStyles]::RoundtripKind,
            [ref]$createdAt
        ) -or
        -not [DateTimeOffset]::TryParse(
            [string]$Manifest.expires_at_utc,
            [Globalization.CultureInfo]::InvariantCulture,
            [Globalization.DateTimeStyles]::RoundtripKind,
            [ref]$expiresAt
        ) -or
        -not [Guid]::TryParseExact(
            [string]$Manifest.nonce, 'N', [ref]$nonce
        ) -or
        $createdAt -gt $Now.AddSeconds(30) -or
        $expiresAt -le $Now -or
        $expiresAt -gt $createdAt.AddMinutes(31)) {
        throw 'Upgrade authority manifest is expired, premature, or malformed.'
    }
    return [pscustomobject]@{
        CreatedAt = $createdAt
        ExpiresAt = $expiresAt
        Nonce = $nonce.ToString('N')
    }
}

function Open-BrokerUpgradeAuthority {
    param(
        [Parameter(Mandatory = $true)]
        [ValidateSet('install', 'upgrade')][string]$Action,
        [Parameter(Mandatory = $true)][string]$ManifestPath,
        [Parameter(Mandatory = $true)][string]$ManifestSha256,
        [Parameter(Mandatory = $true)][string]$GoalId,
        [Parameter(Mandatory = $true)][string]$SourceFingerprint,
        [Parameter(Mandatory = $true)][string]$Root,
        [Parameter(Mandatory = $true)][string]$Requests,
        [Parameter(Mandatory = $true)][string]$Name,
        [Parameter(Mandatory = $true)][string]$Account,
        [Parameter(Mandatory = $true)][string]$Group,
        [Parameter(Mandatory = $true)]$InstallationGuard
    )
    $pendingCapabilityKey = if ($Action -ceq 'install') {
        $script:InstallPendingCapabilityKey
    } else { $script:UpgradePendingCapabilityKey }
    $managedRoot = Assert-RealDirectory -Path (
        Join-Path $script:RepositoryRoot '.RaymanCodingSkill\tmp'
    ) -Label 'Upgrade authority managed root'
    $manifestPath = Assert-ChildPath -Child $ManifestPath -Parent $managedRoot -Label 'Upgrade authority manifest'
    $held = [Collections.Generic.List[object]]::new()
    try {
        $manifestHeld = Open-BrokerPinnedFile -Path $manifestPath -Label 'Upgrade authority manifest' -MaximumBytes 2MB -ExpectedSha256 $ManifestSha256
        $held.Add($manifestHeld)
        $manifest = ConvertFrom-StrictJsonBytes -Bytes $manifestHeld.Bytes -Label 'Upgrade authority manifest'
        Assert-ExactProperties -Document $manifest -Label 'Upgrade authority manifest' -Expected @(
            'action', 'created_at_utc', 'expected_head', 'expected_status', 'expires_at_utc',
            'git', 'git_inputs', 'goal_access_rules_protected',
            'goal_access_sddl', 'goal_id',
            'goal_identity', 'goal_owner_sid', 'goal_path', 'goal_sha256',
            'install_root', 'installation_guard_name', 'installer_sha256',
            'nonce', 'pending_access_rules_protected',
            'pending_access_sddl', 'pending_boundary_class',
            'pending_capability_key', 'pending_id', 'pending_identity',
            'pending_owner_sid', 'pending_package_sha256', 'pending_path',
            'pending_sha256', 'powershell', 'repository_root', 'request_root',
            'sandbox_group', 'schema_version', 'source_bindings',
            'source_fingerprint', 'task_name', 'user_account', 'worker_sha256'
        )
        [void](Assert-BrokerUpgradeAuthorityLifetime -Manifest $manifest)
        if ([string]$manifest.action -cne $Action -or
            [string]$manifest.repository_root -cne $script:RepositoryRoot -or
            [string]$manifest.install_root -cne $Root -or
            [string]$manifest.request_root -cne $Requests -or
            [string]$manifest.task_name -cne $Name -or
            [string]$manifest.user_account -cne $Account -or
            [string]$manifest.sandbox_group -cne $Group -or
            [string]$manifest.goal_id -cne $GoalId -or
            [string]$manifest.source_fingerprint -cne $SourceFingerprint -or
            [string]$manifest.pending_capability_key -cne
                $pendingCapabilityKey -or
            [string]$manifest.pending_boundary_class -cne
                $script:UpgradePendingBoundaryClass -or
            [string]$manifest.pending_id -notmatch '^pending_[0-9a-f]{10}$' -or
            [string]$manifest.pending_package_sha256 -notmatch
                '^[0-9a-f]{64}$' -or
            [string]$manifest.installation_guard_name -cne [string]$InstallationGuard.Name) {
            throw 'Upgrade authority manifest fixed tuple drifted.'
        }

        $goalHeld = Open-BrokerPinnedFile -Path ([string]$manifest.goal_path) -Label 'Upgrade authority Goal' -MaximumBytes 1MB -ExpectedSha256 ([string]$manifest.goal_sha256) -ExpectedIdentity ([string]$manifest.goal_identity) -ExpectedOwnerSid ([string]$manifest.goal_owner_sid) -ExpectedAccessSddl ([string]$manifest.goal_access_sddl) -RequireProtectedAcl:([bool]$manifest.goal_access_rules_protected)
        $held.Add($goalHeld)
        $goal = ConvertFrom-BrokerLauncherGoalBytes -Bytes $goalHeld.Bytes
        Assert-BrokerUpgradeGoalAuthority -Goal $goal -GoalId $GoalId `
            -SourceFingerprint $SourceFingerprint -Action $Action
        $pendingHeld = Open-BrokerPinnedFile `
            -Path ([string]$manifest.pending_path) `
            -Label 'Upgrade authority pending state' -MaximumBytes 1MB `
            -ExpectedSha256 ([string]$manifest.pending_sha256) `
            -ExpectedIdentity ([string]$manifest.pending_identity) `
            -ExpectedOwnerSid ([string]$manifest.pending_owner_sid) `
            -ExpectedAccessSddl ([string]$manifest.pending_access_sddl) `
            -RequireProtectedAcl:([bool]$manifest.pending_access_rules_protected)
        $held.Add($pendingHeld)
        $pending = ConvertFrom-BrokerPendingBytes -Bytes $pendingHeld.Bytes
        [void](Assert-BrokerUpgradePendingBoundary `
            -Pending $pending -GoalId $GoalId `
            -CapabilityKey ([string]$manifest.pending_capability_key) `
            -BoundaryClass ([string]$manifest.pending_boundary_class) `
            -ExpectedPendingId ([string]$manifest.pending_id) `
            -ExpectedPackageSha256 ([string]$manifest.pending_package_sha256))

        $boundFiles = @{}
        foreach ($role in @('powershell', 'git')) {
            $binding = $manifest.$role
            Assert-ExactProperties -Document $binding -Label ('Upgrade authority ' + $role) -Expected @(
                'access_rules_protected', 'access_sddl', 'identity',
                'owner_sid', 'path', 'sha256'
            )
            $file = Open-BrokerPinnedFile -Path ([string]$binding.path) -Label ('Upgrade authority ' + $role) -MaximumBytes 64MB -ExpectedSha256 ([string]$binding.sha256) -ExpectedIdentity ([string]$binding.identity) -ExpectedOwnerSid ([string]$binding.owner_sid) -ExpectedAccessSddl ([string]$binding.access_sddl) -RequireProtectedAcl:([bool]$binding.access_rules_protected)
            $held.Add($file)
            $boundFiles[$role] = $file
        }
        $currentRuntime = [Diagnostics.Process]::GetCurrentProcess().MainModule.FileName
        if (-not $currentRuntime.Equals(
                [string]$manifest.powershell.path,
                [StringComparison]::OrdinalIgnoreCase
            )) {
            throw 'Upgrade authority PowerShell runtime path drifted.'
        }

        if ($manifest.source_bindings -isnot [array] -or
            @($manifest.source_bindings).Count -ne $script:UpgradeSourcePaths.Count) {
            throw 'Upgrade authority source binding count drifted.'
        }
        Clear-BrokerGitEnvironment
        $gitPath = [string]$boundFiles['git'].Path
        $currentGitInputPaths = @($script:UpgradeGitInputCandidates | Where-Object {
            Test-Path -LiteralPath (Join-Path $script:RepositoryRoot $_) -PathType Leaf
        })
        $manifestGitInputs = @($manifest.git_inputs)
        if (($currentGitInputPaths -join [char]10) -cne
            (@($manifestGitInputs.relative_path) -join [char]10)) {
            throw 'Upgrade authority Git input set drifted.'
        }
        for ($index = 0; $index -lt $manifestGitInputs.Count; $index++) {
            $binding = $manifestGitInputs[$index]
            Assert-ExactProperties -Document $binding `
                -Label 'Upgrade authority Git input' -Expected @(
                    'access_rules_protected', 'access_sddl', 'identity',
                    'owner_sid', 'path', 'relative_path', 'sha256'
                )
            $expectedPath = Assert-ChildPath -Child (
                Join-Path $script:RepositoryRoot $currentGitInputPaths[$index]
            ) -Parent $script:RepositoryRoot -Label 'Upgrade authority Git input'
            if (-not $expectedPath.Equals(
                    [string]$binding.path,
                    [StringComparison]::OrdinalIgnoreCase
                )) {
                throw 'Upgrade authority Git input path drifted.'
            }
            $file = Open-BrokerPinnedFile -Path $expectedPath `
                -Label ('Upgrade authority Git input ' +
                    $currentGitInputPaths[$index]) -MaximumBytes 16MB `
                -ExpectedSha256 ([string]$binding.sha256) `
                -ExpectedIdentity ([string]$binding.identity) `
                -ExpectedOwnerSid ([string]$binding.owner_sid) `
                -ExpectedAccessSddl ([string]$binding.access_sddl) `
                -RequireProtectedAcl:([bool]$binding.access_rules_protected)
            $held.Add($file)
        }
        $sourceBindings = [Collections.Generic.List[object]]::new()
        for ($index = 0; $index -lt $script:UpgradeSourcePaths.Count; $index++) {
            $binding = @($manifest.source_bindings)[$index]
            Assert-ExactProperties -Document $binding -Label 'Upgrade authority source binding' -Expected @(
                'access_rules_protected', 'access_sddl', 'git_blob_oid',
                'git_mode', 'identity', 'owner_sid', 'path', 'relative_path',
                'sha256'
            )
            if ([string]$binding.relative_path -cne $script:UpgradeSourcePaths[$index]) {
                throw 'Upgrade authority source binding order or identity drifted.'
            }
            $expectedPath = Assert-ChildPath -Child (
                Join-Path $script:RepositoryRoot ([string]$binding.relative_path)
            ) -Parent $script:RepositoryRoot -Label 'Upgrade authority source'
            if (-not $expectedPath.Equals(
                    [string]$binding.path, [StringComparison]::OrdinalIgnoreCase
                )) {
                throw 'Upgrade authority source path drifted.'
            }
            $file = Open-BrokerPinnedFile -Path $expectedPath -Label ('Upgrade authority source ' + [string]$binding.relative_path) -MaximumBytes 4MB -ExpectedSha256 ([string]$binding.sha256) -ExpectedIdentity ([string]$binding.identity) -ExpectedOwnerSid ([string]$binding.owner_sid) -ExpectedAccessSddl ([string]$binding.access_sddl) -RequireProtectedAcl:([bool]$binding.access_rules_protected)
            $held.Add($file)
            $committed = Get-BrokerCommittedSourceBinding `
                -GitPath $gitPath -RepositoryRoot $script:RepositoryRoot `
                -ExpectedHead ([string]$manifest.expected_head) `
                -RelativePath ([string]$binding.relative_path) `
                -Bytes ([byte[]]$file.Bytes)
            if ([string]$committed.Mode -cne [string]$binding.git_mode -or
                [string]$committed.BlobOid -cne
                    [string]$binding.git_blob_oid) {
                throw 'Upgrade authority committed-source binding drifted.'
            }
            $sourceBindings.Add([pscustomobject]@{
                relative_path = [string]$binding.relative_path
                sha256 = [string]$file.Sha256
            })
        }
        if ((Get-BrokerGoalFingerprintFromBaseline -BaselineFiles $goal.baseline.files -SourceBindings @($sourceBindings)) -cne $SourceFingerprint) {
            throw 'Upgrade authority live source fingerprint drifted.'
        }

        $headOutput = @(& $gitPath --no-optional-locks -c core.fsmonitor=false -c core.hooksPath=NUL -c core.excludesFile=NUL -c "safe.directory=$script:RepositoryRoot" -C $script:RepositoryRoot rev-parse HEAD)
        $headExit = $LASTEXITCODE
        $statusOutput = @(& $gitPath --no-optional-locks -c core.fsmonitor=false -c core.hooksPath=NUL -c core.excludesFile=NUL -c "safe.directory=$script:RepositoryRoot" -C $script:RepositoryRoot status --porcelain=v1 --untracked-files=all)
        $statusExit = $LASTEXITCODE
        if ($headExit -ne 0 -or $statusExit -ne 0 -or $headOutput.Count -ne 1 -or
            [string]$headOutput[0].Trim() -cne [string]$manifest.expected_head -or
            ($statusOutput -join [char]10) -cne (@($manifest.expected_status) -join [char]10)) {
            throw 'Upgrade authority live HEAD or worktree status drifted.'
        }
        $installerBinding = @($manifest.source_bindings | Where-Object {
            [string]$_.relative_path -ceq 'scripts/install-codex-powershell-broker.ps1'
        })[0]
        $workerBinding = @($manifest.source_bindings | Where-Object {
            [string]$_.relative_path -ceq 'scripts/codex-powershell-broker.ps1'
        })[0]
        if ([string]$installerBinding.sha256 -cne [string]$manifest.installer_sha256 -or
            [string]$workerBinding.sha256 -cne [string]$manifest.worker_sha256) {
            throw 'Upgrade authority installer/worker binding drifted.'
        }
        return [pscustomobject]@{
            Manifest = $manifest
            Handles = $held
            TransactionId = [string]$manifest.nonce
            LauncherNonce = [string]$manifest.nonce
        }
    } catch {
        for ($index = $held.Count - 1; $index -ge 0; $index--) {
            Close-BrokerPinnedFile -Held $held[$index]
        }
        throw
    }
}

function Close-BrokerUpgradeAuthority {
    param([Parameter(Mandatory = $true)]$Authority)
    for ($index = $Authority.Handles.Count - 1; $index -ge 0; $index--) {
        Close-BrokerPinnedFile -Held $Authority.Handles[$index]
    }
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

function Register-BrokerTaskWithContext {
    param(
        [Parameter(Mandatory = $true)]$Context,
        [Parameter(Mandatory = $true)][string]$Name,
        [Parameter(Mandatory = $true)][string]$Xml
    )
    try {
        $registered = $Context.Folder.RegisterTask(
            $Name.TrimStart('\\'), $Xml,
            6,      # TASK_CREATE_OR_UPDATE
            $null, $null,
            3,      # TASK_LOGON_INTERACTIVE_TOKEN
            $null
        )
    } catch {
        throw "Task Scheduler COM registration failed for ${Name}: $($_.Exception.Message)"
    }
    if ($null -eq $registered -or
        [string]$registered.Path -ine $Name) {
        throw "Task Scheduler COM registered the wrong task. Expected=$Name Actual=$([string]$registered.Path)"
    }
    return $registered
}

function Register-BrokerTask {
    param(
        [Parameter(Mandatory = $true)][string]$Name,
        [Parameter(Mandatory = $true)][string]$Xml
    )
    $context = Get-BrokerTaskCom -Name $Name
    return (Register-BrokerTaskWithContext `
        -Context $context -Name $Name -Xml $Xml)
}

function Start-BrokerTaskWithContext {
    param(
        [Parameter(Mandatory = $true)]$Context,
        [Parameter(Mandatory = $true)][string]$Name
    )
    if ($null -eq $Context.Task) { throw "Broker task is missing: $Name" }
    try { $instance = $Context.Task.Run($null) }
    catch {
        throw "Task Scheduler COM start failed for ${Name}: $($_.Exception.Message)"
    }
    if ($null -eq $instance) {
        throw "Task Scheduler COM returned no running instance for $Name"
    }
    return $instance
}

function Start-BrokerTask {
    param([Parameter(Mandatory = $true)][string]$Name)
    return (Start-BrokerTaskWithContext `
        -Context (Get-BrokerTaskCom -Name $Name) -Name $Name)
}

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

function Get-BrokerTaskRuntimeDiagnostic {
    param([Parameter(Mandatory = $true)][string]$Name)

    try {
        $context = Get-BrokerTaskCom -Name $Name
        if ($null -eq $context.Task) {
            return [ordered]@{
                registered = $false
                state = $null
                last_task_result = $null
                last_task_result_hex = $null
                last_run_time = $null
                running_instances = 0
                error = $null
            }
        }
        $task = $context.Task
        $lastResult = [int64]$task.LastTaskResult
        $lastRun = [DateTime]$task.LastRunTime
        return [ordered]@{
            registered = $true
            state = [int]$task.State
            last_task_result = $lastResult
            last_task_result_hex = '0x{0:X8}' -f (
                [uint32]($lastResult -band 0xffffffffL)
            )
            last_run_time = $lastRun.ToUniversalTime().ToString('o')
            running_instances = @($task.GetInstances(0)).Count
            error = $null
        }
    } catch {
        return [ordered]@{
            registered = $null
            state = $null
            last_task_result = $null
            last_task_result_hex = $null
            last_run_time = $null
            running_instances = $null
            error = $_.Exception.Message.Replace("`r", ' ').Replace("`n", ' ')
        }
    }
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

function ConvertFrom-StrictJsonBytes {
    param(
        [Parameter(Mandatory = $true)][byte[]]$Bytes,
        [Parameter(Mandatory = $true)][string]$Label
    )
    if ($Bytes.Length -le 0 -or $Bytes.Length -gt 64KB) {
        throw "$Label is not bounded JSON."
    }
    try {
        [Rayman.CodexBrokerInstallerNative]::ValidateUniqueJsonProperties($Bytes)
    } catch {
        throw "$Label has duplicate, case-colliding, or structurally invalid JSON properties: $($_.Exception.Message)"
    }
    $text = [Text.UTF8Encoding]::new($false, $true).GetString($Bytes)
    return $text | ConvertFrom-Json `
        -Depth 12 -NoEnumerate -DateKind String -ErrorAction Stop
}

function Read-JsonDocument {
    param([string]$Path, [string]$Label)
    $item = Get-Item -LiteralPath $Path -Force -ErrorAction Stop
    if ($item.PSIsContainer -or
        ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0 -or
        $item.Length -le 0 -or $item.Length -gt 64KB) {
        throw "$Label is not a bounded regular file: $Path"
    }
    return ConvertFrom-StrictJsonBytes `
        -Bytes ([IO.File]::ReadAllBytes($Path)) -Label $Label
}

function Read-BrokerLauncherGoalDocument {
    param([Parameter(Mandatory = $true)][string]$Path)
    $item = Get-Item -LiteralPath $Path -Force -ErrorAction Stop
    if ($item.PSIsContainer -or
        ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0 -or
        $item.Length -le 0 -or $item.Length -gt 1MB) {
        throw "Upgrade launcher Goal is not a bounded regular file: $Path"
    }
    return ConvertFrom-BrokerLauncherGoalBytes `
        -Bytes ([IO.File]::ReadAllBytes($Path))
}

function ConvertFrom-BrokerLauncherGoalBytes {
    param([Parameter(Mandatory = $true)][byte[]]$Bytes)
    if ($Bytes.Length -le 0 -or $Bytes.Length -gt 1MB) {
        throw 'Upgrade launcher Goal bytes are not bounded.'
    }
    try {
        [Rayman.CodexBrokerInstallerNative]::ValidateUniqueJsonProperties($Bytes)
        $text = [Text.UTF8Encoding]::new($false, $true).GetString($Bytes)
        return $text | ConvertFrom-Json `
            -Depth 64 -NoEnumerate -DateKind String -ErrorAction Stop
    } catch {
        throw "Upgrade launcher Goal is not strict unique-property JSON: $($_.Exception.Message)"
    }
}

function ConvertFrom-BrokerPendingBytes {
    param([Parameter(Mandatory = $true)][byte[]]$Bytes)
    if ($Bytes.Length -le 0 -or $Bytes.Length -gt 1MB) {
        throw 'Upgrade authority pending bytes are not bounded.'
    }
    try {
        [Rayman.CodexBrokerInstallerNative]::ValidateUniqueJsonProperties($Bytes)
        $text = [Text.UTF8Encoding]::new($false, $true).GetString($Bytes)
        return $text | ConvertFrom-Json `
            -Depth 64 -NoEnumerate -DateKind String -ErrorAction Stop
    } catch {
        throw "Upgrade authority pending state is not strict unique-property JSON: $($_.Exception.Message)"
    }
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

function Assert-BrokerUpgradeRecoveryReceiptDocument {
    param(
        [Parameter(Mandatory = $true)]$Receipt,
        [Parameter(Mandatory = $true)][int]$Schema,
        [Parameter(Mandatory = $true)][string]$Root,
        [Parameter(Mandatory = $true)][string]$Requests,
        [Parameter(Mandatory = $true)][string]$Name,
        [Parameter(Mandatory = $true)][string]$Account,
        [Parameter(Mandatory = $true)][string]$UserSid,
        [Parameter(Mandatory = $true)][string]$Group,
        [Parameter(Mandatory = $true)][string]$SandboxSid,
        [Parameter(Mandatory = $true)][string]$Label
    )
    $expected = @(
        'capabilities', 'install_id', 'install_root', 'installed_at_utc',
        'powershell_path', 'powershell_sha256', 'request_root', 'result_root',
        'sandbox_group', 'sandbox_group_sid', 'schema_version', 'task_name',
        'user_account', 'user_sid', 'worker_path', 'worker_sha256'
    )
    if ($Schema -eq $script:SchemaVersion) {
        $expected += @(
            'git_capability_manifest_path', 'git_capability_manifest_sha256'
        )
    }
    Assert-ExactProperties -Document $Receipt -Expected $expected -Label $Label
    $capabilities = @($Receipt.capabilities)
    $capabilityShapeValid = if ($Schema -eq $script:LegacySchemaVersion) {
        $capabilities.Count -eq 1 -and
        [string]$capabilities[0] -ceq 'identity_probe'
    } else {
        $capabilities.Count -eq 2 -and
        [string]$capabilities[0] -ceq 'identity_probe' -and
        [string]$capabilities[1] -ceq $script:GitCapabilityId
    }
    $installId = [Guid]::Empty
    if ([int]$Receipt.schema_version -ne $Schema -or
        [string]$Receipt.install_root -cne $Root -or
        [string]$Receipt.request_root -cne $Requests -or
        [string]$Receipt.result_root -cne (Join-Path $Root 'results') -or
        [string]$Receipt.task_name -cne $Name -or
        [string]$Receipt.user_account -cne $Account -or
        [string]$Receipt.user_sid -cne $UserSid -or
        [string]$Receipt.sandbox_group -cne $Group -or
        [string]$Receipt.sandbox_group_sid -cne $SandboxSid -or
        $Receipt.capabilities -isnot [array] -or
        -not $capabilityShapeValid -or
        -not [Guid]::TryParseExact(
            [string]$Receipt.install_id, 'N', [ref]$installId
        ) -or
        [string]$Receipt.worker_sha256 -notmatch '^[0-9a-f]{64}$' -or
        [string]$Receipt.worker_path -cne (Join-Path (
            Join-Path (Join-Path $Root 'versions') `
                ([string]$Receipt.worker_sha256)
        ) 'codex-powershell-broker.ps1') -or
        [string]$Receipt.powershell_sha256 -notmatch '^[0-9a-f]{64}$') {
        throw "$Label is not bound to the exact upgrade installation tuple."
    }
    [void](Get-NormalizedAbsolutePath `
        -Path ([string]$Receipt.powershell_path) -Label "$Label PowerShell")
    if ($Schema -eq $script:SchemaVersion -and (
        [string]$Receipt.git_capability_manifest_path -cne
            (Join-Path $Root $script:GitCapabilityManifestName) -or
        [string]$Receipt.git_capability_manifest_sha256 -notmatch
            '^[0-9a-f]{64}$')) {
        throw "$Label has an invalid Git capability binding."
    }
}

function New-BrokerUpgradeRecoveryJournalDocument {
    param(
        [Parameter(Mandatory = $true)][string]$Root,
        [Parameter(Mandatory = $true)][string]$Requests,
        [Parameter(Mandatory = $true)][string]$Name,
        [Parameter(Mandatory = $true)][string]$Account,
        [Parameter(Mandatory = $true)][string]$UserSid,
        [Parameter(Mandatory = $true)][string]$Group,
        [Parameter(Mandatory = $true)][string]$SandboxSid,
        [Parameter(Mandatory = $true)][byte[]]$LegacyReceiptBytes,
        [Parameter(Mandatory = $true)][string]$LegacyReceiptIdentity,
        [Parameter(Mandatory = $true)][byte[]]$StagedReceiptBytes,
        [Parameter(Mandatory = $true)][string]$LegacyTaskXml,
        [Parameter(Mandatory = $true)][string]$TransactionId,
        [Parameter(Mandatory = $true)][string]$InstallationGuardName,
        [Parameter(Mandatory = $true)][string]$LauncherNonce,
        [Parameter(Mandatory = $true)][string]$GoalId,
        [Parameter(Mandatory = $true)][string]$SourceFingerprint,
        [Parameter(Mandatory = $true)][array]$StagingObjects,
        [ValidateSet(2, 3)]
        [int]$JournalSchemaVersion =
            $script:UpgradeRecoveryJournalSchemaVersion,
        [AllowNull()][byte[]]$LegacyReadyBytes,
        [AllowNull()][string]$LegacyReadyIdentity,
        [AllowNull()][byte[]]$StagedReadyBytes
    )
    if ($JournalSchemaVersion -eq
            $script:CurrentUpgradeRecoveryJournalSchemaVersion) {
        if (-not $PSBoundParameters.ContainsKey('LegacyReadyBytes') -or
            -not $PSBoundParameters.ContainsKey('LegacyReadyIdentity') -or
            -not $PSBoundParameters.ContainsKey('StagedReadyBytes') -or
            $null -eq $LegacyReadyBytes -or
            $LegacyReadyBytes.Count -eq 0 -or
            [string]$LegacyReadyIdentity -notmatch
                '^[0-9a-f]{16}:[0-9a-f]{32}$' -or
            $null -eq $StagedReadyBytes -or
            $StagedReadyBytes.Count -eq 0) {
            throw 'Current-schema upgrade journal requires both exact ready-marker states.'
        }
    } elseif ($PSBoundParameters.ContainsKey('LegacyReadyBytes') -or
        $PSBoundParameters.ContainsKey('LegacyReadyIdentity') -or
        $PSBoundParameters.ContainsKey('StagedReadyBytes')) {
        throw 'Legacy upgrade journal cannot carry current-schema ready-marker states.'
    }
    $taskBytes = [Text.UTF8Encoding]::new($false, $true).GetBytes($LegacyTaskXml)
    $document = [ordered]@{
        schema_version = $JournalSchemaVersion
        created_at_utc = [DateTimeOffset]::UtcNow.ToString('o')
        transaction_id = $TransactionId
        installation_guard_name = $InstallationGuardName
        launcher_nonce = $LauncherNonce
        goal_id = $GoalId
        source_fingerprint = $SourceFingerprint
        install_root = $Root
        request_root = $Requests
        task_name = $Name
        user_account = $Account
        user_sid = $UserSid
        sandbox_group = $Group
        sandbox_group_sid = $SandboxSid
        legacy_receipt_sha256 = Get-BytesSha256 -Bytes $LegacyReceiptBytes
        legacy_receipt_identity = $LegacyReceiptIdentity
        legacy_receipt_base64 = [Convert]::ToBase64String($LegacyReceiptBytes)
        staged_receipt_sha256 = Get-BytesSha256 -Bytes $StagedReceiptBytes
        staged_receipt_base64 = [Convert]::ToBase64String($StagedReceiptBytes)
        legacy_task_xml_sha256 = Get-BytesSha256 -Bytes $taskBytes
        legacy_task_xml_base64 = [Convert]::ToBase64String($taskBytes)
        staging_objects = @($StagingObjects)
    }
    if ($JournalSchemaVersion -eq
            $script:CurrentUpgradeRecoveryJournalSchemaVersion) {
        $document['legacy_ready_sha256'] =
            Get-BytesSha256 -Bytes $LegacyReadyBytes
        $document['legacy_ready_identity'] = $LegacyReadyIdentity
        $document['legacy_ready_base64'] =
            [Convert]::ToBase64String($LegacyReadyBytes)
        $document['staged_ready_sha256'] =
            Get-BytesSha256 -Bytes $StagedReadyBytes
        $document['staged_ready_base64'] =
            [Convert]::ToBase64String($StagedReadyBytes)
    }
    return $document
}

function Read-BrokerUpgradeRecoveryJournal {
    param(
        [Parameter(Mandatory = $true)][string]$Root,
        [Parameter(Mandatory = $true)][string]$Requests,
        [Parameter(Mandatory = $true)][string]$Name,
        [Parameter(Mandatory = $true)][string]$Account,
        [Parameter(Mandatory = $true)][string]$UserSid,
        [Parameter(Mandatory = $true)][string]$Group,
        [Parameter(Mandatory = $true)][string]$SandboxSid,
        [Parameter(Mandatory = $true)]$ExpectedFileSecurity
    )
    $path = Join-Path $Root $script:UpgradeRecoveryJournalName
    if (-not (Test-Path -LiteralPath $path)) { return $null }
    $snapshot = Get-BrokerFileSnapshot `
        -Path $path -Label 'Broker upgrade recovery journal' -MaximumBytes 1MB
    $expectedAccess = $ExpectedFileSecurity.GetSecurityDescriptorSddlForm(
        [Security.AccessControl.AccessControlSections]::Access
    )
    if (-not $snapshot.AccessRulesProtected -or
        [string]$snapshot.OwnerSid -cne $UserSid -or
        [string]$snapshot.AccessSddl -cne $expectedAccess) {
        throw 'Broker upgrade recovery journal held-handle owner/DACL drifted.'
    }
    $journal = ConvertFrom-StrictJsonBytes `
        -Bytes $snapshot.Bytes -Label 'Broker upgrade recovery journal'
    $schema = [int]$journal.schema_version
    $expectedProperties = @(
        'created_at_utc', 'install_root', 'legacy_receipt_base64',
        'legacy_receipt_sha256', 'legacy_task_xml_base64',
        'legacy_task_xml_sha256', 'request_root', 'sandbox_group',
        'sandbox_group_sid', 'schema_version', 'staged_receipt_base64',
        'staged_receipt_sha256', 'task_name', 'user_account', 'user_sid'
    )
    if ($schema -in @(
            $script:UpgradeRecoveryJournalSchemaVersion,
            $script:CurrentUpgradeRecoveryJournalSchemaVersion
        )) {
        $expectedProperties += @(
            'installation_guard_name', 'launcher_nonce',
            'legacy_receipt_identity', 'staging_objects', 'transaction_id',
            'goal_id', 'source_fingerprint'
        )
        if ($schema -eq
                $script:CurrentUpgradeRecoveryJournalSchemaVersion) {
            $expectedProperties += @(
                'legacy_ready_base64', 'legacy_ready_identity',
                'legacy_ready_sha256', 'staged_ready_base64',
                'staged_ready_sha256'
            )
        }
    } elseif ($schema -ne 1) {
        throw 'Broker upgrade recovery journal schema is unsupported.'
    }
    Assert-ExactProperties -Document $journal `
        -Label 'Broker upgrade recovery journal' -Expected $expectedProperties
    $createdAt = [DateTimeOffset]::MinValue
    if ([string]$journal.install_root -cne $Root -or
        [string]$journal.request_root -cne $Requests -or
        [string]$journal.task_name -cne $Name -or
        [string]$journal.user_account -cne $Account -or
        [string]$journal.user_sid -cne $UserSid -or
        [string]$journal.sandbox_group -cne $Group -or
        [string]$journal.sandbox_group_sid -cne $SandboxSid -or
        -not [DateTimeOffset]::TryParse(
            [string]$journal.created_at_utc,
            [Globalization.CultureInfo]::InvariantCulture,
            [Globalization.DateTimeStyles]::RoundtripKind,
            [ref]$createdAt
        )) {
        throw 'Broker upgrade recovery journal tuple is invalid.'
    }
    try {
        $legacyBytes = [Convert]::FromBase64String(
            [string]$journal.legacy_receipt_base64
        )
        $stagedBytes = [Convert]::FromBase64String(
            [string]$journal.staged_receipt_base64
        )
        $taskBytes = [Convert]::FromBase64String(
            [string]$journal.legacy_task_xml_base64
        )
        $legacyReadyBytes = if ($schema -eq
                $script:CurrentUpgradeRecoveryJournalSchemaVersion) {
            [Convert]::FromBase64String(
                [string]$journal.legacy_ready_base64
            )
        } else { $null }
        $stagedReadyBytes = if ($schema -eq
                $script:CurrentUpgradeRecoveryJournalSchemaVersion) {
            [Convert]::FromBase64String(
                [string]$journal.staged_ready_base64
            )
        } else { $null }
    } catch {
        throw "Broker upgrade recovery journal base64 is invalid: $($_.Exception.Message)"
    }
    $hashEntries = @(
        @($legacyBytes, [string]$journal.legacy_receipt_sha256, 'legacy receipt'),
        @($stagedBytes, [string]$journal.staged_receipt_sha256, 'staged receipt'),
        @($taskBytes, [string]$journal.legacy_task_xml_sha256, 'legacy task XML')
    )
    if ($schema -eq
            $script:CurrentUpgradeRecoveryJournalSchemaVersion) {
        $hashEntries += @(
            @($legacyReadyBytes, [string]$journal.legacy_ready_sha256,
                'legacy ready marker'),
            @($stagedReadyBytes, [string]$journal.staged_ready_sha256,
                'staged ready marker')
        )
    }
    foreach ($entry in $hashEntries) {
        if ($entry[1] -notmatch '^[0-9a-f]{64}$' -or
            (Get-BytesSha256 -Bytes $entry[0]) -cne $entry[1]) {
            throw "Broker upgrade recovery journal $($entry[2]) hash is invalid."
        }
    }
    $legacy = ConvertFrom-StrictJsonBytes `
        -Bytes $legacyBytes -Label 'Broker upgrade recovery legacy receipt'
    $staged = ConvertFrom-StrictJsonBytes `
        -Bytes $stagedBytes -Label 'Broker upgrade recovery staged receipt'
    $legacySchema = [int]$legacy.schema_version
    $expectedLegacySchema = if ($schema -eq
            $script:CurrentUpgradeRecoveryJournalSchemaVersion) {
        $script:SchemaVersion
    } else { $script:LegacySchemaVersion }
    Assert-BrokerUpgradeRecoveryReceiptDocument `
        -Receipt $legacy -Schema $expectedLegacySchema `
        -Root $Root -Requests $Requests -Name $Name -Account $Account `
        -UserSid $UserSid -Group $Group -SandboxSid $SandboxSid `
        -Label 'Broker upgrade recovery legacy receipt'
    Assert-BrokerUpgradeRecoveryReceiptDocument `
        -Receipt $staged -Schema $script:SchemaVersion `
        -Root $Root -Requests $Requests -Name $Name -Account $Account `
        -UserSid $UserSid -Group $Group -SandboxSid $SandboxSid `
        -Label 'Broker upgrade recovery staged receipt'
    if ($legacySchema -ne $expectedLegacySchema -or
        [string]$legacy.worker_sha256 -ceq [string]$staged.worker_sha256 -or
        [string]$legacy.powershell_path -cne [string]$staged.powershell_path -or
        [string]$legacy.powershell_sha256 -cne
            [string]$staged.powershell_sha256) {
        throw 'Broker upgrade recovery journal did not preserve distinct receipts under one runtime.'
    }
    if (($legacySchema -eq $script:LegacySchemaVersion -and
            [string]$legacy.install_id -ceq [string]$staged.install_id) -or
        ($legacySchema -eq $script:SchemaVersion -and (
            [string]$legacy.install_id -cne [string]$staged.install_id -or
            [string]$legacy.git_capability_manifest_path -cne
                [string]$staged.git_capability_manifest_path -or
            [string]$legacy.git_capability_manifest_sha256 -cne
                [string]$staged.git_capability_manifest_sha256
        ))) {
        throw 'Broker upgrade recovery journal installation identity transition is invalid.'
    }
    $legacyReady = $null
    $stagedReady = $null
    if ($schema -eq
            $script:CurrentUpgradeRecoveryJournalSchemaVersion) {
        $legacyReady = ConvertFrom-StrictJsonBytes `
            -Bytes $legacyReadyBytes `
            -Label 'Broker upgrade recovery legacy ready marker'
        $stagedReady = ConvertFrom-StrictJsonBytes `
            -Bytes $stagedReadyBytes `
            -Label 'Broker upgrade recovery staged ready marker'
        foreach ($entry in @(
            @($legacyReady, $legacy, 'legacy'),
            @($stagedReady, $staged, 'staged')
        )) {
            $ready = $entry[0]
            $receipt = $entry[1]
            $label = [string]$entry[2]
            Assert-ExactProperties -Document $ready -Expected @(
                'git_capability_manifest_sha256', 'install_id',
                'ready_at_utc', 'schema_version', 'worker_sha256'
            ) -Label "Broker upgrade recovery $label ready marker"
            $readyAt = [DateTimeOffset]::MinValue
            if ([int]$ready.schema_version -ne 1 -or
                [string]$ready.install_id -cne
                    [string]$receipt.install_id -or
                [string]$ready.worker_sha256 -cne
                    [string]$receipt.worker_sha256 -or
                [string]$ready.git_capability_manifest_sha256 -cne
                    [string]$receipt.git_capability_manifest_sha256 -or
                -not [DateTimeOffset]::TryParse(
                    [string]$ready.ready_at_utc,
                    [Globalization.CultureInfo]::InvariantCulture,
                    [Globalization.DateTimeStyles]::RoundtripKind,
                    [ref]$readyAt
                ) -or
                $readyAt -gt [DateTimeOffset]::UtcNow.AddSeconds(30)) {
                throw "Broker upgrade recovery $label ready marker is invalid."
            }
        }
    }
    $transactionId = [string]$staged.install_id
    $installationGuardName = Get-BrokerInstallationMutexName -Root $Root
    $launcherNonce = $null
    $legacyReceiptIdentity = $null
    $stagingObjects = @()
    if ($schema -in @(
            $script:UpgradeRecoveryJournalSchemaVersion,
            $script:CurrentUpgradeRecoveryJournalSchemaVersion
        )) {
        $parsedTransaction = [Guid]::Empty
        $parsedNonce = [Guid]::Empty
        if (-not [Guid]::TryParseExact(
                [string]$journal.transaction_id, 'N', [ref]$parsedTransaction
            ) -or
            ($schema -eq $script:UpgradeRecoveryJournalSchemaVersion -and
                [string]$journal.transaction_id -cne $transactionId) -or
            [string]$journal.installation_guard_name -cne
                $installationGuardName -or
            -not [Guid]::TryParseExact(
                [string]$journal.launcher_nonce, 'N', [ref]$parsedNonce
            ) -or
            [string]$journal.goal_id -notmatch '^goal_[0-9a-f]{10}$' -or
            [string]$journal.source_fingerprint -notmatch '^[0-9a-f]{64}$' -or
            [string]$journal.legacy_receipt_identity -notmatch
                '^[0-9a-f]{16}:[0-9a-f]{32}$' -or
            $journal.staging_objects -isnot [array]) {
            throw 'Broker upgrade recovery journal transaction binding is invalid.'
        }
        $transactionId = [string]$journal.transaction_id
        $launcherNonce = [string]$journal.launcher_nonce
        $legacyReceiptIdentity = [string]$journal.legacy_receipt_identity
        $seenStagingPaths = [Collections.Generic.HashSet[string]]::new(
            [StringComparer]::OrdinalIgnoreCase
        )
        foreach ($object in @($journal.staging_objects)) {
            Assert-ExactProperties -Document $object `
                -Expected @(
                    'access_sddl', 'identity', 'owner_sid',
                    'path', 'sha256', 'type'
                ) `
                -Label 'Broker upgrade recovery staging object'
            $objectPath = Assert-ChildPath `
                -Child ([string]$object.path) -Parent $Root `
                -Label 'Broker upgrade recovery staging object'
            if (-not $seenStagingPaths.Add($objectPath) -or
                [string]$object.identity -notmatch
                    '^[0-9a-f]{16}:[0-9a-f]{32}$' -or
                [string]$object.owner_sid -notmatch '^S-1-' -or
                [string]::IsNullOrWhiteSpace([string]$object.access_sddl) -or
                [string]$object.type -notin @('file', 'directory') -or
                ([string]$object.type -ceq 'file' -and
                    [string]$object.sha256 -notmatch '^[0-9a-f]{64}$') -or
                ([string]$object.type -ceq 'directory' -and
                    $null -ne $object.sha256)) {
                throw 'Broker upgrade recovery staging object binding is invalid.'
            }
            $stagingObjects += $object
        }
        if ($stagingObjects.Count -eq 0) {
            throw 'Broker upgrade recovery journal has no owned staging objects.'
        }
        if ($schema -eq
                $script:CurrentUpgradeRecoveryJournalSchemaVersion -and
            [string]$journal.legacy_ready_identity -notmatch
                '^[0-9a-f]{16}:[0-9a-f]{32}$') {
            throw 'Broker upgrade recovery journal ready identity is invalid.'
        }
    }
    $taskXml = [Text.UTF8Encoding]::new($false, $true).GetString($taskBytes)
    Assert-BrokerTaskXmlBinding -Xml $taskXml `
        -PowerShellPath ([string]$legacy.powershell_path) `
        -WorkerPath ([string]$legacy.worker_path) `
        -BrokerRoot $Root -Requests $Requests `
        -Account $Account -UserSid $UserSid
    return [pscustomobject]@{
        Path = $path
        Sha256 = [string]$snapshot.Sha256
        Identity = [string]$snapshot.Identity
        OwnerSid = [string]$snapshot.OwnerSid
        AccessSddl = [string]$snapshot.AccessSddl
        SchemaVersion = $schema
        LegacySchemaVersion = $legacySchema
        TransactionId = $transactionId
        InstallationGuardName = $installationGuardName
        LauncherNonce = $launcherNonce
        GoalId = $(if ($schema -in @(
                $script:UpgradeRecoveryJournalSchemaVersion,
                $script:CurrentUpgradeRecoveryJournalSchemaVersion
            )) {
            [string]$journal.goal_id
        } else { $null })
        SourceFingerprint = $(if ($schema -in @(
                $script:UpgradeRecoveryJournalSchemaVersion,
                $script:CurrentUpgradeRecoveryJournalSchemaVersion
            )) {
            [string]$journal.source_fingerprint
        } else { $null })
        LegacyReceiptIdentity = $legacyReceiptIdentity
        StagingObjects = $stagingObjects
        Journal = $journal
        LegacyReceipt = $legacy
        LegacyReceiptBytes = $legacyBytes
        StagedReceipt = $staged
        StagedReceiptBytes = $stagedBytes
        LegacyTaskXml = $taskXml
        LegacyReady = $legacyReady
        LegacyReadyBytes = $legacyReadyBytes
        LegacyReadyIdentity = $(if ($schema -eq
                $script:CurrentUpgradeRecoveryJournalSchemaVersion) {
            [string]$journal.legacy_ready_identity
        } else { $null })
        StagedReady = $stagedReady
        StagedReadyBytes = $stagedReadyBytes
    }
}

function Remove-BrokerUpgradeRecoveryJournal {
    param([Parameter(Mandatory = $true)]$Recovery)
    [void](Remove-BrokerFileExact `
        -Path ([string]$Recovery.Path) `
        -ExpectedSha256 ([string]$Recovery.Sha256) `
        -ExpectedIdentity ([string]$Recovery.Identity) `
        -ExpectedOwnerSid ([string]$Recovery.OwnerSid) `
        -ExpectedAccessSddl ([string]$Recovery.AccessSddl) `
        -MaximumBytes 1MB -Label 'Broker upgrade recovery journal')
}

function Get-BrokerUpgradeConsumedNoncePath {
    param(
        [Parameter(Mandatory = $true)][string]$Root,
        [Parameter(Mandatory = $true)][string]$Nonce
    )
    $parsed = [Guid]::Empty
    if (-not [Guid]::TryParseExact($Nonce, 'N', [ref]$parsed)) {
        throw 'Broker upgrade launcher nonce is invalid.'
    }
    return Assert-ChildPath -Child (
        Join-Path $Root ($script:UpgradeConsumedNoncePrefix + $Nonce + '.json')
    ) -Parent $Root -Label 'Broker upgrade consumed nonce'
}

function Write-BrokerUpgradeConsumedNonce {
    param(
        [Parameter(Mandatory = $true)][string]$Root,
        [Parameter(Mandatory = $true)][string]$Nonce,
        [Parameter(Mandatory = $true)][string]$TransactionId,
        [Parameter(Mandatory = $true)][string]$GoalId,
        [Parameter(Mandatory = $true)][string]$SourceFingerprint,
        [Parameter(Mandatory = $true)]$FileSecurity,
        [Parameter(Mandatory = $true)][string]$UserSid
    )
    $path = Get-BrokerUpgradeConsumedNoncePath -Root $Root -Nonce $Nonce
    if (Test-Path -LiteralPath $path) {
        throw 'Broker upgrade authority nonce was already consumed.'
    }
    $document = [ordered]@{
        schema_version = 1
        nonce = $Nonce
        transaction_id = $TransactionId
        goal_id = $GoalId
        source_fingerprint = $SourceFingerprint
        consumed_at_utc = [DateTimeOffset]::UtcNow.ToString('o')
    }
    Write-JsonAtomic -Path $path -Document $document -Security $FileSecurity
    Assert-ExactSecurity -Path $path -Expected $FileSecurity `
        -ExpectedOwnerSid $UserSid -Label 'Broker upgrade consumed nonce'
    return Get-BrokerFileSnapshot `
        -Path $path -Label 'Broker upgrade consumed nonce' -MaximumBytes 64KB
}

function Assert-BrokerUpgradeNonceUnconsumed {
    param(
        [Parameter(Mandatory = $true)][string]$Root,
        [Parameter(Mandatory = $true)][string]$Nonce
    )
    $path = Get-BrokerUpgradeConsumedNoncePath -Root $Root -Nonce $Nonce
    if (Test-Path -LiteralPath $path) {
        throw 'Broker upgrade authority nonce was already consumed; generate a new authority manifest.'
    }
}

function Ensure-BrokerUpgradeConsumedNonceForRecovery {
    param(
        [Parameter(Mandatory = $true)]$Recovery,
        [Parameter(Mandatory = $true)][string]$Root,
        [Parameter(Mandatory = $true)]$FileSecurity,
        [Parameter(Mandatory = $true)][string]$UserSid
    )
    if ([int]$Recovery.SchemaVersion -notin @(
            $script:UpgradeRecoveryJournalSchemaVersion,
            $script:CurrentUpgradeRecoveryJournalSchemaVersion
        )) {
        return
    }
    $path = Get-BrokerUpgradeConsumedNoncePath `
        -Root $Root -Nonce ([string]$Recovery.LauncherNonce)
    if (-not (Test-Path -LiteralPath $path)) {
        [void](Write-BrokerUpgradeConsumedNonce `
            -Root $Root -Nonce ([string]$Recovery.LauncherNonce) `
            -TransactionId ([string]$Recovery.TransactionId) `
            -GoalId ([string]$Recovery.GoalId) `
            -SourceFingerprint ([string]$Recovery.SourceFingerprint) `
            -FileSecurity $FileSecurity -UserSid $UserSid)
        return
    }
    $snapshot = Get-BrokerFileSnapshot `
        -Path $path -Label 'Broker upgrade consumed nonce' -MaximumBytes 64KB
    $expectedAccess = $FileSecurity.GetSecurityDescriptorSddlForm(
        [Security.AccessControl.AccessControlSections]::Access
    )
    $document = ConvertFrom-StrictJsonBytes `
        -Bytes $snapshot.Bytes -Label 'Broker upgrade consumed nonce'
    Assert-ExactProperties -Document $document `
        -Label 'Broker upgrade consumed nonce' -Expected @(
            'consumed_at_utc', 'goal_id', 'nonce', 'schema_version',
            'source_fingerprint', 'transaction_id'
        )
    if (-not $snapshot.AccessRulesProtected -or
        [string]$snapshot.OwnerSid -cne $UserSid -or
        [string]$snapshot.AccessSddl -cne $expectedAccess -or
        [int]$document.schema_version -ne 1 -or
        [string]$document.nonce -cne [string]$Recovery.LauncherNonce -or
        [string]$document.transaction_id -cne [string]$Recovery.TransactionId -or
        [string]$document.goal_id -cne [string]$Recovery.GoalId -or
        [string]$document.source_fingerprint -cne
            [string]$Recovery.SourceFingerprint) {
        throw 'Broker upgrade consumed nonce recovery binding drifted.'
    }
}

function Read-Receipt {
    param(
        [Parameter(Mandatory = $true)][string]$Root,
        [Parameter(Mandatory = $true)][string]$Requests,
        [Parameter(Mandatory = $true)][string]$Name,
        [switch]$SkipTaskValidation,
        [switch]$SkipGitLiveValidation
    )
    $receipt = Read-JsonDocument `
        -Path (Join-Path $Root $script:ReceiptName) `
        -Label 'Broker install receipt'
    $schema = [int]$receipt.schema_version
    if ($schema -ne $script:LegacySchemaVersion -and
        $schema -ne $script:SchemaVersion) {
        throw 'Broker install receipt schema is unsupported.'
    }
    $expectedProperties = @(
        'capabilities', 'install_id', 'install_root', 'installed_at_utc',
        'powershell_path', 'powershell_sha256', 'request_root', 'result_root',
        'sandbox_group', 'sandbox_group_sid', 'schema_version', 'task_name',
        'user_account', 'user_sid', 'worker_path', 'worker_sha256'
    )
    if ($schema -eq $script:SchemaVersion) {
        $expectedProperties += @(
            'git_capability_manifest_path', 'git_capability_manifest_sha256'
        )
    }
    Assert-ExactProperties -Document $receipt -Label 'Broker install receipt' -Expected @(
        $expectedProperties
    )
    $capabilityShapeValid = if ($schema -eq $script:LegacySchemaVersion) {
        @($receipt.capabilities).Count -eq 1 -and
        [string]$receipt.capabilities[0] -ceq 'identity_probe'
    } else {
        @($receipt.capabilities).Count -eq 2 -and
        [string]$receipt.capabilities[0] -ceq 'identity_probe' -and
        [string]$receipt.capabilities[1] -ceq $script:GitCapabilityId
    }
    if (
        [string]$receipt.install_root -cne $Root -or
        [string]$receipt.request_root -cne $Requests -or
        [string]$receipt.result_root -cne (Join-Path $Root 'results') -or
        [string]$receipt.task_name -cne $Name -or
        $receipt.capabilities -isnot [array] -or
        -not $capabilityShapeValid) {
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
    if ($schema -eq $script:SchemaVersion) {
        foreach ($entry in @(
            @((Join-Path $Root $script:GitTransactionDirectoryName), 'Git transaction root'),
            @((Join-Path $Root 'empty-hooks'), 'Protected empty hooks root')
        )) {
            [void](Assert-RealDirectory -Path $entry[0] -Label $entry[1])
            Assert-ExactSecurity -Path $entry[0] -Expected $readOnly `
                -ExpectedOwnerSid ([string]$receipt.user_sid) -Label $entry[1]
        }
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
    if ($schema -eq $script:SchemaVersion) {
        $manifestPath = Get-NormalizedAbsolutePath `
            -Path ([string]$receipt.git_capability_manifest_path) `
            -Label 'Git capability manifest'
        if ($manifestPath -cne (Join-Path $Root $script:GitCapabilityManifestName) -or
            [string]$receipt.git_capability_manifest_sha256 -notmatch '^[0-9a-f]{64}$' -or
            -not (Test-Path -LiteralPath $manifestPath -PathType Leaf) -or
            (Get-FileSha256 -Path $manifestPath) -cne
                [string]$receipt.git_capability_manifest_sha256) {
            throw 'git_local_commit_v1 capability manifest receipt binding is invalid.'
        }
        foreach ($entry in @(
            @($manifestPath, 'Git capability manifest'),
            @((Join-Path (Join-Path $Root $script:GitTransactionDirectoryName) `
                'transaction.lock'), 'Git transaction lock')
        )) {
            Assert-ExactSecurity -Path $entry[0] -Expected $fileSecurity `
                -ExpectedOwnerSid ([string]$receipt.user_sid) -Label $entry[1]
        }
        $manifest = Read-JsonDocument -Path $manifestPath `
            -Label 'git_local_commit_v1 capability manifest'
        Assert-GitCapabilityManifestInstalled `
            -Manifest $manifest -Receipt $receipt -Root $Root `
            -SkipLiveRepository:$SkipGitLiveValidation
        if (@(Get-ChildItem -LiteralPath (Join-Path $Root 'empty-hooks') `
            -Force).Count -ne 0) {
            throw 'Protected empty hooks root contains an entry.'
        }
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
        [DateTimeOffset]$NotBefore = [DateTimeOffset]::MinValue,
        [string]$TaskName
    )
    $heartbeatPath = Join-Path (Join-Path $Root 'results') $script:HeartbeatName
    $deadline = [DateTimeOffset]::UtcNow.AddSeconds($TimeoutSeconds)
    $lastHeartbeatError = 'heartbeat file is absent'
    do {
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
                if ($heartbeat.schema_version -eq [int]$Receipt.schema_version -and
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
                throw 'Broker heartbeat exists but is stale or not bound to the expected receipt.'
            } catch {
                $lastHeartbeatError = $_.Exception.Message.Replace(
                    "`r", ' '
                ).Replace("`n", ' ')
            }
        }
        if ([DateTimeOffset]::UtcNow -ge $deadline) { break }
        Start-Sleep -Milliseconds 200
    } while ($true)
    $taskDiagnostic = if ([string]::IsNullOrWhiteSpace($TaskName)) {
        [ordered]@{ available = $false; reason = 'task name not supplied' }
    } else {
        Get-BrokerTaskRuntimeDiagnostic -Name $TaskName
    }
    $taskText = ($taskDiagnostic | ConvertTo-Json -Depth 5 -Compress)
    throw "Broker task did not publish a fresh qinrm-bound heartbeat. " +
        "timeout_seconds=$TimeoutSeconds " +
        "last_heartbeat_error=$lastHeartbeatError task_runtime=$taskText"
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
        -Root $Root -Receipt $Receipt -TimeoutSeconds 2 `
        -TaskName ([string]$Receipt.task_name)
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

function Open-GitTransactionGuard {
    param([Parameter(Mandatory = $true)][string]$Path)

    return [IO.FileStream]::new(
        $Path, [IO.FileMode]::Open, [IO.FileAccess]::ReadWrite,
        [IO.FileShare]::None
    )
}

function Get-GitTransactionCleanupLockPath {
    param([Parameter(Mandatory = $true)][string]$Path)

    [void](Assert-RealDirectory -Path $Path -Label 'Git transaction cleanup root')
    $entries = @(Get-ChildItem -LiteralPath $Path -Force -ErrorAction Stop)
    if ($entries.Count -ne 1 -or
        [string]$entries[0].Name -cne 'transaction.lock' -or
        $entries[0].PSIsContainer -or
        ($entries[0].Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0 -or
        $entries[0].Length -ne 0) {
        throw 'Installer refuses to remove a Git transaction directory with recovery evidence or unknown contents.'
    }
    return [string]$entries[0].FullName
}

function Get-InstallerComparableJsonText {
    param([Parameter(Mandatory = $true)]$Document)

    return ($Document | ConvertTo-Json -Depth 16 -Compress)
}

function Get-TerminalGitTransactionJournalBinding {
    param(
        [Parameter(Mandatory = $true)][string]$JournalPath,
        [Parameter(Mandatory = $true)][string]$Root,
        [Parameter(Mandatory = $true)]$Receipt,
        [Parameter(Mandatory = $true)]
        [Security.AccessControl.FileSecurity]$FileSecurity,
        [Parameter(Mandatory = $true)][string]$UserSid,
        [string]$RepositoryRoot = $script:RepositoryRoot
    )

    $fileName = [IO.Path]::GetFileName($JournalPath)
    if ($fileName -notmatch '^([0-9a-f]{32})\.journal\.json$') {
        throw 'Git transaction journal name is not a fixed request identity.'
    }
    $requestId = [string]$Matches[1]
    $journalSnapshot = Get-BrokerFileSnapshot -Path $JournalPath `
        -Label 'Terminal Git transaction journal' -MaximumBytes 512KB
    $journalSecurityMode = Assert-TerminalGitArtifactSecurity `
        -Snapshot $journalSnapshot -Expected $FileSecurity `
        -ExpectedOwnerSid $UserSid -Label 'Terminal Git transaction journal'
    $journal = ConvertFrom-StrictJsonBytes -Bytes $journalSnapshot.Bytes `
        -Label 'Terminal Git transaction journal'
    Assert-ExactProperties -Document $journal `
        -Label 'Terminal Git transaction journal' -Expected @(
        'alternate_index_path', 'alternate_index_sha256', 'backup_index_path',
        'commit_bytes_base64', 'commit_message_sha256', 'commit_oid',
        'created_at_utc', 'head_before', 'head_tree_before', 'index_before_sha256',
        'index_path', 'install_id', 'output', 'phase', 'remote_refs_before_sha256',
        'other_refs_before_sha256', 'request_id', 'request_sha256',
        'schema_version', 'stage_index_path', 'tree_oid', 'updated_at_utc'
    )
    Assert-ExactProperties -Document $journal.output `
        -Label 'Terminal Git transaction journal output' -Expected @(
        'child_process_policy', 'changed_path_count', 'changed_paths',
        'changed_paths_digest', 'commit_message_sha256', 'commit_oid',
        'git_sha256', 'head_after', 'head_before', 'hooks_disabled',
        'index_after_sha256', 'index_before_sha256', 'mutation_phase',
        'network_operation', 'other_refs_sha256', 'parent', 'post_clean',
        'recovery_state', 'ref', 'remote_refs_sha256', 'repository_id',
        'transaction_id', 'tree_oid'
    )
    $gitDirectory = Join-Path $RepositoryRoot '.git'
    if ([int]$journal.schema_version -ne 1 -or
        [string]$journal.install_id -cne [string]$Receipt.install_id -or
        [string]$journal.request_id -cne $requestId -or
        [string]$journal.request_sha256 -notmatch '^[0-9a-f]{64}$' -or
        [string]$journal.phase -cne 'verified' -or
        [string]$journal.commit_oid -notmatch '^[0-9a-f]{40}$' -or
        [string]$journal.tree_oid -notmatch '^[0-9a-f]{40}$' -or
        [string]$journal.output.transaction_id -cne $requestId -or
        [string]$journal.output.commit_oid -cne [string]$journal.commit_oid -or
        [string]$journal.output.tree_oid -cne [string]$journal.tree_oid -or
        [string]$journal.output.head_before -cne [string]$journal.head_before -or
        [string]$journal.output.parent -cne [string]$journal.head_before -or
        [string]$journal.output.head_after -cne [string]$journal.commit_oid -or
        [string]$journal.output.index_before_sha256 -cne
            [string]$journal.index_before_sha256 -or
        [string]$journal.output.commit_message_sha256 -cne
            [string]$journal.commit_message_sha256 -or
        [string]$journal.output.remote_refs_sha256 -cne
            [string]$journal.remote_refs_before_sha256 -or
        [string]$journal.output.other_refs_sha256 -cne
            [string]$journal.other_refs_before_sha256 -or
        [string]$journal.index_path -cne (Join-Path $gitDirectory 'index') -or
        [string]$journal.alternate_index_path -cne
            (Join-Path (Split-Path -Parent $JournalPath) ($requestId + '.index')) -or
        [string]$journal.stage_index_path -cne
            (Join-Path $gitDirectory 'index.lock') -or
        [string]$journal.backup_index_path -cne
            (Join-Path $gitDirectory (
                '.rayman-git-local-commit-' + $requestId + '.backup'
            )) -or
        [string]$journal.output.mutation_phase -cne 'verified' -or
        [string]$journal.output.recovery_state -cne 'none' -or
        [bool]$journal.output.post_clean -ne $true -or
        [bool]$journal.output.network_operation -ne $false) {
        throw 'Git transaction journal is not a terminal verified commit.'
    }

    $expectedResultsRoot = Join-Path $Root 'results'
    $expectedRequestsRoot = Join-Path $Root 'requests'
    if ([string]$Receipt.result_root -cne $expectedResultsRoot -or
        [string]$Receipt.request_root -cne $expectedRequestsRoot) {
        throw 'Terminal Git transaction receipt roots drifted.'
    }
    $resultPath = Assert-ChildPath `
        -Child (Join-Path $expectedResultsRoot ($requestId + '.result.json')) `
        -Parent $expectedResultsRoot -Label 'Terminal Git transaction result'
    $resultSnapshot = Get-BrokerFileSnapshot -Path $resultPath `
        -Label 'Terminal Git transaction result' -MaximumBytes 256KB
    $resultSecurityMode = Assert-TerminalGitArtifactSecurity `
        -Snapshot $resultSnapshot -Expected $FileSecurity `
        -ExpectedOwnerSid $UserSid -Label 'Terminal Git transaction result'
    $result = ConvertFrom-StrictJsonBytes -Bytes $resultSnapshot.Bytes `
        -Label 'Terminal Git transaction result'
    Assert-ExactProperties -Document $result `
        -Label 'Terminal Git transaction result' -Expected @(
        'error', 'error_code', 'executor_account', 'executor_sid', 'exit_code',
        'finished_at_utc', 'install_id', 'operation', 'output',
        'powershell_sha256', 'request_id', 'request_sha256',
        'schema_version', 'started_at_utc', 'status', 'worker_sha256'
    )
    Assert-ExactProperties -Document $result.output `
        -Label 'Terminal Git transaction result output' -Expected @(
        'child_process_policy', 'changed_path_count', 'changed_paths',
        'changed_paths_digest', 'commit_message_sha256', 'commit_oid',
        'git_sha256', 'head_after', 'head_before', 'hooks_disabled',
        'index_after_sha256', 'index_before_sha256', 'mutation_phase',
        'network_operation', 'other_refs_sha256', 'parent', 'post_clean',
        'recovery_state', 'ref', 'remote_refs_sha256', 'repository_id',
        'transaction_id', 'tree_oid'
    )
    if ([int]$result.schema_version -ne $script:SchemaVersion -or
        [string]$result.install_id -cne [string]$Receipt.install_id -or
        [string]$result.request_id -cne $requestId -or
        [string]$result.operation -cne $script:GitCapabilityId -or
        [string]$result.status -cne 'success' -or
        [int]$result.exit_code -ne 0 -or
        [string]$result.executor_account -cne [string]$Receipt.user_account -or
        [string]$result.executor_sid -cne [string]$Receipt.user_sid -or
        [string]$result.worker_sha256 -cne [string]$Receipt.worker_sha256 -or
        [string]$result.powershell_sha256 -cne [string]$Receipt.powershell_sha256 -or
        [string]$result.request_sha256 -cne [string]$journal.request_sha256 -or
        -not [string]::IsNullOrEmpty([string]$result.error_code) -or
        -not [string]::IsNullOrEmpty([string]$result.error) -or
        (Get-InstallerComparableJsonText -Document $result.output) -cne
            (Get-InstallerComparableJsonText -Document $journal.output)) {
        throw 'Terminal Git transaction result does not match its verified journal.'
    }
    if (Test-Path -LiteralPath (
            Join-Path ([string]$Receipt.request_root) ($requestId + '.request.json')
        )) {
        throw 'Terminal Git transaction still has a live request.'
    }
    foreach ($scratch in @(
        [string]$journal.alternate_index_path,
        [string]$journal.backup_index_path,
        [string]$journal.stage_index_path
    )) {
        if (-not [string]::IsNullOrEmpty($scratch) -and
            (Test-Path -LiteralPath $scratch)) {
            throw 'Terminal Git transaction still has journal-owned scratch.'
        }
    }
    return [pscustomobject]@{
        Path = [string]$journalSnapshot.Path
        Sha256 = [string]$journalSnapshot.Sha256
        Identity = [string]$journalSnapshot.Identity
        OwnerSid = [string]$journalSnapshot.OwnerSid
        AccessSddl = [string]$journalSnapshot.AccessSddl
        SecurityMode = [string]$journalSecurityMode
        RequestId = $requestId
        ResultSha256 = [string]$resultSnapshot.Sha256
        ResultIdentity = [string]$resultSnapshot.Identity
        ResultOwnerSid = [string]$resultSnapshot.OwnerSid
        ResultAccessSddl = [string]$resultSnapshot.AccessSddl
        ResultSecurityMode = [string]$resultSecurityMode
    }
}

function Get-GitTransactionOperationalState {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$Root,
        [Parameter(Mandatory = $true)]$Receipt,
        [Parameter(Mandatory = $true)]
        [Security.AccessControl.FileSecurity]$FileSecurity,
        [Parameter(Mandatory = $true)][string]$UserSid,
        [string]$RepositoryRoot = $script:RepositoryRoot
    )

    $expectedTransactions = Join-Path $Root $script:GitTransactionDirectoryName
    $expectedResults = Join-Path $Root 'results'
    if ([IO.Path]::GetFullPath($Path) -cne
            [IO.Path]::GetFullPath($expectedTransactions)) {
        throw 'Git transaction operational root escaped the installed tuple.'
    }
    $readOnlySecurity = New-ManagedDirectorySecurity `
        -UserSid $UserSid `
        -SandboxSid ([string]$Receipt.sandbox_group_sid) -Kind ReadOnly
    foreach ($entry in @(
        @($Path, 'Git transaction operational root'),
        @($expectedResults, 'Git transaction result root')
    )) {
        [void](Assert-RealDirectory -Path $entry[0] -Label $entry[1])
        Assert-ExactSecurity -Path $entry[0] -Expected $readOnlySecurity `
            -ExpectedOwnerSid $UserSid -Label $entry[1]
    }
    $entries = @(Get-ChildItem -LiteralPath $Path -Force -ErrorAction Stop)
    if ($entries.Count -lt 1 -or $entries.Count -gt 129) {
        throw 'Git transaction operational root has an invalid entry count.'
    }
    $locks = @($entries | Where-Object {
        [string]$_.Name -ceq 'transaction.lock'
    })
    if ($locks.Count -ne 1 -or $locks[0].PSIsContainer -or
        ($locks[0].Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0 -or
        $locks[0].Length -ne 0) {
        throw 'Git transaction operational lock is missing or invalid.'
    }
    $journalEntries = @($entries | Where-Object {
        [string]$_.Name -cne 'transaction.lock'
    })
    foreach ($entry in $journalEntries) {
        if ($entry.PSIsContainer -or
            ($entry.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0 -or
            [string]$entry.Name -notmatch '^[0-9a-f]{32}\.journal\.json$') {
            throw 'Git transaction operational root contains unknown recovery state.'
        }
    }
    $journalNames = [string[]]@($journalEntries | ForEach-Object Name)
    [Array]::Sort($journalNames, [StringComparer]::Ordinal)
    $bindings = [Collections.Generic.List[object]]::new()
    foreach ($name in $journalNames) {
        $bindings.Add((Get-TerminalGitTransactionJournalBinding `
            -JournalPath (Join-Path $Path $name) -Root $Root `
            -Receipt $Receipt -FileSecurity $FileSecurity -UserSid $UserSid `
            -RepositoryRoot $RepositoryRoot))
    }
    return [pscustomobject]@{
        LockPath = [string]$locks[0].FullName
        Journals = @($bindings)
    }
}

function Remove-TerminalGitTransactionJournalsForUpgrade {
    param(
        [Parameter(Mandatory = $true)]$ExpectedState,
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$Root,
        [Parameter(Mandatory = $true)]$Receipt,
        [Parameter(Mandatory = $true)]
        [Security.AccessControl.FileSecurity]$FileSecurity,
        [Parameter(Mandatory = $true)][string]$UserSid,
        [string]$RepositoryRoot = $script:RepositoryRoot
    )

    $current = Get-GitTransactionOperationalState -Path $Path -Root $Root `
        -Receipt $Receipt -FileSecurity $FileSecurity -UserSid $UserSid `
        -RepositoryRoot $RepositoryRoot
    $expectedBindings = @($ExpectedState.Journals)
    $currentBindings = @($current.Journals)
    if ([string]$current.LockPath -cne [string]$ExpectedState.LockPath -or
        $currentBindings.Count -ne $expectedBindings.Count) {
        throw 'Git transaction terminal journal set changed before retirement.'
    }
    for ($index = 0; $index -lt $expectedBindings.Count; $index++) {
        $expected = $expectedBindings[$index]
        $actual = $currentBindings[$index]
        if ([string]$actual.Path -cne [string]$expected.Path -or
            [string]$actual.Sha256 -cne [string]$expected.Sha256 -or
            [string]$actual.Identity -cne [string]$expected.Identity -or
            [string]$actual.OwnerSid -cne [string]$expected.OwnerSid -or
            [string]$actual.AccessSddl -cne [string]$expected.AccessSddl -or
            [string]$actual.SecurityMode -cne [string]$expected.SecurityMode -or
            [string]$actual.ResultSha256 -cne [string]$expected.ResultSha256 -or
            [string]$actual.ResultIdentity -cne [string]$expected.ResultIdentity -or
            [string]$actual.ResultOwnerSid -cne [string]$expected.ResultOwnerSid -or
            [string]$actual.ResultAccessSddl -cne
                [string]$expected.ResultAccessSddl -or
            [string]$actual.ResultSecurityMode -cne
                [string]$expected.ResultSecurityMode) {
            throw 'Git transaction terminal journal binding changed before retirement.'
        }
    }
    foreach ($binding in $expectedBindings) {
        [void](Remove-BrokerFileExact -Path ([string]$binding.Path) `
            -ExpectedSha256 ([string]$binding.Sha256) `
            -ExpectedIdentity ([string]$binding.Identity) `
            -ExpectedOwnerSid ([string]$binding.OwnerSid) `
            -ExpectedAccessSddl ([string]$binding.AccessSddl) `
            -Label 'Terminal Git transaction journal retirement' `
            -MaximumBytes 512KB)
    }
    if ((Get-GitTransactionCleanupLockPath -Path $Path) -cne
        [string]$ExpectedState.LockPath) {
        throw 'Git transaction root did not converge to its protected lock.'
    }
    return $expectedBindings.Count
}

function Assert-GitTransactionDirectorySafeForInstallerCleanup {
    param([Parameter(Mandatory = $true)][string]$Path)

    $lockPath = Get-GitTransactionCleanupLockPath -Path $Path
    $probe = Open-GitTransactionGuard -Path $lockPath
    $probe.Dispose()
}

function Get-InstallationState {
    param(
        [string]$Root,
        [string]$Requests,
        [string]$Name,
        [switch]$TransactionInProgress
    )
    if ($TransactionInProgress) {
        return [ordered]@{
            installed = $null
            task_registered = $null
            heartbeat_ready = $false
            transaction_in_progress = $true
            recovery_journal_state = 'not_checked'
            recovery_required = $false
            receipt_state = 'not_checked'
            ready_state = 'not_checked'
            identity_probe_ready = $false
            git_capability_ready = $false
            operational_ready = $false
            install_root = $Root
            request_root = $Requests
            task_name = $Name
            install_id = $null
            user_account = $null
            user_sid = $null
            worker_sha256 = $null
            error = 'Broker installation transaction is in progress; no mixed-epoch paths were read.'
        }
    }

    $errors = [Collections.Generic.List[string]]::new()
    $journalState = 'absent'
    $recoveryRequired = $false
    $journalPath = Join-Path $Root $script:UpgradeRecoveryJournalName
    $journalItem = Get-Item -LiteralPath $journalPath -Force -ErrorAction SilentlyContinue
    if ($null -ne $journalItem) {
        $recoveryRequired = $true
        try {
            if ($journalItem.PSIsContainer -or
                ($journalItem.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
                throw 'journal path is not an ordinary file'
            }
            $snapshot = Get-BrokerFileSnapshot -Path $journalPath -Label 'Broker upgrade recovery debt' -MaximumBytes 1MB
            $journal = ConvertFrom-StrictJsonBytes -Bytes $snapshot.Bytes -Label 'Broker upgrade recovery debt'
            if ([string]$journal.user_sid -notmatch '^S-1-' -or
                [string]$journal.sandbox_group_sid -notmatch '^S-1-' -or
                [string]::IsNullOrWhiteSpace([string]$journal.user_account) -or
                [string]::IsNullOrWhiteSpace([string]$journal.sandbox_group)) {
                throw 'journal identity tuple is incomplete'
            }
            $journalSecurity = New-ManagedFileSecurity `
                -UserSid ([string]$journal.user_sid) `
                -SandboxSid ([string]$journal.sandbox_group_sid)
            $validatedJournal = Read-BrokerUpgradeRecoveryJournal `
                -Root $Root -Requests $Requests -Name $Name `
                -Account ([string]$journal.user_account) `
                -UserSid ([string]$journal.user_sid) `
                -Group ([string]$journal.sandbox_group) `
                -SandboxSid ([string]$journal.sandbox_group_sid) `
                -ExpectedFileSecurity $journalSecurity
            $journalState = 'present_valid_schema_' +
                [int]$validatedJournal.SchemaVersion
        } catch {
            $journalState = 'invalid'
            $errors.Add('upgrade recovery journal is invalid: ' + $_.Exception.Message)
        }
    }

    $receipt = $null
    $installed = $null
    $receiptState = 'absent'
    try {
        $receipt = Read-Receipt -Root $Root -Requests $Requests -Name $Name
        $installed = $true
        $receiptState = 'schema_v' + [int]$receipt.schema_version
    } catch {
        $errors.Add($_.Exception.Message)
        if (-not (Test-Path -LiteralPath $Root)) {
            $installed = $false
        } else {
            $receiptState = 'invalid'
        }
    }

    $taskRegistered = $null
    try {
        $taskRegistered = $null -ne (Get-BrokerTaskCom -Name $Name).Task
    } catch {
        $errors.Add($_.Exception.Message)
    }

    $identityProbeReady = $false
    if ($null -ne $receipt) {
        try {
            [void](Wait-BrokerHeartbeat -Root $Root -Receipt $receipt -TimeoutSeconds 1)
            $identityProbeReady = $true
        } catch {
            $errors.Add($_.Exception.Message)
        }
    }

    $readyPath = Join-Path $Root $script:GitCapabilityReadyName
    $readyItem = Get-Item -LiteralPath $readyPath -Force -ErrorAction SilentlyContinue
    $readyState = 'absent'
    $gitCapabilityReady = $false
    if ($null -ne $readyItem) {
        if ($readyItem.PSIsContainer -or
            ($readyItem.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
            $readyState = 'invalid'
            $errors.Add('Git capability ready marker is not an ordinary file.')
        } elseif ($null -eq $receipt -or
            [int]$receipt.schema_version -ne $script:SchemaVersion) {
            $readyState = 'invalid'
            $errors.Add('Git capability ready marker exists without a schema-v3 receipt.')
        } else {
            try {
                [void](Assert-GitCapabilityReadyInstalled -Receipt $receipt -Root $Root)
                $readyState = 'valid'
                $gitCapabilityReady = $true
            } catch {
                $readyState = 'invalid'
                $errors.Add($_.Exception.Message)
            }
        }
    }
    if ($null -ne $receipt -and
        [int]$receipt.schema_version -eq $script:SchemaVersion -and
        -not $gitCapabilityReady) {
        $errors.Add('Schema-v3 receipt is not capability-ready.')
    }
    if ($recoveryRequired) {
        $errors.Add('Broker installation has a retained upgrade recovery journal.')
    }

    $operationalReady = $identityProbeReady -and
        (-not ($null -ne $receipt -and
            [int]$receipt.schema_version -eq $script:SchemaVersion) -or
            $gitCapabilityReady) -and
        -not $recoveryRequired
    return [ordered]@{
        installed = $installed
        task_registered = $taskRegistered
        heartbeat_ready = $identityProbeReady
        transaction_in_progress = $false
        recovery_journal_state = $journalState
        recovery_required = $recoveryRequired
        receipt_state = $receiptState
        ready_state = $readyState
        identity_probe_ready = $identityProbeReady
        git_capability_ready = $gitCapabilityReady
        operational_ready = $operationalReady
        install_root = if ($null -ne $receipt) { [string]$receipt.install_root } else { $Root }
        request_root = if ($null -ne $receipt) { [string]$receipt.request_root } else { $Requests }
        task_name = if ($null -ne $receipt) { [string]$receipt.task_name } else { $Name }
        install_id = if ($null -ne $receipt) { [string]$receipt.install_id } else { $null }
        user_account = if ($null -ne $receipt) { [string]$receipt.user_account } else { $null }
        user_sid = if ($null -ne $receipt) { [string]$receipt.user_sid } else { $null }
        worker_sha256 = if ($null -ne $receipt) { [string]$receipt.worker_sha256 } else { $null }
        error = if ($errors.Count -eq 0) { $null } else { $errors -join '; ' }
    }
}
function Get-BrokerExistingInstallProof {
    param(
        [Parameter(Mandatory = $true)][string]$Root,
        [Parameter(Mandatory = $true)][string]$Requests,
        [Parameter(Mandatory = $true)][string]$Name,
        [Parameter(Mandatory = $true)][string]$Account,
        [Parameter(Mandatory = $true)][string]$Group
    )
    $userSid = Resolve-AccountSid -Account $Account -Label 'Target user'
    $sandboxSid = Resolve-AccountSid -Account $Group -Label 'Sandbox group'
    $expectedRequests = Join-Path $Root 'requests'
    if ($Requests -cne $expectedRequests) {
        throw "Request root must be the protected install-root child: $expectedRequests"
    }
    if ($Name -notmatch '^\\[A-Za-z0-9._-]+$') {
        throw "Broker task name must be one fixed root task: $Name"
    }
    $runtime = Get-CurrentPowerShellRuntime
    $source = Open-BrokerPinnedFile -Path $script:BrokerSource `
        -Label 'Existing-install client source' -MaximumBytes 4MB
    try {
        $existing = Read-Receipt -Root $Root -Requests $Requests `
            -Name $Name -SkipTaskValidation
        if ([int]$existing.schema_version -eq $script:LegacySchemaVersion) {
            throw 'The protected broker is schema v2; use the reviewed -Upgrade -Yes transaction.'
        }
        if ([string]$existing.user_account -cne $Account -or
            [string]$existing.user_sid -cne $userSid -or
            [string]$existing.sandbox_group -cne $Group -or
            [string]$existing.sandbox_group_sid -cne $sandboxSid -or
            [string]$existing.worker_sha256 -cne [string]$source.Sha256 -or
            [string]$existing.powershell_path -cne $runtime.Path -or
            [string]$existing.powershell_sha256 -cne $runtime.Sha256) {
            throw 'An existing broker install is not the exact current source tuple.'
        }
        [void](Assert-GitCapabilityReadyInstalled -Receipt $existing -Root $Root)
        $probeOutput = @(& $runtime.Path -NoProfile -File $source.Path `
            -Operation identity_probe -InstallRoot $Root `
            -RequestRoot $Requests -TimeoutSeconds 30 2>&1)
        $probeExit = $LASTEXITCODE
        if ($probeExit -ne 0 -or $probeOutput.Count -eq 0) {
            throw "Existing-install identity probe failed: exit=$probeExit output=$($probeOutput -join ' | ')"
        }
        try {
            $probe = ($probeOutput -join [char]10) |
                ConvertFrom-Json -Depth 20 -NoEnumerate -DateKind String
        } catch {
            throw "Existing-install identity probe returned invalid JSON: $($_.Exception.Message)"
        }
        if ($probe -is [array] -or
            [string]$probe.operation -cne 'identity_probe' -or
            [string]$probe.status -cne 'success' -or
            [int]$probe.exit_code -ne 0 -or
            [string]$probe.install_id -cne [string]$existing.install_id -or
            [string]$probe.worker_sha256 -cne [string]$existing.worker_sha256 -or
            [string]$probe.executor_account -cne [string]$existing.user_account -or
            [string]$probe.executor_sid -cne [string]$existing.user_sid) {
            throw 'Existing-install identity probe did not prove the protected current tuple.'
        }
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
            worker_sha256 = [string]$source.Sha256
            powershell_sha256 = $runtime.Sha256
            git_capability_manifest_sha256 =
                [string]$existing.git_capability_manifest_sha256
            heartbeat_executor = [string]$probe.executor_account
            heartbeat_sid = [string]$probe.executor_sid
            attestation_operation = 'identity_probe'
        }
    } finally { Close-BrokerPinnedFile -Held $source }
}

function Install-Broker {
    param(
        [string]$Root,
        [string]$Requests,
        [string]$Name,
        [string]$Account,
        [string]$Group,
        [AllowNull()]$SourceAuthority
    )
    if (Test-Path -LiteralPath $Root) {
        if ($null -ne $SourceAuthority) {
            throw 'Existing-install attestation rejects a fresh-install authority manifest.'
        }
        return (Get-BrokerExistingInstallProof `
            -Root $Root -Requests $Requests -Name $Name `
            -Account $Account -Group $Group)
    }
    if ($null -eq $SourceAuthority -or
        [string]$SourceAuthority.Manifest.action -cne 'install') {
        throw 'Fresh install requires a clean-source one-shot authority manifest.'
    }
    $workerAuthority = @($SourceAuthority.Handles | Where-Object {
        $_.Path.Equals(
            $script:BrokerSource,
            [StringComparison]::OrdinalIgnoreCase
        )
    })
    if ($workerAuthority.Count -ne 1) {
        throw 'Fresh-install authority did not retain the worker source handle.'
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
    $sourceBytes = [byte[]]$workerAuthority[0].Bytes
    $workerHash = Get-BytesSha256 -Bytes $sourceBytes
    $runtime = Get-CurrentPowerShellRuntime
    $existingTask = (Get-BrokerTaskCom -Name $Name).Task
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
    $transactionGuard = $null
    $installCommitted = $false
    $createdPaths = [Collections.Generic.List[object]]::new()
    $rootBinding = $null
    $root = $Root
    $requests = $Requests
    $versions = Join-Path $root 'versions'
    $results = Join-Path $root 'results'
    $transactions = Join-Path $root $script:GitTransactionDirectoryName
    $hooks = Join-Path $root 'empty-hooks'
    $versionRoot = Join-Path $versions $workerHash
    $workerPath = Join-Path $versionRoot 'codex-powershell-broker.ps1'
    $workerLockPath = Join-Path $root 'worker.lock'
    $transactionLockPath = Join-Path $transactions 'transaction.lock'
    $receiptPath = Join-Path $root $script:ReceiptName
    $manifestPath = Join-Path $root $script:GitCapabilityManifestName
    $readyPath = Join-Path $root $script:GitCapabilityReadyName
    $installId = [Guid]::NewGuid().ToString('N')
    $manifest = New-GitCapabilityManifest `
        -InstallId $installId -HooksRoot $hooks -TransactionsRoot $transactions
    $manifestBytes = [Text.UTF8Encoding]::new($false, $true).GetBytes(
        ($manifest | ConvertTo-Json -Depth 12 -Compress)
    )
    $manifestHash = Get-BytesSha256 -Bytes $manifestBytes
    $readyDocument = New-GitCapabilityReadyDocument `
        -InstallId $installId -WorkerHash $workerHash -ManifestHash $manifestHash
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
        capabilities = @('identity_probe', $script:GitCapabilityId)
        git_capability_manifest_path = $manifestPath
        git_capability_manifest_sha256 = $manifestHash
    }
    $taskXml = New-BrokerTaskXml `
        -PowerShellPath $runtime.Path -WorkerPath $workerPath -BrokerRoot $root `
        -Requests $requests -Account $Account -Name $Name

    try {
        $root = New-ManagedDirectory -Path $root -Security $readOnlySecurity `
            -OwnerSid $userSid -Label 'Broker install root'
        $rootCreated = $true
        $rootBinding = New-BrokerUpgradeOwnedObjectBinding `
            -Path $root -Type directory
        $versions = New-ManagedDirectory -Path $versions -Security $readOnlySecurity `
            -OwnerSid $userSid -Label 'Broker version root'
        $createdPaths.Add((New-BrokerUpgradeOwnedObjectBinding `
            -Path $versions -Type directory))
        $results = New-ManagedDirectory -Path $results -Security $readOnlySecurity `
            -OwnerSid $userSid -Label 'Broker result root'
        $createdPaths.Add((New-BrokerUpgradeOwnedObjectBinding `
            -Path $results -Type directory))
        $transactions = New-ManagedDirectory -Path $transactions `
            -Security $readOnlySecurity -OwnerSid $userSid `
            -Label 'Git transaction root'
        $createdPaths.Add((New-BrokerUpgradeOwnedObjectBinding `
            -Path $transactions -Type directory))
        $hooks = New-ManagedDirectory -Path $hooks -Security $readOnlySecurity `
            -OwnerSid $userSid -Label 'Protected empty hooks root'
        $createdPaths.Add((New-BrokerUpgradeOwnedObjectBinding `
            -Path $hooks -Type directory))
        $requests = New-ManagedDirectory -Path $requests -Security $requestSecurity `
            -OwnerSid $userSid -Label 'Broker request root'
        $createdPaths.Add((New-BrokerUpgradeOwnedObjectBinding `
            -Path $requests -Type directory))
        $versionRoot = New-ManagedDirectory -Path $versionRoot `
            -Security $readOnlySecurity -OwnerSid $userSid `
            -Label 'Broker version directory'
        $createdPaths.Add((New-BrokerUpgradeOwnedObjectBinding `
            -Path $versionRoot -Type directory))
        Write-BytesAtomic -Path $workerPath -Bytes $sourceBytes -Security $fileSecurity
        $createdPaths.Add((New-BrokerUpgradeOwnedObjectBinding `
            -Path $workerPath -Type file))
        Assert-ExactSecurity -Path $workerPath -Expected $fileSecurity `
            -ExpectedOwnerSid $userSid -Label 'Installed worker'
        Write-BytesAtomic -Path $workerLockPath -Bytes ([byte[]]::new(0)) `
            -Security $fileSecurity
        $createdPaths.Add((New-BrokerUpgradeOwnedObjectBinding `
            -Path $workerLockPath -Type file))
        Assert-ExactSecurity -Path $workerLockPath -Expected $fileSecurity `
            -ExpectedOwnerSid $userSid -Label 'Worker lock'
        Write-BytesAtomic -Path $transactionLockPath -Bytes ([byte[]]::new(0)) `
            -Security $fileSecurity
        $createdPaths.Add((New-BrokerUpgradeOwnedObjectBinding `
            -Path $transactionLockPath -Type file))
        Assert-ExactSecurity -Path $transactionLockPath -Expected $fileSecurity `
            -ExpectedOwnerSid $userSid -Label 'Git transaction lock'
        $transactionGuard = Open-GitTransactionGuard -Path $transactionLockPath
        Write-BytesAtomic -Path $manifestPath -Bytes $manifestBytes `
            -Security $fileSecurity
        $createdPaths.Add((New-BrokerUpgradeOwnedObjectBinding `
            -Path $manifestPath -Type file))
        Assert-ExactSecurity -Path $manifestPath -Expected $fileSecurity `
            -ExpectedOwnerSid $userSid -Label 'Git capability manifest'
        Write-JsonAtomic -Path $receiptPath -Document $receipt -Security $fileSecurity
        $createdPaths.Add((New-BrokerUpgradeOwnedObjectBinding `
            -Path $receiptPath -Type file))
        Assert-ExactSecurity -Path $receiptPath -Expected $fileSecurity `
            -ExpectedOwnerSid $userSid -Label 'Install receipt'
        $consumedNonce = Write-BrokerUpgradeConsumedNonce `
            -Root $root -Nonce ([string]$SourceAuthority.LauncherNonce) `
            -TransactionId ([string]$SourceAuthority.TransactionId) `
            -GoalId ([string]$SourceAuthority.Manifest.goal_id) `
            -SourceFingerprint ([string]$SourceAuthority.Manifest.source_fingerprint) `
            -FileSecurity $fileSecurity -UserSid $userSid
        $createdPaths.Add((New-BrokerUpgradeOwnedObjectBinding `
            -Path ([string]$consumedNonce.Path) -Type file))
        $heartbeatPath = Join-Path $results $script:HeartbeatName
        Remove-Item -LiteralPath $heartbeatPath -Force -ErrorAction SilentlyContinue
        [void](Register-BrokerTask -Name $Name -Xml $taskXml)
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
            -TimeoutSeconds $script:BrokerHeartbeatStartupTimeoutSeconds `
            -NotBefore $startedAt -TaskName $Name
        $result = [ordered]@{
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
            git_capability_manifest_sha256 = $manifestHash
            heartbeat_executor = [string]$heartbeat.executor_account
            heartbeat_sid = [string]$heartbeat.executor_sid
        }
        $transactionGuard.Dispose()
        $transactionGuard = $null
        Write-JsonAtomic -Path $readyPath -Document $readyDocument `
            -Security $fileSecurity
        $createdPaths.Add((New-BrokerUpgradeOwnedObjectBinding `
            -Path $readyPath -Type file))
        Assert-ExactSecurity -Path $readyPath -Expected $fileSecurity `
            -ExpectedOwnerSid $userSid -Label 'Git capability ready marker'
        $installCommitted = $true
        return $result
    } catch {
        if ($installCommitted) { throw }
        $failure = $_.Exception.Message
        $taskSafeForFileRemoval = $false
        try {
            $taskSafeForFileRemoval = Confirm-BrokerTaskSafeForFileRemoval `
                -TaskRegistered $taskRegistered -Name $Name
        } catch {
            $failure += "; task rollback failed and files were preserved: $($_.Exception.Message)"
        } finally {
            if ($null -ne $transactionGuard) {
                $transactionGuard.Dispose()
                $transactionGuard = $null
            }
        }
        if ($taskSafeForFileRemoval -and $rootCreated -and
            (Test-Path -LiteralPath $root -PathType Container)) {
            try {
                $removeRootAction = {
                    Assert-ExactSecurity -Path $root -Expected $readOnlySecurity `
                        -ExpectedOwnerSid $userSid -Label 'Rollback install root'
                    Wait-BrokerWorkerLockReleased `
                        -Path (Join-Path $root 'worker.lock')
                    Assert-GitTransactionDirectorySafeForInstallerCleanup `
                        -Path $transactions
                    $heartbeatPath = Join-Path $results $script:HeartbeatName
                    $resultEntries = @(Get-ChildItem -LiteralPath $results `
                        -Force -ErrorAction Stop)
                    if ($resultEntries.Count -gt 1 -or
                        ($resultEntries.Count -eq 1 -and
                            [string]$resultEntries[0].FullName -cne
                                $heartbeatPath)) {
                        throw 'Fresh-install rollback found unknown result entries.'
                    }
                    if ($resultEntries.Count -eq 1) {
                        Assert-ExactSecurity -Path $heartbeatPath `
                            -Expected $fileSecurity -ExpectedOwnerSid $userSid `
                            -Label 'Fresh-install rollback heartbeat'
                        $createdPaths.Add((New-BrokerUpgradeOwnedObjectBinding `
                            -Path $heartbeatPath -Type file))
                    }
                    if (@(Get-ChildItem -LiteralPath $requests -Force).Count -ne 0) {
                        throw 'Fresh-install rollback found an unexpected request.'
                    }
                    $expectedRootEntries = @($createdPaths | Where-Object {
                        [IO.Path]::GetDirectoryName([string]$_.path) -ceq $root
                    } | ForEach-Object {
                        [IO.Path]::GetFileName([string]$_.path)
                    } | Sort-Object -Unique)
                    $actualRootEntries = @(Get-ChildItem -LiteralPath $root `
                        -Force | ForEach-Object Name | Sort-Object -Unique)
                    if (($actualRootEntries -join "`n") -cne
                        ($expectedRootEntries -join "`n")) {
                        throw 'Fresh-install rollback root child set drifted.'
                    }
                    Remove-BrokerUpgradeCreatedPaths `
                        -Root $root -TransactionsPath $transactions `
                        -CreatedPaths $createdPaths
                    [void](Remove-BrokerDirectoryExact `
                        -Path $root `
                        -ExpectedIdentity ([string]$rootBinding.identity) `
                        -ExpectedOwnerSid ([string]$rootBinding.owner_sid) `
                        -ExpectedAccessSddl ([string]$rootBinding.access_sddl) `
                        -Label 'Fresh-install rollback root')
                }
                Invoke-AfterBrokerTaskAbsence `
                    -Name $Name -Action $removeRootAction
            } catch { $failure += "; filesystem rollback failed: $($_.Exception.Message)" }
        }
        throw "Broker installation failed; fresh-install rollback was enforced: $failure"
    }
}

function Clear-ExactInterruptedUpgradeStaging {
    param(
        [Parameter(Mandatory = $true)][string]$Root,
        [Parameter(Mandatory = $true)]$LegacyReceipt,
        [Parameter(Mandatory = $true)][string]$UserSid,
        [Parameter(Mandatory = $true)][string]$SandboxSid
    )

    $readyPath = Join-Path $Root $script:GitCapabilityReadyName
    if ($null -ne (Get-Item -LiteralPath $readyPath -Force `
            -ErrorAction SilentlyContinue)) {
        throw 'Interrupted upgrade cleanup refuses a committed Git capability marker.'
    }
    $versionsRoot = Assert-RealDirectory `
        -Path (Join-Path $Root 'versions') -Label 'Broker version root'
    $legacyVersionRoot = Split-Path -Parent ([string]$LegacyReceipt.worker_path)
    if ([string]$LegacyReceipt.worker_sha256 -notmatch '^[0-9a-f]{64}$' -or
        $legacyVersionRoot -cne
            (Join-Path $versionsRoot ([string]$LegacyReceipt.worker_sha256))) {
        throw 'Interrupted upgrade cleanup received an invalid formal schema-v2 worker binding.'
    }
    $allVersionEntries = @(Get-ChildItem -LiteralPath $versionsRoot -Force)
    if (@($allVersionEntries | Where-Object {
                -not $_.PSIsContainer -or
                ($_.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0
            }).Count -ne 0) {
        throw 'Interrupted upgrade cleanup refuses irregular version-root entries.'
    }
    $stagedVersions = @($allVersionEntries | Where-Object {
            $_.FullName -cne $legacyVersionRoot
        })
    $transactions = Join-Path $Root $script:GitTransactionDirectoryName
    $transactionLock = Join-Path $transactions 'transaction.lock'
    $hooks = Join-Path $Root 'empty-hooks'
    $manifestPath = Join-Path $Root $script:GitCapabilityManifestName
    $componentPaths = @($transactions, $transactionLock, $hooks, $manifestPath)
    $presentComponents = @($componentPaths | Where-Object {
            Test-Path -LiteralPath $_
        })
    if ($presentComponents.Count -eq 0 -and $stagedVersions.Count -eq 0) {
        return $false
    }
    if ($presentComponents.Count -ne $componentPaths.Count -or
        $stagedVersions.Count -ne 1 -or
        [string]$stagedVersions[0].Name -notmatch '^[0-9a-f]{64}$') {
        throw "Broker upgrade found incomplete or ambiguous pre-existing v3 staging state. components=$($presentComponents -join ',') staged_versions=$(@($stagedVersions.Name) -join ',')"
    }
    $workerHash = [string]$stagedVersions[0].Name
    $versionRoot = [string]$stagedVersions[0].FullName
    $workerPath = Join-Path $versionRoot 'codex-powershell-broker.ps1'
    $required = @(
        $versionRoot, $workerPath, $transactions, $transactionLock,
        $hooks, $manifestPath
    )

    $readOnlySecurity = New-ManagedDirectorySecurity `
        -UserSid $UserSid -SandboxSid $SandboxSid -Kind ReadOnly
    $fileSecurity = New-ManagedFileSecurity `
        -UserSid $UserSid -SandboxSid $SandboxSid
    foreach ($entry in @(
        @($versionRoot, 'Interrupted upgrade version directory'),
        @($transactions, 'Interrupted upgrade transaction directory'),
        @($hooks, 'Interrupted upgrade hooks directory')
    )) {
        [void](Assert-RealDirectory -Path $entry[0] -Label $entry[1])
        Assert-ExactSecurity -Path $entry[0] -Expected $readOnlySecurity `
            -ExpectedOwnerSid $UserSid -Label $entry[1]
    }
    foreach ($entry in @(
        @($workerPath, 'Interrupted upgrade worker'),
        @($transactionLock, 'Interrupted upgrade transaction lock'),
        @($manifestPath, 'Interrupted upgrade capability manifest')
    )) {
        $item = Get-Item -LiteralPath $entry[0] -Force -ErrorAction Stop
        if ($item.PSIsContainer -or
            ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
            throw "$($entry[1]) must be a regular non-reparse file."
        }
        Assert-ExactSecurity -Path $entry[0] -Expected $fileSecurity `
            -ExpectedOwnerSid $UserSid -Label $entry[1]
    }
    $versionEntries = @(Get-ChildItem -LiteralPath $versionRoot -Force)
    $transactionEntries = @(Get-ChildItem -LiteralPath $transactions -Force)
    if ($versionEntries.Count -ne 1 -or
        [string]$versionEntries[0].Name -cne 'codex-powershell-broker.ps1' -or
        $transactionEntries.Count -ne 1 -or
        [string]$transactionEntries[0].Name -cne 'transaction.lock' -or
        @(Get-ChildItem -LiteralPath $hooks -Force).Count -ne 0 -or
        (Get-FileSha256 -Path $workerPath) -cne $WorkerHash -or
        (Get-Item -LiteralPath $transactionLock -Force).Length -ne 0) {
        throw 'Broker upgrade refuses non-exact pre-existing v3 staging contents.'
    }
    $lockProbe = [IO.FileStream]::new(
        $transactionLock, [IO.FileMode]::Open,
        [IO.FileAccess]::ReadWrite, [IO.FileShare]::None
    )
    $lockProbe.Dispose()
    $manifest = Read-JsonDocument -Path $manifestPath `
        -Label 'Interrupted upgrade capability manifest'
    $stagedInstallId = [Guid]::Empty
    if (-not [Guid]::TryParseExact(
        [string]$manifest.install_id, 'N', [ref]$stagedInstallId
    ) -or [string]$manifest.install_id -ceq
        [string]$LegacyReceipt.install_id) {
        throw 'Interrupted upgrade manifest install_id is invalid or equals the formal v2 install.'
    }
    Assert-GitCapabilityManifestInstalled `
        -Manifest $manifest `
        -Receipt ([pscustomobject]@{ install_id = [string]$manifest.install_id }) `
        -Root $Root

    $ownedStaging = [Collections.Generic.List[object]]::new()
    foreach ($entry in @(
        @($manifestPath, 'file'),
        @($hooks, 'directory'),
        @($transactions, 'directory'),
        @($transactionLock, 'file'),
        @($versionRoot, 'directory'),
        @($workerPath, 'file')
    )) {
        $ownedStaging.Add((New-BrokerUpgradeOwnedObjectBinding `
            -Path $entry[0] -Type $entry[1]))
    }
    Remove-BrokerUpgradeCreatedPaths -Root $Root `
        -TransactionsPath $transactions -CreatedPaths $ownedStaging
    foreach ($path in $required) {
        if (Test-Path -LiteralPath $path) {
            throw "Interrupted upgrade staging cleanup did not persist: $path"
        }
    }
    return $true
}

function Stop-ExactBrokerWorkerForUpgrade {
    param(
        [Parameter(Mandatory = $true)][string]$Name,
        [Parameter(Mandatory = $true)][string]$Root,
        [Parameter(Mandatory = $true)][array]$Receipts,
        [switch]$AllowMissingTask
    )

    [void](Stop-BrokerTaskStrict -Name $Name -AllowMissing:$AllowMissingTask)
    $lockPath = Join-Path $Root 'worker.lock'
    if (Test-BrokerWorkerLockReleased -Path $lockPath) { return }
    $errors = [Collections.Generic.List[string]]::new()
    foreach ($candidate in $Receipts) {
        if ($null -eq $candidate) { continue }
        $binding = $null
        try {
            $binding = Get-ReceiptBoundWorkerProcess `
                -Root $Root -Receipt $candidate
            Stop-ReceiptBoundWorkerProcess `
                -Binding $binding -LockPath $lockPath
            return
        } catch {
            $errors.Add($_.Exception.Message)
        } finally {
            if ($null -ne $binding) { $binding.Process.Dispose() }
        }
    }
    throw "Broker worker lock remained held without an exact receipt-bound upgrade process: $($errors -join '; ')"
}

function New-BrokerUpgradeOwnedObjectBinding {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)]
        [ValidateSet('file', 'directory')]
        [string]$Type
    )
    if ($Type -ceq 'file') {
        $snapshot = Get-BrokerFileSnapshot `
            -Path $Path -Label 'Broker upgrade owned file' -MaximumBytes 4MB
        return [pscustomobject]@{
            path = $snapshot.Path
            type = 'file'
            identity = $snapshot.Identity
            sha256 = $snapshot.Sha256
            owner_sid = $snapshot.OwnerSid
            access_sddl = $snapshot.AccessSddl
        }
    }
    $directory = Assert-RealDirectory `
        -Path $Path -Label 'Broker upgrade owned directory'
    $security = Get-Acl -LiteralPath $directory -ErrorAction Stop
    return [pscustomobject]@{
        path = $directory
        type = 'directory'
        identity = Get-StrongPathIdentity -Path $directory -Directory $true
        sha256 = $null
        owner_sid = [string]$security.GetOwner(
            [Security.Principal.SecurityIdentifier]
        ).Value
        access_sddl = $security.GetSecurityDescriptorSddlForm(
            [Security.AccessControl.AccessControlSections]::Access
        )
    }
}

function New-BrokerUpgradeOwnedFileBindingFromHeldStream {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][IO.FileStream]$Stream,
        [string]$Label = 'Broker upgrade held owned file'
    )

    $fullPath = Get-NormalizedAbsolutePath -Path $Path -Label $Label
    $streamPath = Get-NormalizedAbsolutePath -Path $Stream.Name `
        -Label "$Label stream"
    if (-not $streamPath.Equals(
            $fullPath, [StringComparison]::OrdinalIgnoreCase
        )) {
        throw "$Label stream/path binding drifted."
    }
    $metadata = Get-BrokerHeldFileMetadata `
        -Stream $Stream -Label $Label -MaximumBytes 4MB
    return [pscustomobject]@{
        path = $fullPath
        type = 'file'
        identity = $metadata.Identity
        sha256 = $metadata.Sha256
        owner_sid = $metadata.OwnerSid
        access_sddl = $metadata.AccessSddl
    }
}

function Remove-BrokerUpgradeCreatedPaths {
    param(
        [Parameter(Mandatory = $true)][string]$Root,
        [Parameter(Mandatory = $true)][string]$TransactionsPath,
        [Parameter(Mandatory = $true)][Collections.Generic.List[object]]$CreatedPaths
    )

    if (Test-Path -LiteralPath $TransactionsPath -PathType Container) {
        Assert-GitTransactionDirectorySafeForInstallerCleanup `
            -Path $TransactionsPath
    }
    foreach ($binding in @($CreatedPaths | Sort-Object { $_.path.Length } -Descending)) {
        $verifiedPath = Assert-ChildPath -Child ([string]$binding.path) -Parent $Root `
            -Label 'Upgrade rollback path'
        if (-not (Test-Path -LiteralPath $verifiedPath)) { continue }
        $actualType = if ((Get-Item -LiteralPath $verifiedPath -Force).PSIsContainer) {
            'directory'
        } else { 'file' }
        if ($actualType -cne [string]$binding.type -or
            (Get-StrongPathIdentity -Path $verifiedPath `
                -Directory:($actualType -ceq 'directory')) -cne
                    [string]$binding.identity) {
            throw "Upgrade rollback object identity drifted; preserved path: $verifiedPath"
        }
        if ($actualType -ceq 'file') {
            [void](Remove-BrokerFileExact `
                -Path $verifiedPath `
                -ExpectedSha256 ([string]$binding.sha256) `
                -ExpectedIdentity ([string]$binding.identity) `
                -ExpectedOwnerSid ([string]$binding.owner_sid) `
                -ExpectedAccessSddl ([string]$binding.access_sddl) `
                -MaximumBytes 4MB -Label 'Broker upgrade rollback file')
        } else {
            [void](Remove-BrokerDirectoryExact `
                -Path $verifiedPath `
                -ExpectedIdentity ([string]$binding.identity) `
                -ExpectedOwnerSid ([string]$binding.owner_sid) `
                -ExpectedAccessSddl ([string]$binding.access_sddl) `
                -Label 'Broker upgrade rollback directory')
        }
        if (Test-Path -LiteralPath $verifiedPath) {
            throw "Upgrade rollback path still exists after cleanup: $verifiedPath"
        }
    }
}

function Ensure-BrokerUpgradeResultsRoot {
    param(
        [Parameter(Mandatory = $true)][string]$Root,
        [Parameter(Mandatory = $true)][string]$UserSid,
        [Parameter(Mandatory = $true)][string]$SandboxSid
    )
    $path = Join-Path $Root 'results'
    $security = New-ManagedDirectorySecurity `
        -UserSid $UserSid -SandboxSid $SandboxSid -Kind ReadOnly
    if (Test-Path -LiteralPath $path) {
        [void](Assert-RealDirectory -Path $path -Label 'Broker result root')
        Assert-ExactSecurity -Path $path -Expected $security `
            -ExpectedOwnerSid $UserSid -Label 'Broker result root'
        return $false
    }
    [void](New-ManagedDirectory -Path $path -Security $security `
        -OwnerSid $UserSid -Label 'Recovered broker result root')
    return $true
}

function Resolve-BrokerUpgradeRecoveryDisposition {
    param(
        [Parameter(Mandatory = $true)]
        [ValidateSet('old', 'new', 'unknown')]
        [string]$ReceiptState,
        [Parameter(Mandatory = $true)]
        [ValidateSet('absent', 'valid', 'invalid')]
        [string]$ReadyState
    )
    if ($ReceiptState -ceq 'unknown') { return 'indeterminate_requires_recovery' }
    if ($ReadyState -ceq 'invalid') {
        return 'indeterminate_requires_recovery'
    }
    if ($ReadyState -ceq 'valid') {
        if ($ReceiptState -ceq 'new') { return 'committed_v3' }
        return 'indeterminate_requires_recovery'
    }
    return 'rollback_v2'
}

function Read-BrokerRecoverySelfTestTaskStore {
    param(
        [Parameter(Mandatory = $true)][string]$Root,
        [Parameter(Mandatory = $true)]$FileSecurity,
        [Parameter(Mandatory = $true)][string]$UserSid
    )
    $path = Join-Path $Root 'recovery-selftest-task.json'
    [void](Assert-NoReparseAncestors `
        -Path $path -Label 'Recovery self-test task store' -RequireLeaf)
    Assert-ExactSecurity -Path $path -Expected $FileSecurity `
        -ExpectedOwnerSid $UserSid -Label 'Recovery self-test task store'
    $state = Read-JsonDocument -Path $path `
        -Label 'Recovery self-test task store'
    Assert-ExactProperties -Document $state `
        -Label 'Recovery self-test task store' -Expected @(
            'action_log', 'results_guard_observed', 'running',
            'security_applied', 'task_xml', 'transaction_guard_observed'
        )
    if ($state.action_log -isnot [array] -or
        $state.results_guard_observed -isnot [bool] -or
        $state.running -isnot [bool] -or
        $state.security_applied -isnot [bool] -or
        [string]::IsNullOrWhiteSpace([string]$state.task_xml) -or
        $state.transaction_guard_observed -isnot [bool]) {
        throw 'Recovery self-test task store has an invalid shape.'
    }
    return $state
}

function Write-BrokerRecoverySelfTestTaskStore {
    param(
        [Parameter(Mandatory = $true)][string]$Root,
        [Parameter(Mandatory = $true)]$State,
        [Parameter(Mandatory = $true)]$FileSecurity
    )
    $path = Join-Path $Root 'recovery-selftest-task.json'
    Write-JsonAtomic -Path $path -Document $State `
        -Replace:(Test-Path -LiteralPath $path) -Security $FileSecurity
}

function Test-BrokerRecoverySelfTestResultsGuard {
    param([Parameter(Mandatory = $true)][string]$Root)
    $results = Join-Path $Root 'results'
    $moved = Join-Path $Root 'results.guard-probe'
    $blocked = $false
    try {
        [IO.Directory]::Move($results, $moved)
    } catch [IO.IOException] {
        $blocked = $true
    } catch [UnauthorizedAccessException] {
        $blocked = $true
    }
    if (Test-Path -LiteralPath $moved) {
        [IO.Directory]::Move($moved, $results)
    }
    if (-not $blocked) {
        throw 'Recovery self-test observed an unguarded result root.'
    }
}

function Get-BrokerUpgradeRecoveryTaskXml {
    param(
        [Parameter(Mandatory = $true)][string]$Name,
        [Parameter(Mandatory = $true)][string]$Root,
        [Parameter(Mandatory = $true)]$FileSecurity,
        [Parameter(Mandatory = $true)][string]$UserSid,
        [switch]$UseRecoverySelfTestTaskStore
    )
    if (-not $UseRecoverySelfTestTaskStore) {
        return Get-TaskXmlSnapshot -Name $Name
    }
    $state = Read-BrokerRecoverySelfTestTaskStore `
        -Root $Root -FileSecurity $FileSecurity -UserSid $UserSid
    return [string]$state.task_xml
}

function Assert-BrokerUpgradeRecoveryTaskSecurity {
    param(
        [Parameter(Mandatory = $true)][string]$Name,
        [Parameter(Mandatory = $true)][string]$Root,
        [Parameter(Mandatory = $true)]$FileSecurity,
        [Parameter(Mandatory = $true)][string]$UserSid,
        [Parameter(Mandatory = $true)][string]$SandboxSid,
        [switch]$UseRecoverySelfTestTaskStore
    )
    if (-not $UseRecoverySelfTestTaskStore) {
        Assert-BrokerTaskSecurity -Name $Name `
            -UserSid $UserSid -SandboxSid $SandboxSid
        return
    }
    $state = Read-BrokerRecoverySelfTestTaskStore `
        -Root $Root -FileSecurity $FileSecurity -UserSid $UserSid
    if (-not [bool]$state.security_applied) {
        throw 'Recovery self-test task security is not applied.'
    }
    Test-BrokerRecoverySelfTestResultsGuard -Root $Root
    $state.results_guard_observed = $true
    Write-BrokerRecoverySelfTestTaskStore `
        -Root $Root -State $state -FileSecurity $FileSecurity
}

function Stop-BrokerUpgradeRecoveryWorker {
    param(
        [Parameter(Mandatory = $true)][string]$Name,
        [Parameter(Mandatory = $true)][string]$Root,
        [Parameter(Mandatory = $true)][array]$Receipts,
        [Parameter(Mandatory = $true)]$FileSecurity,
        [Parameter(Mandatory = $true)][string]$UserSid,
        [switch]$UseRecoverySelfTestTaskStore
    )
    if (-not $UseRecoverySelfTestTaskStore) {
        Stop-ExactBrokerWorkerForUpgrade -Name $Name -Root $Root `
            -Receipts $Receipts -AllowMissingTask
        return
    }
    $state = Read-BrokerRecoverySelfTestTaskStore `
        -Root $Root -FileSecurity $FileSecurity -UserSid $UserSid
    $transactions = Join-Path $Root $script:GitTransactionDirectoryName
    if (Test-Path -LiteralPath $transactions -PathType Container) {
        $lockPath = Get-GitTransactionCleanupLockPath -Path $transactions
        $competing = $null
        $blocked = $false
        try {
            $competing = Open-GitTransactionGuard -Path $lockPath
        } catch [IO.IOException] {
            $blocked = $true
        } finally {
            if ($null -ne $competing) { $competing.Dispose() }
        }
        if (-not $blocked) {
            throw 'Recovery self-test observed an unguarded transaction lock.'
        }
        $state.transaction_guard_observed = $true
    }
    $state.running = $false
    $state.action_log = @($state.action_log) + 'stop'
    Remove-Item -LiteralPath (Join-Path (Join-Path $Root 'results') `
        $script:HeartbeatName) -Force -ErrorAction SilentlyContinue
    Write-BrokerRecoverySelfTestTaskStore `
        -Root $Root -State $state -FileSecurity $FileSecurity
}

function Register-BrokerUpgradeRecoveryTask {
    param(
        [Parameter(Mandatory = $true)][string]$Name,
        [Parameter(Mandatory = $true)][string]$Xml,
        [Parameter(Mandatory = $true)][string]$Root,
        [Parameter(Mandatory = $true)]$FileSecurity,
        [Parameter(Mandatory = $true)][string]$UserSid,
        [switch]$UseRecoverySelfTestTaskStore
    )
    if (-not $UseRecoverySelfTestTaskStore) {
        [void](Register-BrokerTask -Name $Name -Xml $Xml)
        return
    }
    $state = Read-BrokerRecoverySelfTestTaskStore `
        -Root $Root -FileSecurity $FileSecurity -UserSid $UserSid
    $state.task_xml = $Xml
    $state.security_applied = $false
    $state.running = $false
    $state.action_log = @($state.action_log) + 'register'
    Write-BrokerRecoverySelfTestTaskStore `
        -Root $Root -State $state -FileSecurity $FileSecurity
}

function Set-BrokerUpgradeRecoveryTaskSecurity {
    param(
        [Parameter(Mandatory = $true)][string]$Name,
        [Parameter(Mandatory = $true)][string]$ExpectedSddl,
        [Parameter(Mandatory = $true)][string]$Root,
        [Parameter(Mandatory = $true)]$FileSecurity,
        [Parameter(Mandatory = $true)][string]$UserSid,
        [Parameter(Mandatory = $true)][string]$SandboxSid,
        [switch]$UseRecoverySelfTestTaskStore
    )
    if (-not $UseRecoverySelfTestTaskStore) {
        Set-And-AssertBrokerTaskSecurity -Name $Name `
            -ExpectedSddl $ExpectedSddl `
            -UserSid $UserSid -SandboxSid $SandboxSid
        return
    }
    $expected = Get-ExpectedTaskSecurityDescriptor `
        -UserSid $UserSid -SandboxSid $SandboxSid
    if ($ExpectedSddl -cne $expected) {
        throw 'Recovery self-test task SDDL drifted.'
    }
    $state = Read-BrokerRecoverySelfTestTaskStore `
        -Root $Root -FileSecurity $FileSecurity -UserSid $UserSid
    $state.security_applied = $true
    $state.action_log = @($state.action_log) + 'set_security'
    Write-BrokerRecoverySelfTestTaskStore `
        -Root $Root -State $state -FileSecurity $FileSecurity
}

function Write-BrokerRecoverySelfTestHeartbeat {
    param(
        [Parameter(Mandatory = $true)][string]$Root,
        [Parameter(Mandatory = $true)]$Receipt,
        [Parameter(Mandatory = $true)]$FileSecurity,
        [switch]$Replace
    )
    $heartbeat = [ordered]@{
        schema_version = [int]$Receipt.schema_version
        install_id = [string]$Receipt.install_id
        executor_account = [string]$Receipt.user_account
        executor_sid = [string]$Receipt.user_sid
        worker_sha256 = [string]$Receipt.worker_sha256
        powershell_sha256 = [string]$Receipt.powershell_sha256
        process_id = $PID
        session_id = [Diagnostics.Process]::GetCurrentProcess().SessionId
        observed_at_utc = [DateTimeOffset]::UtcNow.ToString('o')
    }
    Write-JsonAtomic -Path (Join-Path (Join-Path $Root 'results') `
        $script:HeartbeatName) -Document $heartbeat `
        -Replace:$Replace -Security $FileSecurity
}

function Start-BrokerUpgradeRecoveryTask {
    param(
        [Parameter(Mandatory = $true)][string]$Name,
        [Parameter(Mandatory = $true)][string]$Root,
        [Parameter(Mandatory = $true)]$Receipt,
        [Parameter(Mandatory = $true)]$FileSecurity,
        [Parameter(Mandatory = $true)][string]$UserSid,
        [switch]$UseRecoverySelfTestTaskStore
    )
    if (-not $UseRecoverySelfTestTaskStore) {
        Start-BrokerTask -Name $Name
        return
    }
    $state = Read-BrokerRecoverySelfTestTaskStore `
        -Root $Root -FileSecurity $FileSecurity -UserSid $UserSid
    if (-not [bool]$state.security_applied) {
        throw 'Recovery self-test refused to start an unsecured task.'
    }
    Write-BrokerRecoverySelfTestHeartbeat `
        -Root $Root -Receipt $Receipt -FileSecurity $FileSecurity
    $state.running = $true
    $state.action_log = @($state.action_log) + 'start'
    Write-BrokerRecoverySelfTestTaskStore `
        -Root $Root -State $state -FileSecurity $FileSecurity
}

function Initialize-BrokerRecoverySelfTestFixture {
    param(
        [Parameter(Mandatory = $true)][string]$CaseRoot,
        [Parameter(Mandatory = $true)]
        [ValidateSet(
            'staged', 'old_stopped', 'new_receipt_published',
            'new_task_registered', 'new_task_started',
            'new_heartbeat_verified', 'guard_released',
            'ready_published', 'unknown_receipt',
            'malformed_ready', 'wrong_acl_ready',
            'directory_ready', 'reparse_ready'
        )]
        [string]$Phase,
        [Parameter(Mandatory = $true)][string]$Account,
        [Parameter(Mandatory = $true)][string]$UserSid,
        [Parameter(Mandatory = $true)][string]$Group,
        [Parameter(Mandatory = $true)][string]$SandboxSid,
        [ValidateSet(2, 3)]
        [int]$PreviousSchema = $script:LegacySchemaVersion
    )
    if (Test-Path -LiteralPath $CaseRoot) {
        throw "Recovery self-test case root already exists: $CaseRoot"
    }
    $readOnlySecurity = New-ManagedDirectorySecurity `
        -UserSid $UserSid -SandboxSid $SandboxSid -Kind ReadOnly
    $requestSecurity = New-ManagedDirectorySecurity `
        -UserSid $UserSid -SandboxSid $SandboxSid -Kind Requests
    $fileSecurity = New-ManagedFileSecurity `
        -UserSid $UserSid -SandboxSid $SandboxSid
    [void](New-ManagedDirectory -Path $CaseRoot `
        -Security $readOnlySecurity -OwnerSid $UserSid `
        -Label 'Recovery self-test case root')
    $requests = Join-Path $CaseRoot 'requests'
    [void](New-ManagedDirectory -Path $requests `
        -Security $requestSecurity -OwnerSid $UserSid `
        -Label 'Recovery self-test request root')
    $versions = Join-Path $CaseRoot 'versions'
    [void](New-ManagedDirectory -Path $versions `
        -Security $readOnlySecurity -OwnerSid $UserSid `
        -Label 'Recovery self-test versions')
    $legacyWorkerBytes = [Text.UTF8Encoding]::new($false, $true).GetBytes(
        "legacy recovery worker $Phase"
    )
    $stagedWorkerBytes = [IO.File]::ReadAllBytes($script:BrokerSource)
    $legacyWorkerHash = Get-BytesSha256 -Bytes $legacyWorkerBytes
    $stagedWorkerHash = Get-BytesSha256 -Bytes $stagedWorkerBytes
    $legacyVersion = Join-Path $versions $legacyWorkerHash
    $stagedVersion = Join-Path $versions $stagedWorkerHash
    foreach ($entry in @(
        @($legacyVersion, 'Recovery self-test legacy version'),
        @($stagedVersion, 'Recovery self-test staged version')
    )) {
        [void](New-ManagedDirectory -Path $entry[0] `
            -Security $readOnlySecurity -OwnerSid $UserSid -Label $entry[1])
    }
    $legacyWorker = Join-Path $legacyVersion 'codex-powershell-broker.ps1'
    $stagedWorker = Join-Path $stagedVersion 'codex-powershell-broker.ps1'
    Write-BytesAtomic -Path $legacyWorker `
        -Bytes $legacyWorkerBytes -Security $fileSecurity
    Write-BytesAtomic -Path $stagedWorker `
        -Bytes $stagedWorkerBytes -Security $fileSecurity
    Write-BytesAtomic -Path (Join-Path $CaseRoot 'worker.lock') `
        -Bytes ([byte[]]::new(0)) -Security $fileSecurity

    $transactions = Join-Path $CaseRoot $script:GitTransactionDirectoryName
    $hooks = Join-Path $CaseRoot 'empty-hooks'
    [void](New-ManagedDirectory -Path $transactions `
        -Security $readOnlySecurity -OwnerSid $UserSid `
        -Label 'Recovery self-test transactions')
    Write-BytesAtomic -Path (Join-Path $transactions 'transaction.lock') `
        -Bytes ([byte[]]::new(0)) -Security $fileSecurity
    [void](New-ManagedDirectory -Path $hooks `
        -Security $readOnlySecurity -OwnerSid $UserSid `
        -Label 'Recovery self-test hooks')

    $runtime = Get-CurrentPowerShellRuntime
    $taskName = '\Rayman-Broker-Recovery-SelfTest-' +
        [IO.Path]::GetFileName($CaseRoot)
    $legacyInstallId = [Guid]::NewGuid().ToString('N')
    $stagedInstallId = if ($PreviousSchema -eq $script:SchemaVersion) {
        $legacyInstallId
    } else { [Guid]::NewGuid().ToString('N') }
    $manifest = New-GitCapabilityManifest `
        -InstallId $stagedInstallId -HooksRoot $hooks `
        -TransactionsRoot $transactions
    $manifestPath = Join-Path $CaseRoot $script:GitCapabilityManifestName
    $manifestBytes = [Text.UTF8Encoding]::new($false, $true).GetBytes(
        ($manifest | ConvertTo-Json -Depth 16 -Compress)
    )
    Write-BytesAtomic -Path $manifestPath `
        -Bytes $manifestBytes -Security $fileSecurity
    $manifestHash = Get-BytesSha256 -Bytes $manifestBytes
    $legacyReceipt = [ordered]@{
        schema_version = $PreviousSchema
        install_id = $legacyInstallId
        install_root = $CaseRoot
        installed_at_utc = [DateTimeOffset]::UtcNow.ToString('o')
        task_name = $taskName
        user_account = $Account
        user_sid = $UserSid
        sandbox_group = $Group
        sandbox_group_sid = $SandboxSid
        worker_path = $legacyWorker
        worker_sha256 = $legacyWorkerHash
        powershell_path = $runtime.Path
        powershell_sha256 = $runtime.Sha256
        request_root = $requests
        result_root = Join-Path $CaseRoot 'results'
        capabilities = @('identity_probe')
    }
    if ($PreviousSchema -eq $script:SchemaVersion) {
        $legacyReceipt.capabilities = @(
            'identity_probe', $script:GitCapabilityId
        )
        $legacyReceipt['git_capability_manifest_path'] = $manifestPath
        $legacyReceipt['git_capability_manifest_sha256'] = $manifestHash
    }
    $stagedReceipt = [ordered]@{
        schema_version = $script:SchemaVersion
        install_id = $stagedInstallId
        install_root = $CaseRoot
        installed_at_utc = [DateTimeOffset]::UtcNow.ToString('o')
        task_name = $taskName
        user_account = $Account
        user_sid = $UserSid
        sandbox_group = $Group
        sandbox_group_sid = $SandboxSid
        worker_path = $stagedWorker
        worker_sha256 = $stagedWorkerHash
        powershell_path = $runtime.Path
        powershell_sha256 = $runtime.Sha256
        request_root = $requests
        result_root = Join-Path $CaseRoot 'results'
        capabilities = @('identity_probe', $script:GitCapabilityId)
        git_capability_manifest_path = $manifestPath
        git_capability_manifest_sha256 = $manifestHash
    }
    $legacyReceiptBytes = [Text.UTF8Encoding]::new($false, $true).GetBytes(
        ($legacyReceipt | ConvertTo-Json -Depth 16 -Compress)
    )
    $stagedReceiptBytes = [Text.UTF8Encoding]::new($false, $true).GetBytes(
        ($stagedReceipt | ConvertTo-Json -Depth 16 -Compress)
    )
    $legacyReady = if ($PreviousSchema -eq $script:SchemaVersion) {
        New-GitCapabilityReadyDocument -InstallId $legacyInstallId `
            -WorkerHash $legacyWorkerHash -ManifestHash $manifestHash
    } else { $null }
    $legacyReadyBytes = if ($null -ne $legacyReady) {
        [Text.UTF8Encoding]::new($false, $true).GetBytes(
            ($legacyReady | ConvertTo-Json -Depth 12 -Compress)
        )
    } else { $null }
    $stagedReady = New-GitCapabilityReadyDocument `
        -InstallId $stagedInstallId -WorkerHash $stagedWorkerHash `
        -ManifestHash $manifestHash
    $stagedReadyBytes = [Text.UTF8Encoding]::new($false, $true).GetBytes(
        ($stagedReady | ConvertTo-Json -Depth 12 -Compress)
    )
    $legacyTaskXml = New-BrokerTaskXml `
        -PowerShellPath $runtime.Path -WorkerPath $legacyWorker `
        -BrokerRoot $CaseRoot -Requests $requests `
        -Account $Account -Name $taskName
    $stagedTaskXml = New-BrokerTaskXml `
        -PowerShellPath $runtime.Path -WorkerPath $stagedWorker `
        -BrokerRoot $CaseRoot -Requests $requests `
        -Account $Account -Name $taskName
    $formalReceiptPath = Join-Path $CaseRoot $script:ReceiptName
    Write-BytesAtomic -Path $formalReceiptPath `
        -Bytes $legacyReceiptBytes -Security $fileSecurity
    $legacyReceiptSnapshot = Get-BrokerFileSnapshot `
        -Path $formalReceiptPath -Label 'Recovery self-test legacy receipt' `
        -MaximumBytes 64KB
    $readyPath = Join-Path $CaseRoot $script:GitCapabilityReadyName
    $legacyReadySnapshot = $null
    if ($PreviousSchema -eq $script:SchemaVersion) {
        Write-BytesAtomic -Path $readyPath -Bytes $legacyReadyBytes `
            -Security $fileSecurity
        $legacyReadySnapshot = Get-BrokerFileSnapshot `
            -Path $readyPath -Label 'Recovery self-test legacy ready' `
            -MaximumBytes 64KB
    }
    $allFixtureStagingObjects = @(
        (New-BrokerUpgradeOwnedObjectBinding `
            -Path $stagedVersion -Type directory),
        (New-BrokerUpgradeOwnedObjectBinding `
            -Path $stagedWorker -Type file),
        (New-BrokerUpgradeOwnedObjectBinding `
            -Path $transactions -Type directory),
        (New-BrokerUpgradeOwnedObjectBinding `
            -Path (Join-Path $transactions 'transaction.lock') -Type file),
        (New-BrokerUpgradeOwnedObjectBinding `
            -Path $hooks -Type directory),
        (New-BrokerUpgradeOwnedObjectBinding `
            -Path $manifestPath -Type file)
    )
    $fixtureStagingObjects = if (
        $PreviousSchema -eq $script:SchemaVersion
    ) {
        @($allFixtureStagingObjects | Select-Object -First 2)
    } else { $allFixtureStagingObjects }
    $fixtureJournalObjects = @($fixtureStagingObjects | ForEach-Object {
        [ordered]@{
            path = $_.path
            type = $_.type
            identity = $_.identity
            sha256 = $_.sha256
            owner_sid = $_.owner_sid
            access_sddl = $_.access_sddl
        }
    })
    $fixtureNonce = [Guid]::NewGuid().ToString('N')
    $journalArguments = @{
        Root = $CaseRoot
        Requests = $requests
        Name = $taskName
        Account = $Account
        UserSid = $UserSid
        Group = $Group
        SandboxSid = $SandboxSid
        LegacyReceiptBytes = $legacyReceiptBytes
        LegacyReceiptIdentity = [string]$legacyReceiptSnapshot.Identity
        StagedReceiptBytes = $stagedReceiptBytes
        LegacyTaskXml = $legacyTaskXml
        TransactionId = $(if (
            $PreviousSchema -eq $script:SchemaVersion
        ) { [Guid]::NewGuid().ToString('N') } else { $stagedInstallId })
        InstallationGuardName = $(Get-BrokerInstallationMutexName `
            -Root $CaseRoot)
        LauncherNonce = $fixtureNonce
        GoalId = 'goal_0123456789'
        SourceFingerprint = 'a' * 64
        StagingObjects = $fixtureJournalObjects
    }
    if ($PreviousSchema -eq $script:SchemaVersion) {
        $journalArguments.JournalSchemaVersion =
            $script:CurrentUpgradeRecoveryJournalSchemaVersion
        $journalArguments.LegacyReadyBytes = $legacyReadyBytes
        $journalArguments.LegacyReadyIdentity =
            [string]$legacyReadySnapshot.Identity
        $journalArguments.StagedReadyBytes = $stagedReadyBytes
    }
    $journal = New-BrokerUpgradeRecoveryJournalDocument @journalArguments
    Write-JsonAtomic -Path (Join-Path $CaseRoot `
        $script:UpgradeRecoveryJournalName) `
        -Document $journal -Security $fileSecurity

    $receiptState = if ($Phase -in @('staged', 'old_stopped')) { 'old' }
        elseif ($Phase -ceq 'unknown_receipt') { 'unknown' }
        else { 'new' }
    $formalBytes = if ($receiptState -ceq 'old') { $legacyReceiptBytes }
        elseif ($receiptState -ceq 'new') { $stagedReceiptBytes }
        else { [Text.UTF8Encoding]::new($false, $true).GetBytes('unknown') }
    if (-not [Linq.Enumerable]::SequenceEqual(
            [byte[]]$formalBytes, [byte[]]$legacyReceiptBytes
        )) {
        Write-BytesAtomic -Path $formalReceiptPath `
            -Bytes $formalBytes -Replace -Security $fileSecurity
    }

    $taskIsNew = $Phase -in @(
        'new_task_registered', 'new_task_started', 'new_heartbeat_verified',
        'guard_released', 'ready_published', 'unknown_receipt',
        'malformed_ready', 'wrong_acl_ready',
        'directory_ready', 'reparse_ready'
    )
    $running = $Phase -in @(
        'staged', 'new_task_started', 'new_heartbeat_verified',
        'guard_released', 'ready_published', 'unknown_receipt',
        'malformed_ready', 'wrong_acl_ready',
        'directory_ready', 'reparse_ready'
    )
    Write-BrokerRecoverySelfTestTaskStore -Root $CaseRoot `
        -FileSecurity $fileSecurity -State ([ordered]@{
            task_xml = $(if ($taskIsNew) { $stagedTaskXml } else { $legacyTaskXml })
            security_applied = $true
            running = $running
            action_log = @()
            results_guard_observed = $false
            transaction_guard_observed = $false
        })

    $expectResultsRecreated = $Phase -ceq 'old_stopped'
    if (-not $expectResultsRecreated) {
        [void](New-ManagedDirectory -Path (Join-Path $CaseRoot 'results') `
            -Security $readOnlySecurity -OwnerSid $UserSid `
            -Label 'Recovery self-test results')
        if ($Phase -ceq 'staged') {
            Write-BrokerRecoverySelfTestHeartbeat `
                -Root $CaseRoot -Receipt $legacyReceipt `
                -FileSecurity $fileSecurity
        } elseif ($Phase -in @(
            'new_heartbeat_verified', 'guard_released',
            'ready_published', 'unknown_receipt',
            'malformed_ready', 'wrong_acl_ready',
            'directory_ready', 'reparse_ready'
        )) {
            Write-BrokerRecoverySelfTestHeartbeat `
                -Root $CaseRoot -Receipt $stagedReceipt `
                -FileSecurity $fileSecurity
        }
    }
    if ($Phase -ceq 'ready_published') {
        Write-BytesAtomic -Path $readyPath -Bytes $stagedReadyBytes `
            -Replace:($PreviousSchema -eq $script:SchemaVersion) `
            -Security $fileSecurity
    }
    if ($Phase -ceq 'malformed_ready') {
        Write-BytesAtomic -Path $readyPath `
            -Bytes ([Text.UTF8Encoding]::new($false).GetBytes('{bad')) `
            -Replace:(Test-Path -LiteralPath $readyPath) `
            -Security $fileSecurity
    } elseif ($Phase -ceq 'wrong_acl_ready') {
        Remove-Item -LiteralPath $readyPath -Force `
            -ErrorAction SilentlyContinue
        [IO.File]::WriteAllText(
            $readyPath,
            ((New-GitCapabilityReadyDocument `
                -InstallId $stagedInstallId -WorkerHash $stagedWorkerHash `
                -ManifestHash $manifestHash) | ConvertTo-Json -Compress),
            [Text.UTF8Encoding]::new($false, $true)
        )
    } elseif ($Phase -ceq 'directory_ready') {
        Remove-Item -LiteralPath $readyPath -Force `
            -ErrorAction SilentlyContinue
        [void](New-ManagedDirectory -Path $readyPath `
            -Security $readOnlySecurity -OwnerSid $UserSid `
            -Label 'Recovery self-test directory ready marker')
    } elseif ($Phase -ceq 'reparse_ready') {
        Remove-Item -LiteralPath $readyPath -Force `
            -ErrorAction SilentlyContinue
        $readyTarget = Join-Path $CaseRoot 'ready-reparse-target'
        [void](New-ManagedDirectory -Path $readyTarget `
            -Security $readOnlySecurity -OwnerSid $UserSid `
            -Label 'Recovery self-test ready reparse target')
        [void](New-Item -ItemType Junction -Path $readyPath `
            -Target $readyTarget -ErrorAction Stop)
    }
    $expected = if ($Phase -ceq 'ready_published') { 'committed_v3' }
        elseif ($Phase -in @(
            'unknown_receipt', 'malformed_ready', 'wrong_acl_ready',
            'directory_ready', 'reparse_ready'
        )) {
            'indeterminate_requires_recovery'
        } elseif ($PreviousSchema -eq $script:SchemaVersion) {
            'rolled_back_v3'
        } else { 'rolled_back_v2' }
    Write-JsonAtomic -Path (Join-Path $CaseRoot 'case.json') `
        -Document ([ordered]@{
            phase = $Phase
            expected = $expected
            expect_results_recreated = $expectResultsRecreated
            expect_ready_retained =
                $PreviousSchema -eq $script:SchemaVersion -or
                $Phase -in @(
                    'malformed_ready', 'wrong_acl_ready',
                    'directory_ready', 'reparse_ready'
                )
            previous_schema = $PreviousSchema
            task_name = $taskName
            user_account = $Account
            user_sid = $UserSid
            sandbox_group = $Group
            sandbox_sid = $SandboxSid
        }) -Security $fileSecurity
}

function Get-BrokerSelfTestManagedRoot {
    if (-not ($SelfTest -or $RecoverySelfTestChild -or
            $UpgradeCrashSelfTestChild)) {
        throw 'Broker self-test temp root is unavailable in production mode.'
    }
    $root = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
    return Assert-RealDirectory -Path $root `
        -Label 'Broker self-test process temp root'
}

function Get-BrokerSelfTestCaseRoot {
    param(
        [Parameter(Mandatory = $true)][string]$ManagedRoot,
        [Parameter(Mandatory = $true)]
        [ValidatePattern('^[0-9a-f]{32}$')][string]$Token,
        [Parameter(Mandatory = $true)][string]$Label
    )
    $caseRoot = Assert-ChildPath `
        -Child (Join-Path $ManagedRoot ('b-' + $Token)) `
        -Parent $ManagedRoot -Label $Label
    $deepestWorker = Join-Path $caseRoot (
        'versions\' + ('f' * 64) + '\codex-powershell-broker.ps1'
    )
    if ($deepestWorker.Length -gt 240) {
        throw "Broker self-test case path budget is unsafe for native held opens: length=$($deepestWorker.Length) path=$deepestWorker"
    }
    return $caseRoot
}

function Invoke-BrokerUpgradeCrashSelfTestChild {
    param(
        [Parameter(Mandatory = $true)][string]$Token,
        [Parameter(Mandatory = $true)][string]$Phase
    )
    $managedRoot = Get-BrokerSelfTestManagedRoot
    $caseRoot = Get-BrokerSelfTestCaseRoot `
        -ManagedRoot $managedRoot -Token $Token `
        -Label 'Upgrade crash child case root'
    [void](Assert-RealDirectory -Path $caseRoot `
        -Label 'Upgrade crash child case root')
    $case = Read-JsonDocument -Path (Join-Path $caseRoot 'case.json') `
        -Label 'Upgrade crash child case'
    $fileSecurity = New-ManagedFileSecurity `
        -UserSid ([string]$case.user_sid) `
        -SandboxSid ([string]$case.sandbox_sid)
    $recovery = Read-BrokerUpgradeRecoveryJournal `
        -Root $caseRoot -Requests (Join-Path $caseRoot 'requests') `
        -Name ([string]$case.task_name) `
        -Account ([string]$case.user_account) `
        -UserSid ([string]$case.user_sid) `
        -Group ([string]$case.sandbox_group) `
        -SandboxSid ([string]$case.sandbox_sid) `
        -ExpectedFileSecurity $fileSecurity
    $crashTarget = [pscustomobject]@{ Value = $Phase }
    $crashIf = {
        param([string]$CompletedPhase)
        if ([string]$crashTarget.Value -ceq $CompletedPhase) {
            [Diagnostics.Process]::GetCurrentProcess().Kill($true)
            [Threading.Thread]::Sleep([Threading.Timeout]::Infinite)
        }
    }
    if ($Phase -ceq 'staged') { & $crashIf 'staged' }
    $transactionPath = Join-Path $caseRoot `
        $script:GitTransactionDirectoryName
    $transactionGuard = Open-GitTransactionGuard `
        -Path (Join-Path $transactionPath 'transaction.lock')
    $guardState = [pscustomobject]@{ Stream = $transactionGuard }
    $stopOldAction = {
        Stop-BrokerUpgradeRecoveryWorker `
            -Name ([string]$case.task_name) -Root $caseRoot `
            -Receipts @($recovery.LegacyReceipt) `
            -FileSecurity $fileSecurity -UserSid ([string]$case.user_sid) `
            -UseRecoverySelfTestTaskStore
        & $crashIf 'old_stopped'
    }
    $publishNewAction = {
        Remove-Item -LiteralPath (Join-Path (Join-Path $caseRoot 'results') `
            $script:HeartbeatName) -Force -ErrorAction SilentlyContinue
        [void](Publish-BrokerReceiptForward `
            -Path (Join-Path $caseRoot $script:ReceiptName) `
            -OldBytes $recovery.LegacyReceiptBytes `
            -NewBytes $recovery.StagedReceiptBytes `
            -Security $fileSecurity -OwnerSid ([string]$case.user_sid))
        & $crashIf 'new_receipt_published'
    }
    $registerNewAction = {
        $xml = New-BrokerTaskXml `
            -PowerShellPath ([string]$recovery.StagedReceipt.powershell_path) `
            -WorkerPath ([string]$recovery.StagedReceipt.worker_path) `
            -BrokerRoot $caseRoot -Requests (Join-Path $caseRoot 'requests') `
            -Account ([string]$case.user_account) `
            -Name ([string]$case.task_name)
        Register-BrokerUpgradeRecoveryTask `
            -Name ([string]$case.task_name) -Xml $xml -Root $caseRoot `
            -FileSecurity $fileSecurity -UserSid ([string]$case.user_sid) `
            -UseRecoverySelfTestTaskStore
        Set-BrokerUpgradeRecoveryTaskSecurity `
            -Name ([string]$case.task_name) `
            -ExpectedSddl (Get-ExpectedTaskSecurityDescriptor `
                -UserSid ([string]$case.user_sid) `
                -SandboxSid ([string]$case.sandbox_sid)) `
            -Root $caseRoot -FileSecurity $fileSecurity `
            -UserSid ([string]$case.user_sid) `
            -SandboxSid ([string]$case.sandbox_sid) `
            -UseRecoverySelfTestTaskStore
        & $crashIf 'new_task_registered'
    }
    $startNewAction = {
        Start-BrokerUpgradeRecoveryTask `
            -Name ([string]$case.task_name) -Root $caseRoot `
            -Receipt $recovery.StagedReceipt -FileSecurity $fileSecurity `
            -UserSid ([string]$case.user_sid) `
            -UseRecoverySelfTestTaskStore
        & $crashIf 'new_task_started'
    }
    $waitNewAction = {
        $heartbeat = Wait-BrokerHeartbeat `
            -Root $caseRoot -Receipt $recovery.StagedReceipt `
            -TimeoutSeconds 2
        & $crashIf 'new_heartbeat_verified'
        return $heartbeat
    }
    $releaseGuardAction = {
        if ($null -ne $guardState.Stream) {
            $guardState.Stream.Dispose()
            $guardState.Stream = $null
        }
        & $crashIf 'guard_released'
    }
    $commitAction = {
        param($Heartbeat)
        $readyPath = Join-Path $caseRoot $script:GitCapabilityReadyName
        if ([int]$recovery.LegacySchemaVersion -eq
                $script:SchemaVersion) {
            [void](Publish-BrokerReadyForward -Path $readyPath `
                -OldBytes $recovery.LegacyReadyBytes `
                -NewBytes $recovery.StagedReadyBytes `
                -Security $fileSecurity `
                -OwnerSid ([string]$case.user_sid))
        } else {
            $ready = New-GitCapabilityReadyDocument `
                -InstallId ([string]$recovery.StagedReceipt.install_id) `
                -WorkerHash ([string]$recovery.StagedReceipt.worker_sha256) `
                -ManifestHash ([string]$recovery.StagedReceipt.git_capability_manifest_sha256)
            Write-JsonAtomic -Path $readyPath -Document $ready `
                -Security $fileSecurity
        }
        & $crashIf 'ready_published'
        return [pscustomobject]@{ committed = $true }
    }
    $unexpectedRollback = { throw 'Crash child unexpectedly entered rollback.' }
    try {
        [void](Invoke-BrokerUpgradeSwitchTransaction `
            -StopOldAction $stopOldAction `
            -PublishNewReceiptAction $publishNewAction `
            -RegisterNewTaskAction $registerNewAction `
            -StartNewTaskAction $startNewAction `
            -WaitNewHeartbeatAction $waitNewAction `
            -CommitAction $commitAction `
            -ReleaseGuardAction $releaseGuardAction `
            -StopRollbackAction $unexpectedRollback `
            -RestoreOldReceiptAction $unexpectedRollback `
            -RegisterOldTaskAction $unexpectedRollback `
            -StartOldTaskAction $unexpectedRollback `
            -WaitOldHeartbeatAction $unexpectedRollback `
            -CleanupStagingAction $unexpectedRollback)
        throw 'Upgrade crash child completed without the requested process crash.'
    } finally {
        if ($null -ne $guardState.Stream) { $guardState.Stream.Dispose() }
    }
}

function Invoke-BrokerRecoverySelfTestChild {
    param([Parameter(Mandatory = $true)][string]$Token)
    $managedRoot = Get-BrokerSelfTestManagedRoot
    $caseRoot = Get-BrokerSelfTestCaseRoot `
        -ManagedRoot $managedRoot -Token $Token `
        -Label 'Recovery child case root'
    [void](Assert-RealDirectory -Path $caseRoot -Label 'Recovery child case root')
    $case = Read-JsonDocument -Path (Join-Path $caseRoot 'case.json') `
        -Label 'Recovery child case'
    Assert-ExactProperties -Document $case -Label 'Recovery child case' `
        -Expected @(
            'expected', 'expect_results_recreated', 'phase', 'sandbox_group',
            'expect_ready_retained', 'sandbox_sid', 'task_name',
            'user_account', 'user_sid', 'previous_schema'
        )
    $fileSecurity = New-ManagedFileSecurity `
        -UserSid ([string]$case.user_sid) `
        -SandboxSid ([string]$case.sandbox_sid)
    $journalBefore = Read-BrokerUpgradeRecoveryJournal `
        -Root $caseRoot -Requests (Join-Path $caseRoot 'requests') `
        -Name ([string]$case.task_name) `
        -Account ([string]$case.user_account) `
        -UserSid ([string]$case.user_sid) `
        -Group ([string]$case.sandbox_group) `
        -SandboxSid ([string]$case.sandbox_sid) `
        -ExpectedFileSecurity $fileSecurity
    $receiptPath = Join-Path $caseRoot $script:ReceiptName
    $receiptBytesBefore = [IO.File]::ReadAllBytes($receiptPath)
    $receiptIdentityBefore = Get-StrongPathIdentity `
        -Path $receiptPath -Directory $false
    $taskBefore = Read-BrokerRecoverySelfTestTaskStore `
        -Root $caseRoot -FileSecurity $fileSecurity `
        -UserSid ([string]$case.user_sid)
    $result = $null
    $failure = $null
    try {
        $recoveryOutput = @(Invoke-BrokerInterruptedUpgradeRecovery `
            -Root $caseRoot -Requests (Join-Path $caseRoot 'requests') `
            -Name ([string]$case.task_name) `
            -Account ([string]$case.user_account) `
            -UserSid ([string]$case.user_sid) `
            -Group ([string]$case.sandbox_group) `
            -SandboxSid ([string]$case.sandbox_sid) `
            -FileSecurity $fileSecurity -UseRecoverySelfTestTaskStore)
        if ($recoveryOutput.Count -ne 1) {
            $types = @($recoveryOutput | ForEach-Object {
                if ($null -eq $_) { '<null>' } else { $_.GetType().FullName }
            }) -join ', '
            throw "Recovery child expected one result; count=$($recoveryOutput.Count) types=$types"
        }
        $result = $recoveryOutput[0]
    } catch {
        $failure = $_.Exception.Message
    }
    $expected = [string]$case.expected
    if ($expected -ceq 'indeterminate_requires_recovery') {
        if ($null -eq $failure -or
            -not $failure.StartsWith(
                'indeterminate_requires_recovery:',
                [StringComparison]::Ordinal
            )) {
            throw "Recovery child expected indeterminate failure: phase=$($case.phase) actual=$failure"
        }
        [void](Read-BrokerUpgradeRecoveryJournal `
            -Root $caseRoot -Requests (Join-Path $caseRoot 'requests') `
            -Name ([string]$case.task_name) `
            -Account ([string]$case.user_account) `
            -UserSid ([string]$case.user_sid) `
            -Group ([string]$case.sandbox_group) `
            -SandboxSid ([string]$case.sandbox_sid) `
            -ExpectedFileSecurity $fileSecurity)
        if ((Get-BytesSha256 -Bytes $receiptBytesBefore) -cne
                (Get-FileSha256 -Path $receiptPath) -or
            (Get-StrongPathIdentity -Path $receiptPath -Directory $false) -cne
                $receiptIdentityBefore) {
            throw 'Indeterminate recovery changed the formal receipt.'
        }
    } else {
        $resultMode = if ($null -ne $result -and
            $null -ne $result.PSObject.Properties['Mode']) {
            [string]$result.Mode
        } else { '<missing>' }
        if ($null -ne $failure -or $null -eq $result -or
            $resultMode -cne $expected) {
            $resultType = if ($null -eq $result) { '<null>' }
                else { $result.GetType().FullName }
            throw "Recovery child mode mismatch: phase=$($case.phase) actual=$resultMode type=$resultType failure=$failure expected=$expected"
        }
        if (Test-Path -LiteralPath $journalBefore.Path) {
            throw 'Completed recovery retained its durable journal.'
        }
    }
    $taskAfter = Read-BrokerRecoverySelfTestTaskStore `
        -Root $caseRoot -FileSecurity $fileSecurity `
        -UserSid ([string]$case.user_sid)
    if ($expected -ceq 'indeterminate_requires_recovery') {
        if ([bool]$taskAfter.results_guard_observed) {
            throw 'Indeterminate recovery opened the result-root mutation guard.'
        }
    } elseif (-not [bool]$taskAfter.results_guard_observed) {
        throw 'Recovery child did not prove the result-root guard.'
    }
    $readyPath = Join-Path $caseRoot $script:GitCapabilityReadyName
    $stagedVersion = Split-Path -Parent `
        ([string]$journalBefore.StagedReceipt.worker_path)
    $previousSchema = [int]$case.previous_schema
    $persistentPaths = @(
        (Join-Path $caseRoot $script:GitTransactionDirectoryName),
        (Join-Path $caseRoot 'empty-hooks'),
        (Join-Path $caseRoot $script:GitCapabilityManifestName)
    )
    $stagingPaths = if ($previousSchema -eq $script:SchemaVersion) {
        @($stagedVersion)
    } else { @($stagedVersion) + $persistentPaths }
    if ($expected -ceq 'rolled_back_v2') {
        $rollbackTail = @($taskAfter.action_log | Select-Object -Last 4) -join ','
        if ((Get-BytesSha256 -Bytes $journalBefore.LegacyReceiptBytes) -cne
                (Get-FileSha256 -Path $receiptPath) -or
            [string]$taskAfter.task_xml -cne
                [string]$journalBefore.LegacyTaskXml -or
            -not [bool]$taskAfter.security_applied -or
            -not [bool]$taskAfter.running -or
            $rollbackTail -cne 'stop,register,set_security,start' -or
            -not [bool]$taskAfter.transaction_guard_observed -or
            ([bool](Test-Path -LiteralPath $readyPath) -ne
                [bool]$case.expect_ready_retained)) {
            throw 'Recovery child did not prove the exact v2 rollback post-state.'
        }
        foreach ($path in $stagingPaths) {
            if (Test-Path -LiteralPath $path) {
                throw "Recovery child retained rollback staging: $path"
            }
        }
        [void](Wait-BrokerHeartbeat -Root $caseRoot `
            -Receipt $journalBefore.LegacyReceipt -TimeoutSeconds 1)
        if ((Get-BytesSha256 -Bytes $receiptBytesBefore) -ceq
                [string]$journalBefore.Journal.legacy_receipt_sha256 -and
            (Get-StrongPathIdentity -Path $receiptPath -Directory $false) -cne
                $receiptIdentityBefore) {
            throw 'Recovery child rewrote an already-old receipt.'
        }
    } elseif ($expected -ceq 'rolled_back_v3') {
        $rollbackTail = @($taskAfter.action_log | Select-Object -Last 4) -join ','
        if ((Get-BytesSha256 -Bytes $journalBefore.LegacyReceiptBytes) -cne
                (Get-FileSha256 -Path $receiptPath) -or
            [string]$taskAfter.task_xml -cne
                [string]$journalBefore.LegacyTaskXml -or
            -not [bool]$taskAfter.security_applied -or
            -not [bool]$taskAfter.running -or
            $rollbackTail -cne 'stop,register,set_security,start' -or
            -not [bool]$taskAfter.transaction_guard_observed) {
            throw 'Recovery child did not prove the exact v3 rollback post-state.'
        }
        [void](Assert-GitCapabilityReadyInstalled `
            -Receipt $journalBefore.LegacyReceipt -Root $caseRoot)
        foreach ($path in $stagingPaths) {
            if (Test-Path -LiteralPath $path) {
                throw "Current-schema recovery retained rollback staging: $path"
            }
        }
        foreach ($path in $persistentPaths) {
            if (-not (Test-Path -LiteralPath $path)) {
                throw "Current-schema recovery lost persistent state: $path"
            }
        }
        [void](Wait-BrokerHeartbeat -Root $caseRoot `
            -Receipt $journalBefore.LegacyReceipt -TimeoutSeconds 1)
        if ((Get-BytesSha256 -Bytes $receiptBytesBefore) -ceq
                [string]$journalBefore.Journal.legacy_receipt_sha256 -and
            (Get-StrongPathIdentity -Path $receiptPath -Directory $false) -cne
                $receiptIdentityBefore) {
            throw 'Current-schema recovery rewrote an already-old receipt.'
        }
    } elseif ($expected -ceq 'committed_v3') {
        if ((Get-BytesSha256 -Bytes $journalBefore.StagedReceiptBytes) -cne
                (Get-FileSha256 -Path $receiptPath) -or
            (@($taskAfter.action_log) -join ',') -cne
                (@($taskBefore.action_log) -join ',') -or
            -not [bool]$taskAfter.running -or
            -not [bool]$taskAfter.security_applied) {
            throw 'Recovery child mutated an already-committed v3 task or receipt.'
        }
        foreach ($path in $stagingPaths) {
            if (-not (Test-Path -LiteralPath $path)) {
                throw "Committed recovery lost installed state: $path"
            }
        }
        foreach ($path in $persistentPaths) {
            if (-not (Test-Path -LiteralPath $path)) {
                throw "Committed recovery lost persistent state: $path"
            }
        }
        [void](Assert-GitCapabilityReadyInstalled `
            -Receipt $journalBefore.StagedReceipt -Root $caseRoot)
        [void](Wait-BrokerHeartbeat -Root $caseRoot `
            -Receipt $journalBefore.StagedReceipt -TimeoutSeconds 1)
    } else {
        if ((@($taskAfter.action_log).Count -ne 0) -or
            [string]$taskAfter.task_xml -cne [string]$taskBefore.task_xml -or
            [bool]$taskAfter.running -ne [bool]$taskBefore.running -or
            [bool]$taskAfter.security_applied -ne
                [bool]$taskBefore.security_applied -or
            ([bool](Test-Path -LiteralPath $readyPath) -ne
                [bool]$case.expect_ready_retained)) {
            throw 'Indeterminate recovery mutated task or ready state.'
        }
        foreach ($path in $stagingPaths) {
            if (-not (Test-Path -LiteralPath $path)) {
                throw "Indeterminate recovery removed evidence: $path"
            }
        }
        foreach ($path in $persistentPaths) {
            if (-not (Test-Path -LiteralPath $path)) {
                throw "Indeterminate recovery removed persistent evidence: $path"
            }
        }
    }
    $actualResultsRecreated = if ($null -ne $result) {
        [bool]$result.RecreatedResultsRoot
    } else { $false }
    if ([bool]$case.expect_results_recreated -ne $actualResultsRecreated) {
        throw 'Recovery child result-root reconstruction report drifted.'
    }
    $results = Join-Path $caseRoot 'results'
    $roundTrip = Join-Path $caseRoot 'results.guard-released'
    [IO.Directory]::Move($results, $roundTrip)
    [IO.Directory]::Move($roundTrip, $results)
    $runtime = Get-CurrentPowerShellRuntime
    [ordered]@{
        phase = [string]$case.phase
        mode = $expected
        results_recreated = [bool]$case.expect_results_recreated
        powershell_path = $runtime.Path
        powershell_sha256 = $runtime.Sha256
    } | ConvertTo-Json -Compress
}

function Invoke-BrokerInterruptedCurrentSchemaUpgradeRecovery {
    param(
        [Parameter(Mandatory = $true)]$Recovery,
        [Parameter(Mandatory = $true)][string]$Root,
        [Parameter(Mandatory = $true)][string]$Requests,
        [Parameter(Mandatory = $true)][string]$Name,
        [Parameter(Mandatory = $true)][string]$Account,
        [Parameter(Mandatory = $true)][string]$UserSid,
        [Parameter(Mandatory = $true)][string]$Group,
        [Parameter(Mandatory = $true)][string]$SandboxSid,
        [Parameter(Mandatory = $true)]$FileSecurity,
        [switch]$UseRecoverySelfTestTaskStore
    )
    if ([int]$Recovery.SchemaVersion -ne
            $script:CurrentUpgradeRecoveryJournalSchemaVersion -or
        [int]$Recovery.LegacySchemaVersion -ne $script:SchemaVersion) {
        throw 'Current-schema recovery received a non-current upgrade journal.'
    }
    $recreatedResultsRoot = $false
    $resultsGuard = $null
    $transactionGuard = $null
    try {
        $receiptPath = Join-Path $Root $script:ReceiptName
        $receiptState = Get-BrokerReceiptFileState -Path $receiptPath `
            -OldSha256 ([string]$Recovery.Journal.legacy_receipt_sha256) `
            -NewSha256 ([string]$Recovery.Journal.staged_receipt_sha256) `
            -Security $FileSecurity -OwnerSid $UserSid
        if ([string]$receiptState.State -ceq 'old' -and
            [string]$receiptState.Identity -cne
                [string]$Recovery.LegacyReceiptIdentity) {
            throw 'indeterminate_requires_recovery: previous v3 receipt identity drifted from the durable journal.'
        }
        if ([string]$receiptState.State -ceq 'unknown') {
            throw 'indeterminate_requires_recovery: current-schema receipt bytes are outside the journaled pair.'
        }

        $taskXml = Get-BrokerUpgradeRecoveryTaskXml `
            -Name $Name -Root $Root -FileSecurity $FileSecurity `
            -UserSid $UserSid `
            -UseRecoverySelfTestTaskStore:$UseRecoverySelfTestTaskStore
        $taskState = if ($null -eq $taskXml) { 'absent' } else { 'unknown' }
        if ($null -ne $taskXml) {
            foreach ($candidate in @(
                @('old', $Recovery.LegacyReceipt),
                @('new', $Recovery.StagedReceipt)
            )) {
                try {
                    [void](Assert-BrokerTaskXmlBinding `
                        -Xml ([string]$taskXml) `
                        -PowerShellPath ([string]$candidate[1].powershell_path) `
                        -WorkerPath ([string]$candidate[1].worker_path) `
                        -BrokerRoot $Root -Requests $Requests `
                        -Account $Account -UserSid $UserSid)
                    $taskState = [string]$candidate[0]
                    break
                } catch { }
            }
            if ($taskState -ceq 'unknown') {
                throw 'indeterminate_requires_recovery: current-schema task is outside the journaled pair.'
            }
        }

        $readyPath = Join-Path $Root $script:GitCapabilityReadyName
        $readyState = Get-BrokerUpgradeReadyFileState -Path $readyPath `
            -OldSha256 ([string]$Recovery.Journal.legacy_ready_sha256) `
            -NewSha256 ([string]$Recovery.Journal.staged_ready_sha256) `
            -Security $FileSecurity -OwnerSid $UserSid
        if ([string]$readyState.State -ceq 'old' -and
            [string]$readyState.Identity -cne
                [string]$Recovery.LegacyReadyIdentity) {
            throw 'indeterminate_requires_recovery: previous v3 ready identity drifted from the durable journal.'
        }
        if ([string]$readyState.State -ceq 'unknown') {
            throw 'indeterminate_requires_recovery: current-schema ready bytes are outside the journaled pair.'
        }
        [void](Ensure-BrokerUpgradeConsumedNonceForRecovery `
            -Recovery $Recovery -Root $Root `
            -FileSecurity $FileSecurity -UserSid $UserSid)
        $recreatedResultsRoot = Ensure-BrokerUpgradeResultsRoot `
            -Root $Root -UserSid $UserSid -SandboxSid $SandboxSid
        $resultsGuard = Open-InstallerDirectoryGuard `
            -Path (Join-Path $Root 'results') `
            -Label 'Current-schema result root recovery guard'
        if ($null -ne $taskXml) {
            Assert-BrokerUpgradeRecoveryTaskSecurity `
                -Name $Name -Root $Root -FileSecurity $FileSecurity `
                -UserSid $UserSid -SandboxSid $SandboxSid `
                -UseRecoverySelfTestTaskStore:$UseRecoverySelfTestTaskStore
        }
        if ([string]$readyState.State -ceq 'new') {
            if ([string]$receiptState.State -cne 'new' -or
                $taskState -cne 'new') {
                throw "indeterminate_requires_recovery: committed ready has receipt=$($receiptState.State) task=$taskState."
            }
            $committed = Read-Receipt -Root $Root -Requests $Requests `
                -Name $Name -SkipTaskValidation -SkipGitLiveValidation
            [void](Assert-GitCapabilityReadyInstalled `
                -Receipt $committed -Root $Root)
            $heartbeat = Wait-BrokerHeartbeat -Root $Root `
                -Receipt $committed `
                -TimeoutSeconds $script:BrokerHeartbeatStartupTimeoutSeconds `
                -TaskName $Name
            Remove-BrokerUpgradeRecoveryJournal -Recovery $Recovery
            return [pscustomobject]@{
                Mode = 'committed_v3'
                PreviousSchemaVersion = $script:SchemaVersion
                Receipt = $committed
                Heartbeat = $heartbeat
                RecreatedResultsRoot = $recreatedResultsRoot
            }
        }

        $transactions = Join-Path $Root $script:GitTransactionDirectoryName
        $transactionLock = Get-GitTransactionCleanupLockPath `
            -Path $transactions
        $transactionGuard = Open-GitTransactionGuard -Path $transactionLock
        if ((Get-GitTransactionCleanupLockPath -Path $transactions) -cne
            $transactionLock) {
            throw 'indeterminate_requires_recovery: transaction lock changed during current-schema recovery.'
        }
        Stop-BrokerUpgradeRecoveryWorker -Name $Name -Root $Root `
            -Receipts @($Recovery.StagedReceipt, $Recovery.LegacyReceipt) `
            -FileSecurity $FileSecurity -UserSid $UserSid `
            -UseRecoverySelfTestTaskStore:$UseRecoverySelfTestTaskStore
        [void](Restore-BrokerReceiptFromJournal -Path $receiptPath `
            -OldBytes $Recovery.LegacyReceiptBytes `
            -NewBytes $Recovery.StagedReceiptBytes `
            -Security $FileSecurity -OwnerSid $UserSid)
        [void](Restore-BrokerReadyFromJournal -Path $readyPath `
            -OldBytes $Recovery.LegacyReadyBytes `
            -NewBytes $Recovery.StagedReadyBytes `
            -Security $FileSecurity -OwnerSid $UserSid)
        Register-BrokerUpgradeRecoveryTask `
            -Name $Name -Xml $Recovery.LegacyTaskXml -Root $Root `
            -FileSecurity $FileSecurity -UserSid $UserSid `
            -UseRecoverySelfTestTaskStore:$UseRecoverySelfTestTaskStore
        $taskSecurity = Get-ExpectedTaskSecurityDescriptor `
            -UserSid $UserSid -SandboxSid $SandboxSid
        Set-BrokerUpgradeRecoveryTaskSecurity `
            -Name $Name -ExpectedSddl $taskSecurity -Root $Root `
            -FileSecurity $FileSecurity -UserSid $UserSid `
            -SandboxSid $SandboxSid `
            -UseRecoverySelfTestTaskStore:$UseRecoverySelfTestTaskStore
        Remove-Item -LiteralPath (Join-Path (Join-Path $Root 'results') `
            $script:HeartbeatName) -Force -ErrorAction SilentlyContinue
        $restoredAt = [DateTimeOffset]::UtcNow
        Start-BrokerUpgradeRecoveryTask `
            -Name $Name -Root $Root -Receipt $Recovery.LegacyReceipt `
            -FileSecurity $FileSecurity -UserSid $UserSid `
            -UseRecoverySelfTestTaskStore:$UseRecoverySelfTestTaskStore
        $heartbeat = Wait-BrokerHeartbeat -Root $Root `
            -Receipt $Recovery.LegacyReceipt `
            -TimeoutSeconds $script:BrokerHeartbeatStartupTimeoutSeconds `
            -NotBefore $restoredAt -TaskName $Name
        $transactionGuard.Dispose()
        $transactionGuard = $null
        $createdPaths = [Collections.Generic.List[object]]::new()
        foreach ($binding in @($Recovery.StagingObjects)) {
            $createdPaths.Add($binding)
        }
        Remove-BrokerUpgradeCreatedPaths -Root $Root `
            -TransactionsPath $transactions -CreatedPaths $createdPaths
        Remove-BrokerUpgradeRecoveryJournal -Recovery $Recovery
        return [pscustomobject]@{
            Mode = 'rolled_back_v3'
            PreviousSchemaVersion = $script:SchemaVersion
            Receipt = $Recovery.LegacyReceipt
            Heartbeat = $heartbeat
            RecreatedResultsRoot = $recreatedResultsRoot
        }
    } finally {
        if ($null -ne $transactionGuard) { $transactionGuard.Dispose() }
        if ($null -ne $resultsGuard) { $resultsGuard.Dispose() }
    }
}

function Invoke-BrokerInterruptedUpgradeRecovery {
    param(
        [Parameter(Mandatory = $true)][string]$Root,
        [Parameter(Mandatory = $true)][string]$Requests,
        [Parameter(Mandatory = $true)][string]$Name,
        [Parameter(Mandatory = $true)][string]$Account,
        [Parameter(Mandatory = $true)][string]$UserSid,
        [Parameter(Mandatory = $true)][string]$Group,
        [Parameter(Mandatory = $true)][string]$SandboxSid,
        [Parameter(Mandatory = $true)]$FileSecurity,
        [switch]$UseRecoverySelfTestTaskStore
    )
    if ($UseRecoverySelfTestTaskStore -and -not $RecoverySelfTestChild) {
        throw 'Recovery self-test task store is available only to the fixed child parameter set.'
    }
    $recovery = Read-BrokerUpgradeRecoveryJournal `
        -Root $Root -Requests $Requests -Name $Name -Account $Account `
        -UserSid $UserSid -Group $Group -SandboxSid $SandboxSid `
        -ExpectedFileSecurity $FileSecurity
    if ($null -eq $recovery) { return $null }
    if ([int]$recovery.LegacySchemaVersion -eq $script:SchemaVersion) {
        return Invoke-BrokerInterruptedCurrentSchemaUpgradeRecovery `
            -Recovery $recovery -Root $Root -Requests $Requests `
            -Name $Name -Account $Account -UserSid $UserSid `
            -Group $Group -SandboxSid $SandboxSid `
            -FileSecurity $FileSecurity `
            -UseRecoverySelfTestTaskStore:$UseRecoverySelfTestTaskStore
    }
    $preflightReceipt = Get-BrokerReceiptFileState `
        -Path (Join-Path $Root $script:ReceiptName) `
        -OldSha256 ([string]$recovery.Journal.legacy_receipt_sha256) `
        -NewSha256 ([string]$recovery.Journal.staged_receipt_sha256) `
        -Security $FileSecurity -OwnerSid $UserSid
    if ([int]$recovery.SchemaVersion -eq
            $script:UpgradeRecoveryJournalSchemaVersion -and
        [string]$preflightReceipt.State -ceq 'old' -and
        [string]$preflightReceipt.Identity -cne
            [string]$recovery.LegacyReceiptIdentity) {
        throw 'indeterminate_requires_recovery: legacy receipt identity drifted from the durable journal.'
    }
    $preflightTaskXml = Get-BrokerUpgradeRecoveryTaskXml `
        -Name $Name -Root $Root -FileSecurity $FileSecurity `
        -UserSid $UserSid `
        -UseRecoverySelfTestTaskStore:$UseRecoverySelfTestTaskStore
    $preflightTaskState = 'absent'
    if ($null -ne $preflightTaskXml) {
        foreach ($candidate in @(
            @('old', $recovery.LegacyReceipt),
            @('new', $recovery.StagedReceipt)
        )) {
            try {
                [void](Assert-BrokerTaskXmlBinding `
                    -Xml ([string]$preflightTaskXml) `
                    -PowerShellPath ([string]$candidate[1].powershell_path) `
                    -WorkerPath ([string]$candidate[1].worker_path) `
                    -BrokerRoot $Root -Requests $Requests `
                    -Account $Account -UserSid $UserSid)
                $preflightTaskState = [string]$candidate[0]
                break
            } catch { }
        }
    }
    $preflightReady = Get-BrokerGitCapabilityReadyState `
        -Receipt $recovery.StagedReceipt -Root $Root
    $preflightDisposition = Resolve-BrokerUpgradeRecoveryDisposition `
        -ReceiptState ([string]$preflightReceipt.State) `
        -ReadyState ([string]$preflightReady.State)
    if ($preflightDisposition -ceq 'indeterminate_requires_recovery' -or
        ([string]$preflightReady.State -ceq 'valid' -and
            $preflightTaskState -cne 'new') -or
        ($null -ne $preflightTaskXml -and
            $preflightTaskState -ceq 'absent')) {
        throw "indeterminate_requires_recovery: receipt_state=$($preflightReceipt.State) ready_state=$($preflightReady.State) task_state=$preflightTaskState ready_error=$($preflightReady.Error)"
    }
    [void](Ensure-BrokerUpgradeConsumedNonceForRecovery `
        -Recovery $recovery -Root $Root `
        -FileSecurity $FileSecurity -UserSid $UserSid)
    $recreatedResultsRoot = Ensure-BrokerUpgradeResultsRoot `
        -Root $Root -UserSid $UserSid -SandboxSid $SandboxSid
    $resultsGuard = Open-InstallerDirectoryGuard `
        -Path (Join-Path $Root 'results') `
        -Label 'Broker result root recovery guard'
    try {
    $receiptPath = Join-Path $Root $script:ReceiptName
    $currentReceipt = Get-BrokerReceiptFileState -Path $receiptPath `
        -OldSha256 ([string]$recovery.Journal.legacy_receipt_sha256) `
        -NewSha256 ([string]$recovery.Journal.staged_receipt_sha256) `
        -Security $FileSecurity -OwnerSid $UserSid
    $currentReceiptHash = [string]$currentReceipt.Sha256
    $currentReceiptState = [string]$currentReceipt.State

    $taskXml = Get-BrokerUpgradeRecoveryTaskXml `
        -Name $Name -Root $Root -FileSecurity $FileSecurity `
        -UserSid $UserSid `
        -UseRecoverySelfTestTaskStore:$UseRecoverySelfTestTaskStore
    $taskState = 'absent'
    if ($null -ne $taskXml) {
        foreach ($candidate in @(
            @('old', $recovery.LegacyReceipt),
            @('new', $recovery.StagedReceipt)
        )) {
            try {
                Assert-BrokerTaskXmlBinding -Xml ([string]$taskXml) `
                    -PowerShellPath ([string]$candidate[1].powershell_path) `
                    -WorkerPath ([string]$candidate[1].worker_path) `
                    -BrokerRoot $Root -Requests $Requests `
                    -Account $Account -UserSid $UserSid
                $taskState = [string]$candidate[0]
                break
            } catch { }
        }
        if ($taskState -ceq 'absent') {
            throw 'Broker upgrade recovery refuses a task outside the journaled v2/v3 tuple.'
        }
        Assert-BrokerUpgradeRecoveryTaskSecurity `
            -Name $Name -Root $Root -FileSecurity $FileSecurity `
            -UserSid $UserSid -SandboxSid $SandboxSid `
            -UseRecoverySelfTestTaskStore:$UseRecoverySelfTestTaskStore
    }

    $readyPath = Join-Path $Root $script:GitCapabilityReadyName
    $committedState = $null
    $readyState = 'absent'
    $readyItem = Get-Item -LiteralPath $readyPath -Force `
        -ErrorAction SilentlyContinue
    if ($null -ne $readyItem -and (
            $readyItem.PSIsContainer -or
            ($readyItem.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0
        )) {
        $committedValidationFailure = 'ready marker exists but is not an ordinary file'
        $readyState = 'invalid'
    } elseif ($null -ne $readyItem) {
        try {
            if ($currentReceiptHash -cne
                [string]$recovery.Journal.staged_receipt_sha256) {
                throw 'ready marker exists without the journaled v3 receipt'
            }
            if ($taskState -cne 'new') {
                throw 'ready marker exists without the journaled v3 task'
            }
            $committed = Read-Receipt -Root $Root -Requests $Requests `
                -Name $Name -SkipTaskValidation -SkipGitLiveValidation
            [void](Assert-GitCapabilityReadyInstalled `
                -Receipt $committed -Root $Root)
            $heartbeat = Wait-BrokerHeartbeat `
                -Root $Root -Receipt $committed `
                -TimeoutSeconds $script:BrokerHeartbeatStartupTimeoutSeconds `
                -TaskName $Name
            $committedState = [pscustomobject]@{
                Receipt = $committed
                Heartbeat = $heartbeat
            }
            $readyState = 'valid'
        } catch {
            $committedValidationFailure = $_.Exception.Message
            $readyState = 'invalid'
        }
    } else {
        $committedValidationFailure = 'ready marker is absent'
    }
    $disposition = Resolve-BrokerUpgradeRecoveryDisposition `
        -ReceiptState $currentReceiptState -ReadyState $readyState
    if ($disposition -ceq 'indeterminate_requires_recovery') {
        throw "indeterminate_requires_recovery: receipt_state=$currentReceiptState ready_state=$readyState committed_validation=$committedValidationFailure"
    }
    if ($disposition -ceq 'committed_v3' -and $null -ne $committedState) {
        try {
            Remove-BrokerUpgradeRecoveryJournal -Recovery $recovery
        } catch {
            throw "Broker upgrade recovery verified committed v3 but retained its recovery journal: $($_.Exception.Message)"
        }
        return [pscustomobject]@{
            Mode = 'committed_v3'
            PreviousSchemaVersion = $script:LegacySchemaVersion
            Receipt = $committedState.Receipt
            Heartbeat = $committedState.Heartbeat
            RecreatedResultsRoot = $recreatedResultsRoot
        }
    }

    $transactions = Join-Path $Root $script:GitTransactionDirectoryName
    $recoveryGuard = $null
    $recoveryLockPath = $null
    $recoveryLockBinding = $null
    if (Test-Path -LiteralPath $transactions -PathType Container) {
        try {
            $recoveryLockPath = Get-GitTransactionCleanupLockPath `
                -Path $transactions
            $recoveryGuard = Open-GitTransactionGuard -Path $recoveryLockPath
            $recoveryLockBinding = `
                New-BrokerUpgradeOwnedFileBindingFromHeldStream `
                    -Path $recoveryLockPath -Stream $recoveryGuard `
                    -Label 'Broker recovery held transaction lock'
            if ((Get-GitTransactionCleanupLockPath -Path $transactions) -cne
                $recoveryLockPath) {
                throw 'transaction lock identity changed during recovery preflight'
            }
        } catch {
            if ($null -ne $recoveryGuard) { $recoveryGuard.Dispose() }
            throw "Broker upgrade recovery is indeterminate and preserved its journal: committed_validation=$committedValidationFailure; transaction=$($_.Exception.Message)"
        }
    }
    try {
        Stop-BrokerUpgradeRecoveryWorker -Name $Name -Root $Root `
            -Receipts @($recovery.StagedReceipt, $recovery.LegacyReceipt) `
            -FileSecurity $FileSecurity -UserSid $UserSid `
            -UseRecoverySelfTestTaskStore:$UseRecoverySelfTestTaskStore
        [void](Restore-BrokerReceiptFromJournal -Path $receiptPath `
            -OldBytes $recovery.LegacyReceiptBytes `
            -NewBytes $recovery.StagedReceiptBytes `
            -Security $FileSecurity -OwnerSid $UserSid)
        Register-BrokerUpgradeRecoveryTask `
            -Name $Name -Xml $recovery.LegacyTaskXml -Root $Root `
            -FileSecurity $FileSecurity -UserSid $UserSid `
            -UseRecoverySelfTestTaskStore:$UseRecoverySelfTestTaskStore
        $taskSecurity = Get-ExpectedTaskSecurityDescriptor `
            -UserSid $UserSid -SandboxSid $SandboxSid
        Set-BrokerUpgradeRecoveryTaskSecurity `
            -Name $Name -ExpectedSddl $taskSecurity -Root $Root `
            -FileSecurity $FileSecurity -UserSid $UserSid `
            -SandboxSid $SandboxSid `
            -UseRecoverySelfTestTaskStore:$UseRecoverySelfTestTaskStore
        Remove-Item -LiteralPath (Join-Path (Join-Path $Root 'results') `
            $script:HeartbeatName) -Force -ErrorAction SilentlyContinue
        $restoredAt = [DateTimeOffset]::UtcNow
        Start-BrokerUpgradeRecoveryTask `
            -Name $Name -Root $Root -Receipt $recovery.LegacyReceipt `
            -FileSecurity $FileSecurity -UserSid $UserSid `
            -UseRecoverySelfTestTaskStore:$UseRecoverySelfTestTaskStore
        $heartbeat = Wait-BrokerHeartbeat `
            -Root $Root -Receipt $recovery.LegacyReceipt `
            -TimeoutSeconds $script:BrokerHeartbeatStartupTimeoutSeconds `
            -NotBefore $restoredAt -TaskName $Name

        $createdPaths = [Collections.Generic.List[object]]::new()
        if ([int]$recovery.SchemaVersion -eq
            $script:UpgradeRecoveryJournalSchemaVersion) {
            foreach ($binding in @($recovery.StagingObjects)) {
                $createdPaths.Add($binding)
            }
        } else {
            $legacyReadOnlySecurity = New-ManagedDirectorySecurity `
                -UserSid $UserSid -SandboxSid $SandboxSid -Kind ReadOnly
            $legacyVersionRoot = Split-Path -Parent `
                ([string]$recovery.StagedReceipt.worker_path)
            $legacyWorkerPath = [string]$recovery.StagedReceipt.worker_path
            $legacyTransactionLock = Join-Path $transactions 'transaction.lock'
            $legacyHooks = Join-Path $Root 'empty-hooks'
            $legacyManifest = Join-Path $Root $script:GitCapabilityManifestName
            foreach ($directory in @(
                $legacyVersionRoot, $transactions, $legacyHooks
            )) {
                [void](Assert-RealDirectory `
                    -Path $directory -Label 'Legacy recovery staging directory')
                Assert-ExactSecurity -Path $directory `
                    -Expected $legacyReadOnlySecurity `
                    -ExpectedOwnerSid $UserSid `
                    -Label 'Legacy recovery staging directory'
            }
            foreach ($file in @(
                $legacyWorkerPath, $legacyTransactionLock, $legacyManifest
            )) {
                Assert-ExactSecurity -Path $file -Expected $FileSecurity `
                    -ExpectedOwnerSid $UserSid `
                    -Label 'Legacy recovery staging file'
            }
            if ((Get-FileSha256 -Path $legacyWorkerPath) -cne
                    [string]$recovery.StagedReceipt.worker_sha256 -or
                (Get-FileSha256 -Path $legacyManifest) -cne
                    [string]$recovery.StagedReceipt.git_capability_manifest_sha256 -or
                (Get-Item -LiteralPath $legacyTransactionLock -Force).Length -ne 0 -or
                @(Get-ChildItem -LiteralPath $legacyHooks -Force).Count -ne 0) {
                throw 'Legacy recovery staging bytes are not the journaled exact tuple.'
            }
            foreach ($entry in @(
                @($legacyManifest, 'file'),
                @($legacyHooks, 'directory'),
                @($transactions, 'directory'),
                @($legacyTransactionLock, 'file'),
                @($legacyVersionRoot, 'directory'),
                @($legacyWorkerPath, 'file')
            )) {
                if (Test-Path -LiteralPath $entry[0]) {
                    if ([string]$entry[0] -ceq $legacyTransactionLock) {
                        if ($null -eq $recoveryLockBinding -or
                            [string]$recoveryLockBinding.path -cne
                                $legacyTransactionLock) {
                            throw 'Legacy recovery lost its held transaction-lock binding.'
                        }
                        $createdPaths.Add($recoveryLockBinding)
                    } else {
                        $createdPaths.Add((New-BrokerUpgradeOwnedObjectBinding `
                            -Path $entry[0] -Type $entry[1]))
                    }
                }
            }
        }
        if ($null -ne $recoveryGuard) {
            if ((Get-GitTransactionCleanupLockPath -Path $transactions) -cne
                $recoveryLockPath) {
                throw 'transaction directory changed before recovery cleanup'
            }
            $recoveryGuard.Dispose()
            $recoveryGuard = $null
        }
        Remove-BrokerUpgradeCreatedPaths `
            -Root $Root -TransactionsPath $transactions `
            -CreatedPaths $createdPaths
        Remove-BrokerUpgradeRecoveryJournal -Recovery $recovery
        return [pscustomobject]@{
            Mode = 'rolled_back_v2'
            PreviousSchemaVersion = $script:LegacySchemaVersion
            Receipt = $recovery.LegacyReceipt
            Heartbeat = $heartbeat
            RecreatedResultsRoot = $recreatedResultsRoot
        }
    } finally {
        if ($null -ne $recoveryGuard) { $recoveryGuard.Dispose() }
    }
    } finally {
        $resultsGuard.Dispose()
    }
}

function Invoke-BrokerUpgradeSwitchTransaction {
    param(
        [Parameter(Mandatory = $true)][scriptblock]$StopOldAction,
        [Parameter(Mandatory = $true)][scriptblock]$PublishNewReceiptAction,
        [Parameter(Mandatory = $true)][scriptblock]$RegisterNewTaskAction,
        [Parameter(Mandatory = $true)][scriptblock]$StartNewTaskAction,
        [Parameter(Mandatory = $true)][scriptblock]$WaitNewHeartbeatAction,
        [Parameter(Mandatory = $true)][scriptblock]$CommitAction,
        [Parameter(Mandatory = $true)][scriptblock]$ReleaseGuardAction,
        [Parameter(Mandatory = $true)][scriptblock]$StopRollbackAction,
        [Parameter(Mandatory = $true)][scriptblock]$RestoreOldReceiptAction,
        [scriptblock]$RestoreOldReadyAction,
        [Parameter(Mandatory = $true)][scriptblock]$RegisterOldTaskAction,
        [Parameter(Mandatory = $true)][scriptblock]$StartOldTaskAction,
        [Parameter(Mandatory = $true)][scriptblock]$WaitOldHeartbeatAction,
        [Parameter(Mandatory = $true)][scriptblock]$CleanupStagingAction
    )

    $phase = 'staged'
    try {
        [void](& $StopOldAction)
        $phase = 'old_stopped'
        [void](& $PublishNewReceiptAction)
        $phase = 'new_receipt_published'
        [void](& $RegisterNewTaskAction)
        $phase = 'new_task_registered'
        [void](& $StartNewTaskAction)
        $phase = 'new_task_started'
        $heartbeatOutput = @(& $WaitNewHeartbeatAction)
        if ($heartbeatOutput.Count -ne 1) {
            throw "Broker upgrade new-heartbeat action returned $($heartbeatOutput.Count) objects."
        }
        $heartbeat = $heartbeatOutput[0]
        $phase = 'new_heartbeat_verified'
        [void](& $ReleaseGuardAction)
        $phase = 'guard_released'
        $commitOutput = @(& $CommitAction $heartbeat)
        if ($commitOutput.Count -ne 1) {
            throw "Broker upgrade commit action returned $($commitOutput.Count) objects."
        }
        return $commitOutput[0]
    } catch {
        $failure = $_.Exception.Message
        try {
            [void](& $StopRollbackAction)
            [void](& $RestoreOldReceiptAction)
            if ($null -ne $RestoreOldReadyAction) {
                [void](& $RestoreOldReadyAction)
            }
            [void](& $RegisterOldTaskAction)
            [void](& $StartOldTaskAction)
            $oldHeartbeatOutput = @(& $WaitOldHeartbeatAction)
            if ($oldHeartbeatOutput.Count -ne 1) {
                throw "Broker upgrade rollback heartbeat action returned $($oldHeartbeatOutput.Count) objects."
            }
            [void](& $ReleaseGuardAction)
            [void](& $CleanupStagingAction)
        } catch {
            $rollbackFailure = $_.Exception.Message
            try { & $ReleaseGuardAction } catch {
                $rollbackFailure += "; guard_release=$($_.Exception.Message)"
            }
            throw "Broker upgrade failed and rollback could not be proven: phase=$phase; failure=$failure; rollback=$rollbackFailure"
        }
        throw "Broker upgrade failed; the exact schema-v2 task/receipt/heartbeat were restored: phase=$phase; failure=$failure"
    }
}

function Upgrade-BrokerCurrentSchema {
    param(
        [Parameter(Mandatory = $true)][string]$Root,
        [Parameter(Mandatory = $true)][string]$Requests,
        [Parameter(Mandatory = $true)][string]$Name,
        [Parameter(Mandatory = $true)][string]$Account,
        [Parameter(Mandatory = $true)][string]$Group,
        [Parameter(Mandatory = $true)][string]$UserSid,
        [Parameter(Mandatory = $true)][string]$SandboxSid,
        [Parameter(Mandatory = $true)]$ReadOnlySecurity,
        [Parameter(Mandatory = $true)]$FileSecurity,
        [Parameter(Mandatory = $true)]$UpgradeAuthority,
        [Parameter(Mandatory = $true)]$InstallationGuard,
        [Parameter(Mandatory = $true)]$PreviousSnapshot,
        [Parameter(Mandatory = $true)]$Previous,
        [Parameter(Mandatory = $true)][string]$GoalId,
        [Parameter(Mandatory = $true)][string]$SourceFingerprint,
        [bool]$RecoveredInterruptedSwitch,
        [bool]$RecoveredMissingResultsRoot
    )
    if ([int]$Previous.schema_version -ne $script:SchemaVersion -or
        [string]$Previous.user_account -cne $Account -or
        [string]$Previous.user_sid -cne $UserSid -or
        [string]$Previous.sandbox_group -cne $Group -or
        [string]$Previous.sandbox_group_sid -cne $SandboxSid) {
        throw 'Current-schema upgrade requires one exact protected schema-v3 installation tuple.'
    }
    [void](Wait-BrokerHeartbeat -Root $Root -Receipt $Previous `
        -TimeoutSeconds 2 -TaskName $Name)
    $previousReady = Get-BrokerGitCapabilityReadyState `
        -Receipt $Previous -Root $Root
    if ([string]$previousReady.State -cne 'valid') {
        throw "Current-schema upgrade requires the exact ready marker: $($previousReady.Error)"
    }
    if (@(Get-ChildItem -LiteralPath $Requests `
            -Filter '*.request.json' -File -Force).Count -ne 0) {
        throw 'Current-schema upgrade refuses a non-empty request queue.'
    }
    $workerAuthority = @($UpgradeAuthority.Handles | Where-Object {
        $_.Path.Equals(
            $script:BrokerSource,
            [StringComparison]::OrdinalIgnoreCase
        )
    })
    if ($workerAuthority.Count -ne 1) {
        throw 'Current-schema upgrade did not retain the authority-bound worker source handle.'
    }
    $sourceBytes = [byte[]]$workerAuthority[0].Bytes
    $workerHash = Get-BytesSha256 -Bytes $sourceBytes
    if ($workerHash -cne
            [string]$UpgradeAuthority.Manifest.worker_sha256) {
        throw 'Current-schema upgrade worker source drifted from the authority manifest.'
    }
    if ($workerHash -ceq [string]$Previous.worker_sha256) {
        throw 'Current-schema upgrade refuses to create a new transaction for identical worker bytes.'
    }
    $runtime = Get-CurrentPowerShellRuntime
    if ([string]$Previous.powershell_path -cne $runtime.Path -or
        [string]$Previous.powershell_sha256 -cne $runtime.Sha256) {
        throw 'Current-schema upgrade refuses a PowerShell runtime identity change.'
    }
    $receiptPath = Join-Path $Root $script:ReceiptName
    Wait-BrokerReceiptReplaceReady -Path $receiptPath -TimeoutMilliseconds 5000
    $transactions = Join-Path $Root $script:GitTransactionDirectoryName
    $transactionState = Get-GitTransactionOperationalState `
        -Path $transactions -Root $Root -Receipt $Previous `
        -FileSecurity $FileSecurity -UserSid $UserSid
    $transactionLock = [string]$transactionState.LockPath
    $hooks = Join-Path $Root 'empty-hooks'
    $manifestPath = Join-Path $Root $script:GitCapabilityManifestName
    $readyPath = Join-Path $Root $script:GitCapabilityReadyName
    if ([string]$Previous.git_capability_manifest_path -cne $manifestPath -or
        [string]$Previous.git_capability_manifest_sha256 -cne
            (Get-FileSha256 -Path $manifestPath) -or
        @(Get-ChildItem -LiteralPath $hooks -Force).Count -ne 0) {
        throw 'Current-schema upgrade fixed capability state drifted.'
    }

    $versions = Join-Path $Root 'versions'
    $versionRoot = Join-Path $versions $workerHash
    $workerPath = Join-Path $versionRoot 'codex-powershell-broker.ps1'
    if (Test-Path -LiteralPath $versionRoot) {
        throw "Current-schema upgrade refuses a pre-existing worker version: $versionRoot"
    }
    $receipt = [ordered]@{
        schema_version = $script:SchemaVersion
        install_id = [string]$Previous.install_id
        install_root = $Root
        installed_at_utc = [DateTimeOffset]::UtcNow.ToString('o')
        task_name = $Name
        user_account = $Account
        user_sid = $UserSid
        sandbox_group = $Group
        sandbox_group_sid = $SandboxSid
        worker_path = $workerPath
        worker_sha256 = $workerHash
        powershell_path = $runtime.Path
        powershell_sha256 = $runtime.Sha256
        request_root = $Requests
        result_root = Join-Path $Root 'results'
        capabilities = @('identity_probe', $script:GitCapabilityId)
        git_capability_manifest_path = $manifestPath
        git_capability_manifest_sha256 =
            [string]$Previous.git_capability_manifest_sha256
    }
    $stagedReceiptBytes = [Text.UTF8Encoding]::new($false, $true).GetBytes(
        ($receipt | ConvertTo-Json -Depth 16 -Compress)
    )
    $readyDocument = New-GitCapabilityReadyDocument `
        -InstallId ([string]$Previous.install_id) `
        -WorkerHash $workerHash `
        -ManifestHash ([string]$Previous.git_capability_manifest_sha256
    )
    $stagedReadyBytes = [Text.UTF8Encoding]::new($false, $true).GetBytes(
        ($readyDocument | ConvertTo-Json -Depth 12 -Compress)
    )
    $oldTaskXml = Get-TaskXmlSnapshot -Name $Name
    if ($null -eq $oldTaskXml) {
        throw 'Current-schema upgrade cannot snapshot the previous task.'
    }
    $newTaskXml = New-BrokerTaskXml -PowerShellPath $runtime.Path `
        -WorkerPath $workerPath -BrokerRoot $Root -Requests $Requests `
        -Account $Account -Name $Name
    $oldReceiptBytes = [byte[]]$PreviousSnapshot.Bytes
    $oldReadyBytes = [byte[]]$previousReady.Snapshot.Bytes
    $results = Join-Path $Root 'results'
    $resultsGuard = Open-InstallerDirectoryGuard -Path $results `
        -Label 'Current-schema result root upgrade guard'
    $guardState = [pscustomobject]@{ Stream = $null }
    $createdPaths = [Collections.Generic.List[object]]::new()
    $activeRecovery = $null
    $recoveryJournalPath = Join-Path $Root `
        $script:UpgradeRecoveryJournalName
    try {
        $guardState.Stream = Open-GitTransactionGuard `
            -Path $transactionLock
        [void](Remove-TerminalGitTransactionJournalsForUpgrade `
            -ExpectedState $transactionState -Path $transactions -Root $Root `
            -Receipt $Previous -FileSecurity $FileSecurity -UserSid $UserSid)
        if ((Get-GitTransactionCleanupLockPath -Path $transactions) -cne
            $transactionLock) {
            throw 'Current-schema transaction lock changed during staging.'
        }
        if (@(Get-ChildItem -LiteralPath $Requests `
                -Filter '*.request.json' -File -Force).Count -ne 0) {
            throw 'Current-schema upgrade observed a request after taking the transaction guard.'
        }
        $versionRoot = New-ManagedDirectory -Path $versionRoot `
            -Security $ReadOnlySecurity -OwnerSid $UserSid `
            -Label 'Current-schema worker version directory'
        $createdPaths.Add((New-BrokerUpgradeOwnedObjectBinding `
            -Path $versionRoot -Type directory))
        Write-BytesAtomic -Path $workerPath -Bytes $sourceBytes `
            -Security $FileSecurity
        $createdPaths.Add((New-BrokerUpgradeOwnedObjectBinding `
            -Path $workerPath -Type file))
        $journalObjects = @($createdPaths | ForEach-Object {
            [ordered]@{
                path = [string]$_.path
                type = [string]$_.type
                identity = [string]$_.identity
                sha256 = $_.sha256
                owner_sid = [string]$_.owner_sid
                access_sddl = [string]$_.access_sddl
            }
        })
        $transactionId = [Guid]::NewGuid().ToString('N')
        $recoveryJournal = New-BrokerUpgradeRecoveryJournalDocument `
            -Root $Root -Requests $Requests -Name $Name `
            -Account $Account -UserSid $UserSid -Group $Group `
            -SandboxSid $SandboxSid `
            -LegacyReceiptBytes $oldReceiptBytes `
            -LegacyReceiptIdentity ([string]$PreviousSnapshot.Identity) `
            -StagedReceiptBytes $stagedReceiptBytes `
            -LegacyTaskXml $oldTaskXml `
            -TransactionId $transactionId `
            -InstallationGuardName ([string]$InstallationGuard.Name) `
            -LauncherNonce ([string]$UpgradeAuthority.LauncherNonce) `
            -GoalId $GoalId -SourceFingerprint $SourceFingerprint `
            -StagingObjects $journalObjects `
            -JournalSchemaVersion `
                $script:CurrentUpgradeRecoveryJournalSchemaVersion `
            -LegacyReadyBytes $oldReadyBytes `
            -LegacyReadyIdentity ([string]$previousReady.Snapshot.Identity) `
            -StagedReadyBytes $stagedReadyBytes
        Write-JsonAtomic -Path $recoveryJournalPath `
            -Document $recoveryJournal -Security $FileSecurity
        $activeRecovery = Read-BrokerUpgradeRecoveryJournal `
            -Root $Root -Requests $Requests -Name $Name `
            -Account $Account -UserSid $UserSid -Group $Group `
            -SandboxSid $SandboxSid `
            -ExpectedFileSecurity $FileSecurity
        [void](Write-BrokerUpgradeConsumedNonce `
            -Root $Root `
            -Nonce ([string]$UpgradeAuthority.LauncherNonce) `
            -TransactionId $transactionId -GoalId $GoalId `
            -SourceFingerprint $SourceFingerprint `
            -FileSecurity $FileSecurity -UserSid $UserSid)
    } catch {
        $failure = $_.Exception.Message
        try {
            if ($null -ne $guardState.Stream) {
                $guardState.Stream.Dispose()
                $guardState.Stream = $null
            }
            Remove-BrokerUpgradeCreatedPaths -Root $Root `
                -TransactionsPath $transactions -CreatedPaths $createdPaths
        } catch {
            $failure += "; staging_cleanup=$($_.Exception.Message)"
        }
        if ($null -ne $activeRecovery) {
            try {
                Remove-BrokerUpgradeRecoveryJournal -Recovery $activeRecovery
            } catch {
                $failure += "; recovery_journal_cleanup=$($_.Exception.Message)"
            }
        } elseif (Test-Path -LiteralPath $recoveryJournalPath) {
            $failure += '; recovery_journal_cleanup=unbound journal preserved'
        }
        $resultsGuard.Dispose()
        throw "Current-schema upgrade staging failed before receipt publication: $failure"
    }

    $switchState = [pscustomobject]@{
        NewStartedAt = $null
        RestoredAt = $null
    }
    $releaseGuardAction = {
        if ($null -ne $guardState.Stream) {
            $guardState.Stream.Dispose()
            $guardState.Stream = $null
        }
    }
    $stopOldAction = {
        Stop-ExactBrokerWorkerForUpgrade -Name $Name -Root $Root `
            -Receipts @($Previous)
        Wait-BrokerReceiptReplaceReady -Path $receiptPath `
            -TimeoutMilliseconds 5000
    }
    $publishNewReceiptAction = {
        Remove-Item -LiteralPath (Join-Path $results `
            $script:HeartbeatName) -Force -ErrorAction SilentlyContinue
        [void](Publish-BrokerReceiptForward -Path $receiptPath `
            -OldBytes $oldReceiptBytes -NewBytes $stagedReceiptBytes `
            -Security $FileSecurity -OwnerSid $UserSid)
    }
    $registerNewTaskAction = {
        [void](Register-BrokerTask -Name $Name -Xml $newTaskXml)
        $taskSecurity = Get-ExpectedTaskSecurityDescriptor `
            -UserSid $UserSid -SandboxSid $SandboxSid
        Set-And-AssertBrokerTaskSecurity -Name $Name `
            -ExpectedSddl $taskSecurity -UserSid $UserSid `
            -SandboxSid $SandboxSid
    }
    $startNewTaskAction = {
        $switchState.NewStartedAt = [DateTimeOffset]::UtcNow
        Start-BrokerTask -Name $Name
    }
    $waitNewHeartbeatAction = {
        $current = Read-Receipt -Root $Root -Requests $Requests `
            -Name $Name
        return Wait-BrokerHeartbeat -Root $Root -Receipt $current `
            -TimeoutSeconds $script:BrokerHeartbeatStartupTimeoutSeconds `
            -NotBefore $switchState.NewStartedAt -TaskName $Name
    }
    $commitAction = {
        param($Heartbeat)
        $result = [ordered]@{
            upgraded = $true
            from_schema = $script:SchemaVersion
            to_schema = $script:SchemaVersion
            install_id = [string]$Previous.install_id
            task_name = $Name
            install_root = $Root
            repository_root = $script:RepositoryRoot
            allowed_ref = 'refs/heads/main'
            user_account = $Account
            user_sid = $UserSid
            worker_sha256 = $workerHash
            git_capability_manifest_sha256 =
                [string]$Previous.git_capability_manifest_sha256
            recovered_interrupted_staging = $false
            recovered_interrupted_switch = $(if (
                $RecoveredInterruptedSwitch
            ) { 'rolled_back_v3' } else { $null })
            recovered_missing_results_root = $RecoveredMissingResultsRoot
            recovery_journal_retained = $false
            recovery_journal_cleanup_error = $null
            heartbeat_executor = [string]$Heartbeat.executor_account
            heartbeat_sid = [string]$Heartbeat.executor_sid
        }
        [void](Publish-BrokerReadyForward -Path $readyPath `
            -OldBytes $oldReadyBytes -NewBytes $stagedReadyBytes `
            -Security $FileSecurity -OwnerSid $UserSid)
        try {
            Remove-BrokerUpgradeRecoveryJournal -Recovery $activeRecovery
        } catch {
            $result.recovery_journal_retained = $true
            $result.recovery_journal_cleanup_error = $_.Exception.Message
        }
        return $result
    }
    $stopRollbackAction = {
        Stop-ExactBrokerWorkerForUpgrade -Name $Name -Root $Root `
            -Receipts @($receipt, $Previous) -AllowMissingTask
    }
    $restoreOldReceiptAction = {
        [void](Restore-BrokerReceiptFromJournal -Path $receiptPath `
            -OldBytes $oldReceiptBytes -NewBytes $stagedReceiptBytes `
            -Security $FileSecurity -OwnerSid $UserSid)
    }
    $restoreOldReadyAction = {
        [void](Restore-BrokerReadyFromJournal -Path $readyPath `
            -OldBytes $oldReadyBytes -NewBytes $stagedReadyBytes `
            -Security $FileSecurity -OwnerSid $UserSid)
    }
    $registerOldTaskAction = {
        [void](Register-BrokerTask -Name $Name -Xml $oldTaskXml)
        $taskSecurity = Get-ExpectedTaskSecurityDescriptor `
            -UserSid $UserSid -SandboxSid $SandboxSid
        Set-And-AssertBrokerTaskSecurity -Name $Name `
            -ExpectedSddl $taskSecurity -UserSid $UserSid `
            -SandboxSid $SandboxSid
    }
    $startOldTaskAction = {
        Remove-Item -LiteralPath (Join-Path $results `
            $script:HeartbeatName) -Force -ErrorAction SilentlyContinue
        $switchState.RestoredAt = [DateTimeOffset]::UtcNow
        Start-BrokerTask -Name $Name
    }
    $waitOldHeartbeatAction = {
        $restored = Read-Receipt -Root $Root -Requests $Requests `
            -Name $Name
        return Wait-BrokerHeartbeat -Root $Root -Receipt $restored `
            -TimeoutSeconds $script:BrokerHeartbeatStartupTimeoutSeconds `
            -NotBefore $switchState.RestoredAt -TaskName $Name
    }
    $cleanupStagingAction = {
        Remove-BrokerUpgradeCreatedPaths -Root $Root `
            -TransactionsPath $transactions -CreatedPaths $createdPaths
        Remove-BrokerUpgradeRecoveryJournal -Recovery $activeRecovery
    }
    try {
        return Invoke-BrokerUpgradeSwitchTransaction `
            -StopOldAction $stopOldAction `
            -PublishNewReceiptAction $publishNewReceiptAction `
            -RegisterNewTaskAction $registerNewTaskAction `
            -StartNewTaskAction $startNewTaskAction `
            -WaitNewHeartbeatAction $waitNewHeartbeatAction `
            -CommitAction $commitAction `
            -ReleaseGuardAction $releaseGuardAction `
            -StopRollbackAction $stopRollbackAction `
            -RestoreOldReceiptAction $restoreOldReceiptAction `
            -RestoreOldReadyAction $restoreOldReadyAction `
            -RegisterOldTaskAction $registerOldTaskAction `
            -StartOldTaskAction $startOldTaskAction `
            -WaitOldHeartbeatAction $waitOldHeartbeatAction `
            -CleanupStagingAction $cleanupStagingAction
    } finally {
        if ($null -ne $guardState.Stream) {
            $guardState.Stream.Dispose()
        }
        $resultsGuard.Dispose()
    }
}

function Upgrade-Broker {
    param(
        [string]$Root,
        [string]$Requests,
        [string]$Name,
        [string]$Account,
        [string]$Group,
        [Parameter(Mandatory = $true)][string]$AuthorityManifestPath,
        [Parameter(Mandatory = $true)][string]$AuthorityManifestSha256,
        [Parameter(Mandatory = $true)][string]$GoalId,
        [Parameter(Mandatory = $true)][string]$SourceFingerprint,
        [Parameter(Mandatory = $true)]$InstallationGuard
    )

    $userSid = Resolve-AccountSid -Account $Account -Label 'Target user'
    $sandboxSid = Resolve-AccountSid -Account $Group -Label 'Sandbox group'
    [void](Assert-InstallAuthority -ExpectedAccount $Account -ExpectedSid $userSid)
    $root = Assert-RealDirectory -Path $Root -Label 'Broker install root'
    $requests = Assert-RealDirectory -Path $Requests -Label 'Broker request root'
    $readOnlySecurity = New-ManagedDirectorySecurity `
        -UserSid $userSid -SandboxSid $sandboxSid -Kind ReadOnly
    $fileSecurity = New-ManagedFileSecurity -UserSid $userSid -SandboxSid $sandboxSid
    $upgradeAuthority = Open-BrokerUpgradeAuthority -Action upgrade `
        -ManifestPath $AuthorityManifestPath `
        -ManifestSha256 $AuthorityManifestSha256 `
        -GoalId $GoalId -SourceFingerprint $SourceFingerprint `
        -Root $root -Requests $requests -Name $Name `
        -Account $Account -Group $Group -InstallationGuard $InstallationGuard
    try {
    $interruptedRecovery = Invoke-BrokerInterruptedUpgradeRecovery `
        -Root $root -Requests $requests -Name $Name -Account $Account `
        -UserSid $userSid -Group $Group -SandboxSid $sandboxSid `
        -FileSecurity $fileSecurity
    if ($null -ne $interruptedRecovery -and
        [string]$interruptedRecovery.Mode -ceq 'committed_v3') {
        $committed = $interruptedRecovery.Receipt
        $committedManifest = Read-JsonDocument `
            -Path ([string]$committed.git_capability_manifest_path) `
            -Label 'Recovered committed Git capability manifest'
        return [ordered]@{
            upgraded = $true
            from_schema = [int]$interruptedRecovery.PreviousSchemaVersion
            to_schema = $script:SchemaVersion
            install_id = [string]$committed.install_id
            task_name = $Name
            install_root = $root
            repository_root = [string]$committedManifest.repository_root
            allowed_ref = [string]$committedManifest.allowed_ref
            user_account = $Account
            user_sid = $userSid
            worker_sha256 = [string]$committed.worker_sha256
            git_sha256 = [string]$committedManifest.git_executable_sha256
            git_capability_manifest_sha256 =
                [string]$committed.git_capability_manifest_sha256
            recovered_interrupted_staging = $false
            recovered_interrupted_switch = 'committed_v3'
            recovered_missing_results_root =
                [bool]$interruptedRecovery.RecreatedResultsRoot
            recovery_journal_retained = $false
            recovery_journal_cleanup_error = $null
            heartbeat_executor =
                [string]$interruptedRecovery.Heartbeat.executor_account
            heartbeat_sid = [string]$interruptedRecovery.Heartbeat.executor_sid
        }
    }
    $recoveredInterruptedSwitch = $null -ne $interruptedRecovery -and
        [string]$interruptedRecovery.Mode -ceq 'rolled_back_v2'
    $recoveredMissingResultsRoot = $null -ne $interruptedRecovery -and
        [bool]$interruptedRecovery.RecreatedResultsRoot
    $receiptPath = Join-Path $root $script:ReceiptName
    $currentSnapshot = Get-BrokerFileSnapshot -Path $receiptPath `
        -Label 'Broker pre-upgrade receipt' -MaximumBytes 64KB
    $current = Read-Receipt -Root $root -Requests $requests -Name $Name
    $currentDocument = ConvertFrom-StrictJsonBytes `
        -Bytes $currentSnapshot.Bytes -Label 'Broker pre-upgrade receipt'
    if ([string]$currentDocument.install_id -cne [string]$current.install_id -or
        [string]$currentDocument.worker_sha256 -cne
            [string]$current.worker_sha256) {
        throw 'Broker upgrade pre-upgrade receipt snapshot drifted.'
    }
    if ([int]$current.schema_version -eq $script:SchemaVersion) {
        return Upgrade-BrokerCurrentSchema `
            -Root $root -Requests $requests -Name $Name `
            -Account $Account -Group $Group -UserSid $userSid `
            -SandboxSid $sandboxSid `
            -ReadOnlySecurity $readOnlySecurity `
            -FileSecurity $fileSecurity `
            -UpgradeAuthority $upgradeAuthority `
            -InstallationGuard $InstallationGuard `
            -PreviousSnapshot $currentSnapshot -Previous $current `
            -GoalId $GoalId -SourceFingerprint $SourceFingerprint `
            -RecoveredInterruptedSwitch:(
                $null -ne $interruptedRecovery -and
                [string]$interruptedRecovery.Mode -ceq 'rolled_back_v3'
            ) `
            -RecoveredMissingResultsRoot:$recoveredMissingResultsRoot
    }
    $legacySnapshot = $currentSnapshot
    $legacy = $current
    $legacyDocument = $currentDocument
    if (
        [int]$legacy.schema_version -ne $script:LegacySchemaVersion -or
        [string]$legacy.user_account -cne $Account -or
        [string]$legacy.user_sid -cne $userSid -or
        [string]$legacy.sandbox_group -cne $Group -or
        [string]$legacy.sandbox_group_sid -cne $sandboxSid) {
        throw 'Broker upgrade accepts only one held exact protected schema-v2 installation tuple.'
    }
    [void](Wait-BrokerHeartbeat -Root $root -Receipt $legacy `
        -TimeoutSeconds 2 -TaskName $Name)
    if (@(Get-ChildItem -LiteralPath $requests -Filter '*.request.json' -File -Force).Count -ne 0) {
        throw 'Broker upgrade refuses a non-empty request queue.'
    }
    $workerAuthority = @($upgradeAuthority.Handles | Where-Object {
        $_.Path.Equals($script:BrokerSource, [StringComparison]::OrdinalIgnoreCase)
    })
    if ($workerAuthority.Count -ne 1) {
        throw 'Broker upgrade did not retain the authority-bound worker source handle.'
    }
    $sourceBytes = [byte[]]$workerAuthority[0].Bytes
    $workerHash = Get-BytesSha256 -Bytes $sourceBytes
    if ($workerHash -cne [string]$upgradeAuthority.Manifest.worker_sha256) {
        throw 'Broker upgrade worker source drifted from the authority manifest.'
    }
    $runtime = Get-CurrentPowerShellRuntime
    if ([string]$legacy.powershell_path -cne $runtime.Path -or
        [string]$legacy.powershell_sha256 -cne $runtime.Sha256) {
        throw 'Broker upgrade refuses a PowerShell runtime identity change.'
    }

    try {
        Wait-BrokerReceiptReplaceReady `
            -Path $receiptPath -TimeoutMilliseconds 5000
    } catch {
        throw "Broker upgrade receipt capability preflight failed before formal writes: $($_.Exception.Message)"
    }
    $results = Join-Path $root 'results'
    $resultsGuard = Open-InstallerDirectoryGuard -Path $results -Label 'Broker result root upgrade guard'
    try {
    $recoveredInterruptedStaging = Clear-ExactInterruptedUpgradeStaging -Root $root -LegacyReceipt $legacy -UserSid $userSid -SandboxSid $sandboxSid
    Assert-BrokerUpgradeNonceUnconsumed -Root $root -Nonce $upgradeAuthority.LauncherNonce

    $versions = Join-Path $root 'versions'
    $versionRoot = Join-Path $versions $workerHash
    $workerPath = Join-Path $versionRoot 'codex-powershell-broker.ps1'
    $transactions = Join-Path $root $script:GitTransactionDirectoryName
    $hooks = Join-Path $root 'empty-hooks'
    $transactionLock = Join-Path $transactions 'transaction.lock'
    $manifestPath = Join-Path $root $script:GitCapabilityManifestName
    $readyPath = Join-Path $root $script:GitCapabilityReadyName
    foreach ($path in @($versionRoot, $transactions, $hooks, $manifestPath, $readyPath)) {
        if (Test-Path -LiteralPath $path) {
            throw "Broker upgrade refuses pre-existing v3 staging state: $path"
        }
    }
    $installId = [Guid]::NewGuid().ToString('N')
    $manifest = New-GitCapabilityManifest -InstallId $installId -HooksRoot $hooks -TransactionsRoot $transactions
    $manifestBytes = [Text.UTF8Encoding]::new($false, $true).GetBytes(
        ($manifest | ConvertTo-Json -Depth 12 -Compress)
    )
    $manifestHash = Get-BytesSha256 -Bytes $manifestBytes
    $readyDocument = New-GitCapabilityReadyDocument -InstallId $installId -WorkerHash $workerHash -ManifestHash $manifestHash
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
        capabilities = @('identity_probe', $script:GitCapabilityId)
        git_capability_manifest_path = $manifestPath
        git_capability_manifest_sha256 = $manifestHash
    }
    $stagedReceiptBytes = [Text.UTF8Encoding]::new($false, $true).GetBytes(
        ($receipt | ConvertTo-Json -Depth 16 -Compress)
    )
    $newTaskXml = New-BrokerTaskXml -PowerShellPath $runtime.Path -WorkerPath $workerPath -BrokerRoot $root -Requests $requests -Account $Account -Name $Name
    $oldTaskXml = Get-TaskXmlSnapshot -Name $Name
    if ($null -eq $oldTaskXml) {
        throw 'Broker upgrade cannot snapshot the v2 task.'
    }
    $oldReceiptBytes = [byte[]]$legacySnapshot.Bytes

    $recoveryJournalPath = Join-Path $root $script:UpgradeRecoveryJournalName
    $createdPaths = [Collections.Generic.List[object]]::new()
    $guardState = [pscustomobject]@{ Stream = $null }
    $activeRecovery = $null
    try {
        $versionRoot = New-ManagedDirectory -Path $versionRoot -Security $readOnlySecurity -OwnerSid $userSid -Label 'Upgrade worker version directory'
        $createdPaths.Add((New-BrokerUpgradeOwnedObjectBinding -Path $versionRoot -Type directory))
        Write-BytesAtomic -Path $workerPath -Bytes $sourceBytes -Security $fileSecurity
        $createdPaths.Add((New-BrokerUpgradeOwnedObjectBinding -Path $workerPath -Type file))

        $transactions = New-ManagedDirectory -Path $transactions -Security $readOnlySecurity -OwnerSid $userSid -Label 'Git transaction root'
        $createdPaths.Add((New-BrokerUpgradeOwnedObjectBinding -Path $transactions -Type directory))
        Write-BytesAtomic -Path $transactionLock -Bytes ([byte[]]::new(0)) -Security $fileSecurity
        $createdPaths.Add((New-BrokerUpgradeOwnedObjectBinding -Path $transactionLock -Type file))
        $guardState.Stream = Open-GitTransactionGuard -Path $transactionLock

        $hooks = New-ManagedDirectory -Path $hooks -Security $readOnlySecurity -OwnerSid $userSid -Label 'Protected empty hooks root'
        $createdPaths.Add((New-BrokerUpgradeOwnedObjectBinding -Path $hooks -Type directory))
        Write-BytesAtomic -Path $manifestPath -Bytes $manifestBytes -Security $fileSecurity
        $createdPaths.Add((New-BrokerUpgradeOwnedObjectBinding -Path $manifestPath -Type file))

        $journalObjects = @($createdPaths | ForEach-Object {
            [ordered]@{
                path = [string]$_.path
                type = [string]$_.type
                identity = [string]$_.identity
                sha256 = $_.sha256
                owner_sid = [string]$_.owner_sid
                access_sddl = [string]$_.access_sddl
            }
        })
        $recoveryJournal = New-BrokerUpgradeRecoveryJournalDocument -Root $root -Requests $requests -Name $Name -Account $Account -UserSid $userSid -Group $Group -SandboxSid $sandboxSid -LegacyReceiptBytes $oldReceiptBytes -LegacyReceiptIdentity ([string]$legacySnapshot.Identity) -StagedReceiptBytes $stagedReceiptBytes -LegacyTaskXml $oldTaskXml -TransactionId $installId -InstallationGuardName ([string]$InstallationGuard.Name) -LauncherNonce ([string]$upgradeAuthority.LauncherNonce) -GoalId $GoalId -SourceFingerprint $SourceFingerprint -StagingObjects $journalObjects
        Write-JsonAtomic -Path $recoveryJournalPath -Document $recoveryJournal -Security $fileSecurity
        $activeRecovery = Read-BrokerUpgradeRecoveryJournal -Root $root -Requests $requests -Name $Name -Account $Account -UserSid $userSid -Group $Group -SandboxSid $sandboxSid -ExpectedFileSecurity $fileSecurity
        [void](Write-BrokerUpgradeConsumedNonce -Root $root -Nonce ([string]$upgradeAuthority.LauncherNonce) -TransactionId $installId -GoalId $GoalId -SourceFingerprint $SourceFingerprint -FileSecurity $fileSecurity -UserSid $userSid)
    } catch {
        $failure = $_.Exception.Message
        try {
            if ($null -ne $guardState.Stream) {
                $guardState.Stream.Dispose()
                $guardState.Stream = $null
            }
            Remove-BrokerUpgradeCreatedPaths -Root $root -TransactionsPath $transactions -CreatedPaths $createdPaths
        } catch {
            $failure += "; staging_cleanup=$($_.Exception.Message)"
        }
        if ($null -ne $activeRecovery) {
            try {
                Remove-BrokerUpgradeRecoveryJournal -Recovery $activeRecovery
            } catch {
                $failure += "; recovery_journal_cleanup=$($_.Exception.Message)"
            }
        } elseif (Test-Path -LiteralPath $recoveryJournalPath) {
            $failure += '; recovery_journal_cleanup=unbound journal preserved'
        }
        throw "Broker upgrade staging failed before receipt publication: $failure"
    }
    $switchState = [pscustomobject]@{
        NewStartedAt = $null
        RestoredAt = $null
    }
    $releaseGuardAction = {
        if ($null -ne $guardState.Stream) {
            $guardState.Stream.Dispose()
            $guardState.Stream = $null
        }
    }
    $stopOldAction = {
        Stop-ExactBrokerWorkerForUpgrade `
            -Name $Name -Root $root -Receipts @($legacy)
        Wait-BrokerReceiptReplaceReady `
            -Path $receiptPath -TimeoutMilliseconds 5000
    }
    $publishNewReceiptAction = {
        Remove-Item -LiteralPath (Join-Path $results $script:HeartbeatName) `
            -Force -ErrorAction SilentlyContinue
        [void](Publish-BrokerReceiptForward `
            -Path (Join-Path $root $script:ReceiptName) `
            -OldBytes $oldReceiptBytes -NewBytes $stagedReceiptBytes `
            -Security $fileSecurity -OwnerSid $userSid)
    }
    $registerNewTaskAction = {
        [void](Register-BrokerTask -Name $Name -Xml $newTaskXml)
        $taskSecurity = Get-ExpectedTaskSecurityDescriptor `
            -UserSid $userSid -SandboxSid $sandboxSid
        Set-And-AssertBrokerTaskSecurity -Name $Name -ExpectedSddl $taskSecurity `
            -UserSid $userSid -SandboxSid $sandboxSid
    }
    $startNewTaskAction = {
        $switchState.NewStartedAt = [DateTimeOffset]::UtcNow
        Start-BrokerTask -Name $Name
    }
    $waitNewHeartbeatAction = {
        $current = Read-Receipt -Root $root -Requests $requests -Name $Name
        return Wait-BrokerHeartbeat -Root $root -Receipt $current `
            -TimeoutSeconds $script:BrokerHeartbeatStartupTimeoutSeconds `
            -NotBefore $switchState.NewStartedAt -TaskName $Name
    }
    $commitAction = {
        param($Heartbeat)
        $result = [ordered]@{
            upgraded = $true
            from_schema = $script:LegacySchemaVersion
            to_schema = $script:SchemaVersion
            install_id = $installId
            task_name = $Name
            install_root = $root
            repository_root = [string]$manifest.repository_root
            allowed_ref = [string]$manifest.allowed_ref
            user_account = $Account
            user_sid = $userSid
            worker_sha256 = $workerHash
            git_sha256 = [string]$manifest.git_executable_sha256
            git_capability_manifest_sha256 = $manifestHash
            recovered_interrupted_staging = $recoveredInterruptedStaging
            recovered_interrupted_switch = $recoveredInterruptedSwitch
            recovered_missing_results_root = $recoveredMissingResultsRoot
            recovery_journal_retained = $false
            recovery_journal_cleanup_error = $null
            heartbeat_executor = [string]$Heartbeat.executor_account
            heartbeat_sid = [string]$Heartbeat.executor_sid
        }
        Write-JsonAtomic -Path $readyPath -Document $readyDocument `
            -Security $fileSecurity
        $createdPaths.Add((New-BrokerUpgradeOwnedObjectBinding `
            -Path $readyPath -Type file))
        Assert-ExactSecurity -Path $readyPath -Expected $fileSecurity `
            -ExpectedOwnerSid $userSid -Label 'Git capability ready marker'
        try {
            Remove-BrokerUpgradeRecoveryJournal -Recovery $activeRecovery
        } catch {
            $result.recovery_journal_retained = $true
            $result.recovery_journal_cleanup_error = $_.Exception.Message
        }
        return $result
    }
    $stopRollbackAction = {
        Stop-ExactBrokerWorkerForUpgrade `
            -Name $Name -Root $root -Receipts @($receipt, $legacy) `
            -AllowMissingTask
    }
    $restoreOldReceiptAction = {
        [void](Restore-BrokerReceiptFromJournal `
            -Path (Join-Path $root $script:ReceiptName) `
            -OldBytes $oldReceiptBytes -NewBytes $stagedReceiptBytes `
            -Security $fileSecurity -OwnerSid $userSid)
    }
    $registerOldTaskAction = {
        [void](Register-BrokerTask -Name $Name -Xml $oldTaskXml)
        $taskSecurity = Get-ExpectedTaskSecurityDescriptor `
            -UserSid $userSid -SandboxSid $sandboxSid
        Set-And-AssertBrokerTaskSecurity -Name $Name -ExpectedSddl $taskSecurity `
            -UserSid $userSid -SandboxSid $sandboxSid
    }
    $startOldTaskAction = {
        Remove-Item -LiteralPath (Join-Path $results $script:HeartbeatName) `
            -Force -ErrorAction SilentlyContinue
        $switchState.RestoredAt = [DateTimeOffset]::UtcNow
        Start-BrokerTask -Name $Name
    }
    $waitOldHeartbeatAction = {
        $restored = Read-Receipt -Root $root -Requests $requests -Name $Name
        return Wait-BrokerHeartbeat -Root $root -Receipt $restored `
            -TimeoutSeconds $script:BrokerHeartbeatStartupTimeoutSeconds `
            -NotBefore $switchState.RestoredAt -TaskName $Name
    }
    $cleanupStagingAction = {
        Remove-BrokerUpgradeCreatedPaths `
            -Root $root -TransactionsPath $transactions `
            -CreatedPaths $createdPaths
        Remove-BrokerUpgradeRecoveryJournal -Recovery $activeRecovery
    }

    return Invoke-BrokerUpgradeSwitchTransaction `
        -StopOldAction $stopOldAction `
        -PublishNewReceiptAction $publishNewReceiptAction `
        -RegisterNewTaskAction $registerNewTaskAction `
        -StartNewTaskAction $startNewTaskAction `
        -WaitNewHeartbeatAction $waitNewHeartbeatAction `
        -CommitAction $commitAction `
        -ReleaseGuardAction $releaseGuardAction `
        -StopRollbackAction $stopRollbackAction `
        -RestoreOldReceiptAction $restoreOldReceiptAction `
        -RegisterOldTaskAction $registerOldTaskAction `
        -StartOldTaskAction $startOldTaskAction `
        -WaitOldHeartbeatAction $waitOldHeartbeatAction `
        -CleanupStagingAction $cleanupStagingAction
    } finally {
        $resultsGuard.Dispose()
    }
    } finally {
        Close-BrokerUpgradeAuthority -Authority $upgradeAuthority
    }
}

function Uninstall-Broker {
    param([string]$Root, [string]$Requests, [string]$Name, [string]$Account)
    $userSid = Resolve-AccountSid -Account $Account -Label 'Target user'
    [void](Assert-InstallAuthority -ExpectedAccount $Account -ExpectedSid $userSid)
    $root = Assert-RealDirectory -Path $Root -Label 'Broker install root'
    $requests = Assert-RealDirectory -Path $Requests -Label 'Broker request root'
    $receipt = Read-Receipt -Root $root -Requests $requests -Name $Name `
        -SkipTaskValidation -SkipGitLiveValidation
    if ([string]$receipt.user_sid -cne $userSid -or
        [string]$receipt.task_name -cne $Name -or
        [string]$receipt.request_root -cne $requests) {
        throw 'Uninstall arguments do not match the protected install receipt.'
    }
    $upgradeRecovery = Join-Path $root $script:UpgradeRecoveryJournalName
    if (Test-Path -LiteralPath $upgradeRecovery) {
        throw 'Broker uninstall refuses a pending upgrade recovery journal.'
    }
    $transactionGuard = $null
    $transactionLockPath = $null
    if ([int]$receipt.schema_version -eq $script:SchemaVersion) {
        $transactionPath = Join-Path $root $script:GitTransactionDirectoryName
        $transactionLockPath = Get-GitTransactionCleanupLockPath `
            -Path $transactionPath
        $transactionGuard = Open-GitTransactionGuard -Path $transactionLockPath
        try {
            if ((Get-GitTransactionCleanupLockPath -Path $transactionPath) -cne
                $transactionLockPath) {
                throw 'Broker uninstall transaction lock identity changed during preflight.'
            }
        } catch {
            $transactionGuard.Dispose()
            $transactionGuard = $null
            throw
        }
    }
    $workerLockPath = Join-Path $root 'worker.lock'
    $workerBinding = $null
    try {
        if (-not (Test-BrokerWorkerLockReleased -Path $workerLockPath)) {
            $workerBinding = Get-ReceiptBoundWorkerProcess `
                -Root $root -Receipt $receipt
        }
    } catch {
        if ($null -ne $transactionGuard) {
            $transactionGuard.Dispose()
            $transactionGuard = $null
        }
        throw
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
    }
    $ownedTreeState = [pscustomobject]@{ Value = $null }
    $removeTaskAction = {
        param([string]$TaskName)
        [void](Stop-BrokerTaskStrict -Name $TaskName)
        Confirm-BrokerWorkerStoppedForRootRemoval `
            -LockPath $workerLockPath -Binding $workerBinding
        Wait-BrokerWorkerLockReleased -Path $workerLockPath
        if ([int]$receipt.schema_version -eq $script:SchemaVersion) {
            $currentLockPath = Get-GitTransactionCleanupLockPath `
                -Path (Join-Path $root $script:GitTransactionDirectoryName)
            if ($currentLockPath -cne $transactionLockPath) {
                throw 'Broker uninstall transaction directory changed before removal.'
            }
            $transactionGuard.Dispose()
            $transactionGuard = $null
        }
        $ownedTreeState.Value = New-BrokerOwnedTreeSnapshot -Root $root `
            -ExpectedOwnerSid ([string]$receipt.user_sid)
        [void](Remove-BrokerTaskStrict -Name $TaskName)
    }
    $removeRootAction = {
        if (Test-Path -LiteralPath $root -PathType Container) {
            if ($null -eq $ownedTreeState.Value) {
                throw 'Broker uninstall lost its pre-deletion owned-tree snapshot.'
            }
            Remove-BrokerOwnedTreeExact -Snapshot $ownedTreeState.Value
            $ownedTreeState.Value = $null
        }
    }
    try {
        Invoke-BrokerUninstallDestructivePhase -Name $Name `
            -QueryXmlAction $queryXmlAction `
            -ValidateTaskAction $validateTaskAction `
            -RemoveTaskAction $removeTaskAction `
            -RemoveRootAction $removeRootAction
    } finally {
        if ($null -ne $transactionGuard) { $transactionGuard.Dispose() }
        if ($null -ne $workerBinding) {
            $workerBinding.Process.Dispose()
        }
        if ($null -ne $ownedTreeState.Value) {
            Close-BrokerOwnedTreeSnapshot -Snapshot $ownedTreeState.Value
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
    $rootBinding = New-BrokerUpgradeOwnedObjectBinding `
        -Path $root -Type directory
    $lockBinding = New-BrokerUpgradeOwnedObjectBinding `
        -Path $remnant.WorkerLock -Type file
    $removeRootAction = {
        $current = Assert-PartialUninstallRemnant `
            -Root $root -ExpectedRootSecurity $rootSecurity `
            -ExpectedFileSecurity $fileSecurity -UserSid $userSid
        Wait-BrokerWorkerLockReleased -Path $current.WorkerLock
        if ((Get-StrongPathIdentity -Path $root -Directory $true) -cne
                [string]$rootBinding.identity -or
            (Get-StrongPathIdentity -Path $current.WorkerLock `
                -Directory $false) -cne [string]$lockBinding.identity) {
            throw 'Partial uninstall remnant identity changed before cleanup.'
        }
        [void](Remove-BrokerFileExact `
            -Path $current.WorkerLock `
            -ExpectedSha256 ([string]$lockBinding.sha256) `
            -ExpectedIdentity ([string]$lockBinding.identity) `
            -ExpectedOwnerSid ([string]$lockBinding.owner_sid) `
            -ExpectedAccessSddl ([string]$lockBinding.access_sddl) `
            -Label 'Partial uninstall worker lock' -MaximumBytes 64KB)
        [void](Remove-BrokerDirectoryExact `
            -Path $root -ExpectedIdentity ([string]$rootBinding.identity) `
            -ExpectedOwnerSid ([string]$rootBinding.owner_sid) `
            -ExpectedAccessSddl ([string]$rootBinding.access_sddl) `
            -Label 'Partial uninstall root')
    }
    Invoke-AfterBrokerTaskAbsence -Name $Name -Action $removeRootAction
    return [ordered]@{
        recovered_partial_uninstall = $true
        task_name = $Name
        install_root_removed = $true
    }
}

function Invoke-SelfTest {
    Invoke-BrokerPortableStaticSelfTest
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
    $expectedUpgradeSourcePaths = @(
        'AGENTS.md',
        'CLAUDE.md',
        'README.md',
        'crates/rayman/assets/repository-gate-inputs.json',
        'crates/rayman/src/goal/validation/cargo_isolation.rs',
        'crates/rayman/src/goal/validation/pytest_isolation.rs',
        'crates/rayman/tests/audit_script.rs',
        'docs/CODEX_POWERSHELL_BROKER.md',
        'governance/first-party-test-inventory.json',
        'governance/test-traceability.json',
        'scripts/check-agent-instructions.ps1',
        'scripts/codex-powershell-broker.ps1',
        'scripts/install-codex-powershell-broker.ps1'
    )
    if (($script:UpgradeSourcePaths -join [char]10) -cne
        ($expectedUpgradeSourcePaths -join [char]10)) {
        throw 'Upgrade authority source list self-test drifted from the exact ordered 13-path contract.'
    }
    $fingerprintVector = [pscustomobject][ordered]@{
        'b.txt' = '1' * 64
        'a.txt' = '0' * 64
    }
    $fingerprintBinding = @([pscustomobject]@{
        relative_path = 'b.txt'
        sha256 = '2' * 64
    })
    if ((Get-BrokerGoalFingerprintFromBaseline `
            -BaselineFiles $fingerprintVector `
            -SourceBindings $fingerprintBinding) -cne
        '8397f43496fa3754199942d8329f1e21278309f246d06874f1570e6195013e37') {
        throw 'Upgrade launcher Goal fingerprint self-test drifted from the Rust contract.'
    }
    $scopeProbePath = $PSCommandPath
    $scopeProbe = [pscustomobject]@{ Hash = $null; Released = $false }
    $scopeProbeResult = Invoke-BrokerUpgradeSwitchTransaction `
        -StopOldAction {
            $scopeProbe.Hash = Get-FileSha256 -Path $scopeProbePath
        } `
        -PublishNewReceiptAction { } `
        -RegisterNewTaskAction { } `
        -StartNewTaskAction { } `
        -WaitNewHeartbeatAction { return [pscustomobject]@{ ok = $true } } `
        -CommitAction {
            param($Heartbeat)
            return [pscustomobject]@{ committed = [bool]$Heartbeat.ok }
        } `
        -ReleaseGuardAction { $scopeProbe.Released = $true } `
        -StopRollbackAction { } `
        -RestoreOldReceiptAction { } `
        -RegisterOldTaskAction { } `
        -StartOldTaskAction { } `
        -WaitOldHeartbeatAction { return [pscustomobject]@{ ok = $true } } `
        -CleanupStagingAction { }
    if ([string]$scopeProbe.Hash -cne (Get-FileSha256 -Path $PSCommandPath) -or
        -not [bool]$scopeProbe.Released -or
        -not [bool]$scopeProbeResult.committed) {
        throw 'Upgrade action scope self-test lost script functions or caller locals.'
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
    Assert-BrokerExplicitConfirmation -Confirmed $true `
        -Action 'Broker confirmation self-test'
    foreach ($action in @(
        'Broker install', 'Broker upgrade', 'Broker uninstall',
        'Broker partial-uninstall recovery',
        'Broker install launcher preparation',
        'Broker upgrade launcher preparation'
    )) {
        & $assertRejected {
            Assert-BrokerExplicitConfirmation -Confirmed $false `
                -Action $action
        } "explicit false confirmation: $action"
    }
    $taskComState = [pscustomobject]@{
        RegisterCalls = 0
        RunCalls = 0
        Name = $null
        Xml = $null
        Flags = $null
        LogonType = $null
    }
    $fakeRegisteredTask = [pscustomobject]@{
        Path = '\Rayman-CodexPowerShellBroker'
    }
    $fakeRegisteredTask | Add-Member -MemberType ScriptMethod -Name Run `
        -Value ({
            param($Parameters)
            $taskComState.RunCalls++
            return [pscustomobject]@{ InstanceGuid = [Guid]::NewGuid() }
        }.GetNewClosure())
    $fakeFolder = [pscustomobject]@{}
    $fakeFolder | Add-Member -MemberType ScriptMethod -Name RegisterTask `
        -Value ({
            param(
                [string]$Name, [string]$Xml, [int]$Flags,
                $UserId, $Password, [int]$LogonType, $Sddl
            )
            $taskComState.RegisterCalls++
            $taskComState.Name = $Name
            $taskComState.Xml = $Xml
            $taskComState.Flags = $Flags
            $taskComState.LogonType = $LogonType
            return $fakeRegisteredTask
        }.GetNewClosure())
    $fakeContext = [pscustomobject]@{
        Folder = $fakeFolder
        Task = $fakeRegisteredTask
    }
    $fakeXml = '<Task version="1.4" />'
    [void](Register-BrokerTaskWithContext -Context $fakeContext `
        -Name '\Rayman-CodexPowerShellBroker' -Xml $fakeXml)
    [void](Start-BrokerTaskWithContext -Context $fakeContext `
        -Name '\Rayman-CodexPowerShellBroker')
    if ($taskComState.RegisterCalls -ne 1 -or
        $taskComState.RunCalls -ne 1 -or
        [string]$taskComState.Name -cne 'Rayman-CodexPowerShellBroker' -or
        [string]$taskComState.Xml -cne $fakeXml -or
        [int]$taskComState.Flags -ne 6 -or
        [int]$taskComState.LogonType -ne 3) {
        throw 'Task Scheduler COM registration/start self-test failed.'
    }
    $runUpgradeSwitchProbe = {
        param([string]$FailPhase)
        $state = [pscustomobject]@{
            Fail = $FailPhase
            FailedOnce = $false
            GuardHeld = $true
            Log = [Collections.Generic.List[string]]::new()
        }
        $record = {
            param([string]$Name)
            $state.Log.Add($Name)
            if ([string]$state.Fail -ceq $Name -and -not $state.FailedOnce) {
                $state.FailedOnce = $true
                throw "injected upgrade switch failure: $Name"
            }
        }.GetNewClosure()
        $stopOld = { & $record 'stop_old' }.GetNewClosure()
        $publishNew = { & $record 'publish_new_receipt' }.GetNewClosure()
        $registerNew = { & $record 'register_new_task' }.GetNewClosure()
        $startNew = { & $record 'start_new_task' }.GetNewClosure()
        $waitNew = {
            & $record 'wait_new_heartbeat'
            return [pscustomobject]@{ executor_sid = 'new' }
        }.GetNewClosure()
        $releaseGuard = {
            & $record 'release_guard'
            $state.GuardHeld = $false
        }.GetNewClosure()
        $commit = {
            param($Heartbeat)
            & $record 'commit_ready_marker'
            return [pscustomobject]@{
                status = 'success'; heartbeat = [string]$Heartbeat.executor_sid
            }
        }.GetNewClosure()
        $stopRollback = { $state.Log.Add('stop_rollback') }.GetNewClosure()
        $restoreReceipt = { $state.Log.Add('restore_old_receipt') }.GetNewClosure()
        $registerOld = { $state.Log.Add('register_old_task') }.GetNewClosure()
        $startOld = { $state.Log.Add('start_old_task') }.GetNewClosure()
        $waitOld = {
            $state.Log.Add('wait_old_heartbeat')
            return [pscustomobject]@{ executor_sid = 'old' }
        }.GetNewClosure()
        $cleanup = {
            if ($state.GuardHeld) {
                throw 'upgrade switch cleanup ran before transaction guard release'
            }
            $state.Log.Add('cleanup_staging')
        }.GetNewClosure()
        try {
            $result = Invoke-BrokerUpgradeSwitchTransaction `
                -StopOldAction $stopOld `
                -PublishNewReceiptAction $publishNew `
                -RegisterNewTaskAction $registerNew `
                -StartNewTaskAction $startNew `
                -WaitNewHeartbeatAction $waitNew `
                -CommitAction $commit -ReleaseGuardAction $releaseGuard `
                -StopRollbackAction $stopRollback `
                -RestoreOldReceiptAction $restoreReceipt `
                -RegisterOldTaskAction $registerOld `
                -StartOldTaskAction $startOld `
                -WaitOldHeartbeatAction $waitOld `
                -CleanupStagingAction $cleanup
            return [pscustomobject]@{
                Result = $result; Error = $null; Log = @($state.Log)
            }
        } catch {
            return [pscustomobject]@{
                Result = $null; Error = $_.Exception.Message; Log = @($state.Log)
            }
        }
    }
    $switchForward = @(
        'stop_old', 'publish_new_receipt', 'register_new_task',
        'start_new_task', 'wait_new_heartbeat', 'release_guard',
        'commit_ready_marker'
    )
    $switchRollback = @(
        'stop_rollback', 'restore_old_receipt', 'register_old_task',
        'start_old_task', 'wait_old_heartbeat', 'release_guard',
        'cleanup_staging'
    )
    $switchSuccess = & $runUpgradeSwitchProbe ''
    if ($switchSuccess -is [array] -or
        $null -eq $switchSuccess.Result -or
        $switchSuccess.Result.PSObject.Properties.Name -notcontains 'status') {
        $switchTypes = @($switchSuccess | ForEach-Object {
            if ($null -eq $_) { '<null>' } else { $_.GetType().FullName }
        }) -join ','
        $resultTypes = @($switchSuccess | ForEach-Object {
            if ($null -eq $_ -or $null -eq $_.Result) { '<no-result>' }
            else { $_.Result.GetType().FullName }
        }) -join ','
        $probeError = @($switchSuccess | ForEach-Object { [string]$_.Error }) -join ';'
        $probeLog = @($switchSuccess | ForEach-Object { @($_.Log) }) -join ','
        throw "Upgrade switch transaction returned an invalid result shape: switch_types=$switchTypes result_types=$resultTypes error=$probeError log=$probeLog"
    }
    if ([string]$switchSuccess.Result.status -cne 'success' -or
        [string]$switchSuccess.Result.heartbeat -cne 'new' -or
        (@($switchSuccess.Log) -join "`n") -cne ($switchForward -join "`n")) {
        throw 'Upgrade switch transaction success ordering self-test failed.'
    }
    foreach ($failurePhase in $switchForward) {
        $switchFailure = & $runUpgradeSwitchProbe $failurePhase
        $failureIndex = [Array]::IndexOf($switchForward, $failurePhase)
        $expectedLog = @($switchForward[0..$failureIndex] + $switchRollback)
        if ([string]$switchFailure.Error -notlike
                '*the exact schema-v2 task/receipt/heartbeat were restored*' -or
            (@($switchFailure.Log) -join "`n") -cne ($expectedLog -join "`n")) {
            throw "Upgrade switch transaction rollback ordering self-test failed: $failurePhase"
        }
    }
    $rollbackFailureState = [Collections.Generic.List[string]]::new()
    $rollbackFailure = $null
    try {
        [void](Invoke-BrokerUpgradeSwitchTransaction `
            -StopOldAction { throw 'injected forward failure' } `
            -PublishNewReceiptAction { } -RegisterNewTaskAction { } `
            -StartNewTaskAction { } -WaitNewHeartbeatAction { } `
            -CommitAction { param($Heartbeat) } `
            -ReleaseGuardAction { $rollbackFailureState.Add('release_guard') } `
            -StopRollbackAction { $rollbackFailureState.Add('stop_rollback') } `
            -RestoreOldReceiptAction { throw 'injected rollback failure' } `
            -RegisterOldTaskAction { } -StartOldTaskAction { } `
            -WaitOldHeartbeatAction { } `
            -CleanupStagingAction { $rollbackFailureState.Add('cleanup') })
    } catch { $rollbackFailure = $_.Exception.Message }
    if ([string]$rollbackFailure -notlike '*rollback could not be proven*' -or
        (@($rollbackFailureState) -join "`n") -cne
            (@('stop_rollback', 'release_guard') -join "`n")) {
        throw 'Upgrade switch transaction rollback-failure self-test failed.'
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

    $extraFrameScope = & {
        $callerLocal = [pscustomobject]@{
            AbsenceCalls = 0
            QueryCalls = 0
            ValidateCalls = 0
            RemoveTaskCalls = 0
            LookupCalls = 0
            RemoveRootCalls = 0
            InstallerHash = $null
        }
        $extraFrameLookup = {
            param([string]$TaskName)
            $callerLocal.LookupCalls++
            [pscustomobject]@{ Task = $null }
        }
        $extraFrameAbsenceAction = {
            $callerLocal.AbsenceCalls++
            $callerLocal.InstallerHash = Get-FileSha256 -Path $PSCommandPath
        }
        Invoke-AfterBrokerTaskAbsence -Name $testTaskName `
            -LookupAction $extraFrameLookup -Action $extraFrameAbsenceAction
        $extraFrameQuery = {
            param([string]$TaskName)
            $callerLocal.QueryCalls++
            '<Task />'
        }
        $extraFrameValidate = {
            param([string]$TaskXml)
            $callerLocal.ValidateCalls++
        }
        $extraFrameRemoveTask = {
            param([string]$TaskName)
            $callerLocal.RemoveTaskCalls++
        }
        $extraFrameRemoveRoot = {
            $callerLocal.RemoveRootCalls++
            $callerLocal.InstallerHash = Get-FileSha256 -Path $PSCommandPath
        }
        Invoke-BrokerUninstallDestructivePhase -Name $testTaskName `
            -QueryXmlAction $extraFrameQuery `
            -ValidateTaskAction $extraFrameValidate `
            -RemoveTaskAction $extraFrameRemoveTask `
            -LookupAction $extraFrameLookup `
            -RemoveRootAction $extraFrameRemoveRoot
        $callerLocal
    }
    if ($extraFrameScope.AbsenceCalls -ne 1 -or
        $extraFrameScope.QueryCalls -ne 1 -or
        $extraFrameScope.ValidateCalls -ne 1 -or
        $extraFrameScope.RemoveTaskCalls -ne 1 -or
        $extraFrameScope.LookupCalls -ne 2 -or
        $extraFrameScope.RemoveRootCalls -ne 1 -or
        [string]$extraFrameScope.InstallerHash -cne
            (Get-FileSha256 -Path $PSCommandPath)) {
        throw 'Installer extra-frame actions lost production functions or caller locals.'
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
        "[ValidateSet('identity_probe', 'git_local_commit_v1')]",
        'Arbitrary commands are never accepted.',
        'Open-ExclusiveRequest',
        'Assert-InstalledAclContract',
        'persistent_install_grant',
        'RunSingleProcess',
        'HeldRegularFile',
        'ReadBoundedRegularFileTreeHeld',
        'TerminateProcess(process.hProcess, 1)',
        'JOB_OBJECT_LIMIT_ACTIVE_PROCESS',
        'hash-filtered-blob',
        'function Read-GitCapabilityReady',
        'requires info/attributes to remain absent',
        'index backup cannot prove the replaced live index',
        'git_local_commit_v1 CRLF self-test did not bind the filtered index blob OID.',
        "'update-ref' { @('update-ref', '--no-deref', '--stdin') }",
        'transaction recovery is indeterminate; preserved journal requires recovery',
        'update-ref',
        'GIT_NO_LAZY_FETCH',
        'git_local_commit_v1 requires the agent-side standing-authority assertion -Yes',
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
    foreach ($forbidden in @(
        'Invoke-Expression', "'push' {", "'fetch' {", '--no-verify',
        'danger-full-access', 'unelevated'
    )) {
        if ($workerSource.Contains($forbidden)) {
            throw "Broker source self-test found a forbidden execution boundary: $forbidden"
        }
    }
    $runtime = Get-CurrentPowerShellRuntime
    if ($runtime.Path -cne [Diagnostics.Process]::GetCurrentProcess().MainModule.FileName -or
        $runtime.Sha256 -cne (Get-FileSha256 -Path $runtime.Path)) {
        throw 'Installer did not bind exactly one current PowerShell runtime.'
    }
    $manifestProbe = New-GitCapabilityManifest `
        -InstallId ([Guid]::NewGuid().ToString('N')) `
        -HooksRoot 'C:\ProgramData\Rayman\CodexPowerShellBroker\empty-hooks' `
        -TransactionsRoot 'C:\ProgramData\Rayman\CodexPowerShellBroker\git-local-commit-transactions'
    if ([string]$manifestProbe.repository_root -cne $script:RepositoryRoot -or
        [string]$manifestProbe.allowed_ref -cne 'refs/heads/main' -or
        -not ([string]$manifestProbe.git_executable_path).EndsWith(
            '\mingw64\bin\git.exe', [StringComparison]::OrdinalIgnoreCase
        ) -or
        [string]$manifestProbe.git_executable_sha256 -notmatch '^[0-9a-f]{64}$' -or
        [bool]$manifestProbe.local_commit_only -ne $true -or
        [bool]$manifestProbe.push_allowed -ne $false -or
        [bool]$manifestProbe.confirmation_required -ne $false -or
        $null -ne $manifestProbe.info_attributes_sha256 -or
        [string]$manifestProbe.authorization_mode -cne 'persistent_install_grant') {
        throw 'Installer git_local_commit_v1 manifest self-test failed.'
    }
    $manifestProbeDocument = $manifestProbe | ConvertTo-Json -Depth 12 -Compress |
        ConvertFrom-Json -Depth 12
    [void](Assert-GitCapabilityManifestInstalled `
        -Manifest $manifestProbeDocument `
        -Receipt ([pscustomobject]@{ install_id = [string]$manifestProbe.install_id }) `
        -Root 'C:\ProgramData\Rayman\CodexPowerShellBroker' `
        -SkipLiveRepository)
    $forgedInfoAttributes = $manifestProbeDocument | ConvertTo-Json -Depth 12 -Compress |
        ConvertFrom-Json -Depth 12
    $forgedInfoAttributes.info_attributes_sha256 = '0' * 64
    & $assertRejected {
        [void](Assert-GitCapabilityManifestInstalled `
            -Manifest $forgedInfoAttributes `
            -Receipt ([pscustomobject]@{
                install_id = [string]$forgedInfoAttributes.install_id
            }) `
            -Root 'C:\ProgramData\Rayman\CodexPowerShellBroker' `
            -SkipLiveRepository)
    } 'info/attributes manifest registration'
    $installerSource = Get-Content -Raw -LiteralPath $PSCommandPath
    foreach ($required in @(
        'function Upgrade-Broker',
        'function Upgrade-BrokerCurrentSchema',
        'function Clear-ExactInterruptedUpgradeStaging',
        'function Stop-ExactBrokerWorkerForUpgrade',
        'function Read-BrokerUpgradeRecoveryJournal',
        'function Invoke-BrokerInterruptedUpgradeRecovery',
        'function Invoke-BrokerInterruptedCurrentSchemaUpgradeRecovery',
        'function Ensure-BrokerUpgradeResultsRoot',
        'OpenDirectoryGuard',
        'held upgrade result root removal',
        'function Publish-BrokerReceiptForward',
        'function Restore-BrokerReceiptFromJournal',
        'function Get-BrokerReceiptFileState',
        'function Publish-BrokerReadyForward',
        'function Restore-BrokerReadyFromJournal',
        'function Resolve-BrokerUpgradeRecoveryDisposition',
        'function Open-BrokerInstallationGuard',
        'function Open-BrokerUpgradeAuthority',
        'function Assert-BrokerUpgradeTransactionLockReady',
        'function New-BrokerUpgradeOwnedFileBindingFromHeldStream',
        'function Get-BrokerTaskRuntimeDiagnostic',
        'function New-BrokerUpgradeUserCommandText',
        'function Remove-BrokerFileExact',
        'mutable_launcher_file_published = $false',
        'transaction_in_progress',
        'transaction_lock_preflight',
        'last_heartbeat_error=',
        'BrokerHeartbeatStartupTimeoutSeconds = 60',
        'CurrentUpgradeRecoveryJournalSchemaVersion = 3',
        'recovery_journal_state',
        'RecoverySelfTestChild',
        'ready_published',
        'rolled_back_v3',
        'ProbeFileReplace',
        'held receipt replace probe',
        'function Invoke-BrokerUpgradeSwitchTransaction',
        'function Assert-GitCapabilityReadyInstalled',
        'function Assert-GitTransactionDirectorySafeForInstallerCleanup',
        'the exact schema-v2 task/receipt/heartbeat were restored',
        'git_capability_manifest_sha256',
        "capabilities = @('identity_probe', `$script:GitCapabilityId)"
    )) {
        if (-not $installerSource.Contains($required)) {
            throw "Installer upgrade/manifest self-test is missing: $required"
        }
    }
    $selfTestSourceStart = $installerSource.IndexOf(
        'function Invoke-SelfTest', [StringComparison]::Ordinal
    )
    if ($selfTestSourceStart -le 0 -or
        $installerSource.Substring(0, $selfTestSourceStart).Contains(
            '.GetNewClosure()'
        )) {
        throw 'All production installer actions must retain caller script scope.'
    }
    $authorityStart = $installerSource.IndexOf(
        'function Open-BrokerUpgradeAuthority',
        [StringComparison]::Ordinal
    )
    $authorityEnd = $installerSource.IndexOf(
        'function Close-BrokerUpgradeAuthority',
        [StringComparison]::Ordinal
    )
    if ($authorityStart -lt 0 -or $authorityEnd -le $authorityStart) {
        throw 'Elevated upgrade authority source boundary is missing.'
    }
    $authoritySource = $installerSource.Substring(
        $authorityStart, $authorityEnd - $authorityStart
    )
    if ($authoritySource.Contains('Invoke-BrokerBoundGoalCli') -or
        $authoritySource.Contains("['rayman']") -or
        $authoritySource.Contains('$manifest.' + 'rayman')) {
        throw 'Elevated upgrade authority must not execute a workspace Rayman binary.'
    }
    $upgradeStart = $installerSource.IndexOf(
        'function Upgrade-Broker {', [StringComparison]::Ordinal
    )
    $upgradeEnd = $installerSource.IndexOf(
        'function Uninstall-Broker', [StringComparison]::Ordinal
    )
    if ($upgradeStart -lt 0 -or $upgradeEnd -le $upgradeStart) {
        throw 'Production upgrade source boundary is missing.'
    }
    $upgradeSource = $installerSource.Substring(
        $upgradeStart, $upgradeEnd - $upgradeStart
    )
    $initialProbeIndex = $upgradeSource.IndexOf(
        'Broker upgrade receipt capability preflight failed before formal writes',
        [StringComparison]::Ordinal
    )
    $journalPublishIndex = $upgradeSource.IndexOf(
        'Write-JsonAtomic -Path $recoveryJournalPath',
        [StringComparison]::Ordinal
    )
    $stopActionIndex = $upgradeSource.IndexOf(
        '$stopOldAction = {', [StringComparison]::Ordinal
    )
    if ($initialProbeIndex -lt 0 -or $journalPublishIndex -le $initialProbeIndex -or
        $stopActionIndex -le $journalPublishIndex) {
        throw 'Upgrade order must be initial probe, durable journal, then old-worker stop.'
    }
    $currentUpgradeStart = $installerSource.IndexOf(
        'function Upgrade-BrokerCurrentSchema {',
        [StringComparison]::Ordinal
    )
    if ($currentUpgradeStart -lt 0 -or
        $currentUpgradeStart -ge $upgradeStart) {
        throw 'Current-schema production upgrade source boundary is missing.'
    }
    $currentUpgradeSource = $installerSource.Substring(
        $currentUpgradeStart, $upgradeStart - $currentUpgradeStart
    )
    $currentProbeIndex = $currentUpgradeSource.IndexOf(
        'Wait-BrokerReceiptReplaceReady -Path $receiptPath',
        [StringComparison]::Ordinal
    )
    $currentJournalIndex = $currentUpgradeSource.IndexOf(
        'Write-JsonAtomic -Path $recoveryJournalPath',
        [StringComparison]::Ordinal
    )
    $currentStopIndex = $currentUpgradeSource.IndexOf(
        '$stopOldAction = {', [StringComparison]::Ordinal
    )
    if ($currentProbeIndex -lt 0 -or
        $currentJournalIndex -le $currentProbeIndex -or
        $currentStopIndex -le $currentJournalIndex) {
        throw 'Current-schema upgrade order must be initial probe, durable journal, then old-worker stop.'
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
    $managedRoot = Get-BrokerSelfTestManagedRoot
    $testRoot = Get-BrokerSelfTestCaseRoot -ManagedRoot $managedRoot `
        -Token ([Guid]::NewGuid().ToString('N')) `
        -Label 'Installer self-test root'
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
        $committedSourceRoot = Join-Path $testRoot 'committed-source-selftest'
        [void][IO.Directory]::CreateDirectory($committedSourceRoot)
        $committedSourceAttributes = Join-Path $committedSourceRoot '.gitattributes'
        $committedSourcePath = Join-Path $committedSourceRoot 'source.txt'
        [IO.File]::WriteAllText(
            $committedSourceAttributes, "* text=auto eol=lf`n",
            [Text.UTF8Encoding]::new($false, $true)
        )
        [IO.File]::WriteAllText(
            $committedSourcePath, "committed`n",
            [Text.UTF8Encoding]::new($false, $true)
        )
        $fixtureGit = Get-RegisteredGitExecutable
        $invokeCommittedSourceGit = {
            param([Parameter(Mandatory = $true)][object[]]$Arguments)
            $output = @(& $fixtureGit.Path @Arguments 2>&1)
            if ($LASTEXITCODE -ne 0) {
                throw "Committed-source self-test Git failed: $($output -join ' | ')"
            }
            return ,$output
        }
        [void](& $invokeCommittedSourceGit -Arguments @(
            'init', '-b', 'main', $committedSourceRoot
        ))
        [void](& $invokeCommittedSourceGit -Arguments @(
            '-C', $committedSourceRoot, 'add', '--',
            '.gitattributes', 'source.txt'
        ))
        [void](& $invokeCommittedSourceGit -Arguments @(
            '-C', $committedSourceRoot,
            '-c', 'user.name=rayman-selftest',
            '-c', 'user.email=rayman-selftest@example.invalid',
            'commit', '-m', 'committed-source baseline'
        ))
        $committedSourceHead = @(& $fixtureGit.Path -C $committedSourceRoot `
            rev-parse HEAD)
        if ($LASTEXITCODE -ne 0 -or $committedSourceHead.Count -ne 1 -or
            [string]$committedSourceHead[0].Trim() -notmatch '^[0-9a-f]{40}$') {
            throw 'Committed-source self-test could not bind HEAD.'
        }
        $committedSourceHead = [string]$committedSourceHead[0].Trim()
        $committedSourceBytes = [IO.File]::ReadAllBytes($committedSourcePath)
        $committedSource = Get-BrokerCommittedSourceBinding `
            -GitPath $fixtureGit.Path -RepositoryRoot $committedSourceRoot `
            -ExpectedHead $committedSourceHead -RelativePath 'source.txt' `
            -Bytes $committedSourceBytes
        if ([string]$committedSource.Mode -cne '100644' -or
            [string]$committedSource.BlobOid -cne
                (Get-BrokerGitBlobOid -Bytes $committedSourceBytes)) {
            throw 'Committed-source self-test did not bind the exact HEAD blob.'
        }
        [void](& $invokeCommittedSourceGit -Arguments @(
            '-C', $committedSourceRoot, 'update-index',
            '--assume-unchanged', '--', 'source.txt'
        ))
        [IO.File]::WriteAllText(
            $committedSourcePath, "hidden-assume`n",
            [Text.UTF8Encoding]::new($false, $true)
        )
        & $assertRejected {
            [void](Get-BrokerCommittedSourceBinding `
                -GitPath $fixtureGit.Path -RepositoryRoot $committedSourceRoot `
                -ExpectedHead $committedSourceHead -RelativePath 'source.txt' `
                -Bytes ([IO.File]::ReadAllBytes($committedSourcePath)))
        } 'committed-source assume-unchanged flag'
        [void](& $invokeCommittedSourceGit -Arguments @(
            '-C', $committedSourceRoot, 'update-index',
            '--no-assume-unchanged', '--', 'source.txt'
        ))
        [IO.File]::WriteAllText(
            $committedSourcePath, "committed`n",
            [Text.UTF8Encoding]::new($false, $true)
        )
        [void](& $invokeCommittedSourceGit -Arguments @(
            '-C', $committedSourceRoot, 'update-index',
            '--skip-worktree', '--', 'source.txt'
        ))
        [IO.File]::WriteAllText(
            $committedSourcePath, "hidden-skip`n",
            [Text.UTF8Encoding]::new($false, $true)
        )
        & $assertRejected {
            [void](Get-BrokerCommittedSourceBinding `
                -GitPath $fixtureGit.Path -RepositoryRoot $committedSourceRoot `
                -ExpectedHead $committedSourceHead -RelativePath 'source.txt' `
                -Bytes ([IO.File]::ReadAllBytes($committedSourcePath)))
        } 'committed-source skip-worktree flag'
        [void](& $invokeCommittedSourceGit -Arguments @(
            '-C', $committedSourceRoot, 'update-index',
            '--no-skip-worktree', '--', 'source.txt'
        ))
        [IO.File]::WriteAllText(
            $committedSourcePath, "staged-drift`n",
            [Text.UTF8Encoding]::new($false, $true)
        )
        [void](& $invokeCommittedSourceGit -Arguments @(
            '-C', $committedSourceRoot, 'add', '--', 'source.txt'
        ))
        & $assertRejected {
            [void](Get-BrokerCommittedSourceBinding `
                -GitPath $fixtureGit.Path -RepositoryRoot $committedSourceRoot `
                -ExpectedHead $committedSourceHead -RelativePath 'source.txt' `
                -Bytes ([IO.File]::ReadAllBytes($committedSourcePath)))
        } 'committed-source staged divergence'

        $ownedTreeRoot = Join-Path $testRoot 'owned-tree-selftest'
        $ownedTreeChild = Join-Path $ownedTreeRoot 'child'
        $ownedTreeFile = Join-Path $ownedTreeChild 'payload.json'
        [void](New-ManagedDirectory -Path $ownedTreeRoot `
            -Security $readOnlySecurity -OwnerSid $testUserSid `
            -Label 'Owned-tree self-test root')
        [void](New-ManagedDirectory -Path $ownedTreeChild `
            -Security $readOnlySecurity -OwnerSid $testUserSid `
            -Label 'Owned-tree self-test child')
        Write-BytesAtomic -Path $ownedTreeFile `
            -Bytes ([Text.UTF8Encoding]::new($false, $true).GetBytes('first')) `
            -Security $fileSecurity
        $staleOwnedTree = New-BrokerOwnedTreeSnapshot `
            -Root $ownedTreeRoot -ExpectedOwnerSid $testUserSid
        Write-BytesAtomic -Path $ownedTreeFile `
            -Bytes ([Text.UTF8Encoding]::new($false, $true).GetBytes('replacement')) `
            -Replace -Security $fileSecurity
        & $assertRejected {
            Remove-BrokerOwnedTreeExact -Snapshot $staleOwnedTree
        } 'owned-tree concurrent file replacement'
        if (-not (Test-Path -LiteralPath $ownedTreeRoot -PathType Container)) {
            throw 'Owned-tree self-test removed a replacement after binding drift.'
        }
        $ownedTreeExternal = Join-Path $testRoot 'owned-tree-external'
        $ownedTreeExternalSentinel = Join-Path $ownedTreeExternal 'sentinel.txt'
        [void](New-ManagedDirectory -Path $ownedTreeExternal `
            -Security $readOnlySecurity -OwnerSid $testUserSid `
            -Label 'Owned-tree external sentinel root')
        Write-BytesAtomic -Path $ownedTreeExternalSentinel `
            -Bytes ([Text.UTF8Encoding]::new($false, $true).GetBytes('outside')) `
            -Security $fileSecurity
        $currentOwnedTree = New-BrokerOwnedTreeSnapshot `
            -Root $ownedTreeRoot -ExpectedOwnerSid $testUserSid
        $ownedTreeRenameBlocked = $false
        try {
            [IO.Directory]::Move(
                $ownedTreeChild,
                (Join-Path $ownedTreeRoot 'child-replacement-window')
            )
        } catch [IO.IOException] {
            $ownedTreeRenameBlocked = $true
        }
        if (-not $ownedTreeRenameBlocked) {
            throw 'Owned-tree retained descendant handle allowed an ancestor rename/replacement window.'
        }
        Remove-BrokerOwnedTreeExact -Snapshot $currentOwnedTree
        if (Test-Path -LiteralPath $ownedTreeRoot) {
            throw 'Owned-tree exact deletion self-test retained its root.'
        }
        if (-not (Test-Path -LiteralPath $ownedTreeExternalSentinel -PathType Leaf)) {
            throw 'Owned-tree exact deletion crossed into the external sentinel.'
        }

        $launcherTestRoot = Join-Path $testRoot 'launcher-host-selftest'
        if ($launcherTestRoot.StartsWith('\\?\', [StringComparison]::Ordinal)) {
            $launcherTestRoot = $launcherTestRoot.Substring(4)
        }
        [void](New-ManagedDirectory -Path $launcherTestRoot -Security $readOnlySecurity -OwnerSid $testUserSid -Label 'Launcher host self-test root')

        if ((Assert-BrokerUpgradeTransactionLockReady `
                -Root (Join-Path $testRoot 'absent-install-root')) -cne
                'not_present') {
            throw 'Fresh-install transaction preflight did not accept an absent root.'
        }
        $transactionPreflightRoot = Join-Path $testRoot 'transaction-preflight'
        [void](New-ManagedDirectory -Path $transactionPreflightRoot `
            -Security $readOnlySecurity -OwnerSid $testUserSid `
            -Label 'Transaction preflight self-test root')
        $transactionPreflightDirectory = Join-Path $transactionPreflightRoot `
            $script:GitTransactionDirectoryName
        [void](New-ManagedDirectory -Path $transactionPreflightDirectory `
            -Security $readOnlySecurity -OwnerSid $testUserSid `
            -Label 'Transaction preflight self-test directory')
        $transactionPreflightLock = Join-Path $transactionPreflightDirectory `
            'transaction.lock'
        Write-BytesAtomic -Path $transactionPreflightLock `
            -Bytes ([byte[]]::new(0)) -Security $fileSecurity
        if ((Assert-BrokerUpgradeTransactionLockReady `
                -Root $transactionPreflightRoot) -cne 'available') {
            throw 'Transaction lock preflight did not report an available lock.'
        }
        $heldTransactionPreflight = [IO.FileStream]::new(
            $transactionPreflightLock, [IO.FileMode]::Open,
            [IO.FileAccess]::ReadWrite, [IO.FileShare]::None
        )
        try {
            $transactionPreflightError = $null
            try {
                [void](Assert-BrokerUpgradeTransactionLockReady `
                    -Root $transactionPreflightRoot `
                    -RetryTimeoutMilliseconds 50)
            } catch { $transactionPreflightError = $_.Exception.Message }
            if ($null -eq $transactionPreflightError -or
                -not $transactionPreflightError.Contains(
                    'Upgrade preparation is blocked by an active Git transaction',
                    [StringComparison]::Ordinal
                ) -or $transactionPreflightError -notmatch 'win32=(32|33)') {
                throw "Transaction lock preflight diagnostic drifted: $transactionPreflightError"
            }
        } finally { $heldTransactionPreflight.Dispose() }

        $heartbeatTimeoutRoot = Join-Path $testRoot 'heartbeat-timeout'
        [void](New-ManagedDirectory -Path $heartbeatTimeoutRoot `
            -Security $readOnlySecurity -OwnerSid $testUserSid `
            -Label 'Heartbeat timeout self-test root')
        $heartbeatTimeoutResults = Join-Path $heartbeatTimeoutRoot 'results'
        [void](New-ManagedDirectory -Path $heartbeatTimeoutResults `
            -Security $readOnlySecurity -OwnerSid $testUserSid `
            -Label 'Heartbeat timeout self-test results')
        Write-JsonAtomic -Path (Join-Path $heartbeatTimeoutResults `
            $script:HeartbeatName) -Security $fileSecurity `
            -Document ([ordered]@{ schema_version = $script:SchemaVersion })
        $heartbeatTimeoutError = $null
        try {
            [void](Wait-BrokerHeartbeat -Root $heartbeatTimeoutRoot `
                -Receipt ([pscustomobject]@{
                    schema_version = $script:SchemaVersion
                    install_id = 'self-test-install'
                    user_account = 'self-test-account'
                    user_sid = $testUserSid
                    worker_sha256 = 'a' * 64
                    powershell_sha256 = 'b' * 64
                }) -TimeoutSeconds 0)
        } catch { $heartbeatTimeoutError = $_.Exception.Message }
        if ($null -eq $heartbeatTimeoutError -or
            -not $heartbeatTimeoutError.Contains(
                'timeout_seconds=0', [StringComparison]::Ordinal
            ) -or
            -not $heartbeatTimeoutError.Contains(
                'last_heartbeat_error=', [StringComparison]::Ordinal
            ) -or
            -not $heartbeatTimeoutError.Contains(
                'task_runtime=', [StringComparison]::Ordinal
            )) {
            throw "Heartbeat timeout diagnostic self-test drifted: $heartbeatTimeoutError"
        }

        $launcherManifestPath = Join-Path $launcherTestRoot 'authority.json'
        $launcherManifestBytes = [Text.UTF8Encoding]::new($false, $true).GetBytes(
            '{"schema_version":1,"nonce":"0123456789abcdef0123456789abcdef"}'
        )
        Write-BytesAtomic -Path $launcherManifestPath -Bytes $launcherManifestBytes -Security $fileSecurity
        $launcherManifestHash = Get-BytesSha256 -Bytes $launcherManifestBytes

        $replaceHelperPath = Join-Path $launcherTestRoot 'replace-helper.ps1'
        $replaceHelperText = @'
param(
    [Parameter(Mandatory = $true)][string]$Target,
    [Parameter(Mandatory = $true)][string]$Replacement,
    [switch]$Directory
)
try {
    if ($Directory) {
        [IO.Directory]::Move($Target, $Replacement)
    } else {
        [IO.File]::Move($Replacement, $Target, $true)
    }
    'unexpected_success'
} catch {
    $current = $_.Exception
    $native = $null
    while ($null -ne $current) {
        $candidate = $current.HResult -band 0xFFFF
        if ($candidate -in @(5, 32)) {
            $native = $candidate
            break
        }
        $current = $current.InnerException
    }
    if ($null -eq $native) {
        'blocked_unclassified:' + $_.Exception.Message
    } else {
        'blocked:' + $native
    }
}
'@
        Write-BytesAtomic -Path $replaceHelperPath -Bytes (
            [Text.UTF8Encoding]::new($false, $true).GetBytes($replaceHelperText)
        ) -Security $fileSecurity

        $fakeInstallerPath = Join-Path $launcherTestRoot 'fake-installer.ps1'
        $fakeInstallerResult = Join-Path $launcherTestRoot 'fake-installer-result.json'
        $fakeInstallerReplacement = Join-Path $launcherTestRoot 'fake-installer-replacement.ps1'
        $manifestReplacement = Join-Path $launcherTestRoot 'manifest-replacement.json'
        $launcherRootReplacement = $launcherTestRoot + '.renamed'
        $fakeInstallerTemplate = @'
param(
    [switch]$Upgrade,
    [switch]$Yes,
    [string]$ExpectedGoalId,
    [string]$ExpectedSourceFingerprint,
    [string]$UpgradeAuthorityManifestPath,
    [string]$UpgradeAuthorityManifestSha256
)
if (-not $Upgrade -or -not $Yes -or
    $ExpectedGoalId -cne 'goal_0123456789' -or
    $ExpectedSourceFingerprint -cne ('a' * 64) -or
    $UpgradeAuthorityManifestSha256 -notmatch '^[0-9a-f]{64}$') {
    throw 'fixed upgrade authority arguments missing'
}
$installerHelperArguments = @(
    '-NoProfile', '-NonInteractive', '-File', __HELPER_PATH__,
    '-Target', $PSCommandPath,
    '-Replacement', __INSTALLER_REPLACEMENT__
)
$manifestHelperArguments = @(
    '-NoProfile', '-NonInteractive', '-File', __HELPER_PATH__,
    '-Target', $UpgradeAuthorityManifestPath,
    '-Replacement', __MANIFEST_REPLACEMENT__
)
$parentHelperArguments = @(
    '-NoProfile', '-NonInteractive', '-File', __HELPER_PATH__,
    '-Target', __LAUNCHER_ROOT__,
    '-Replacement', __LAUNCHER_ROOT_REPLACEMENT__, '-Directory'
)
$installerAttempt = @(& __POWERSHELL_PATH__ @installerHelperArguments)
$manifestAttempt = @(& __POWERSHELL_PATH__ @manifestHelperArguments)
$parentAttempt = @(& __POWERSHELL_PATH__ @parentHelperArguments)
[IO.File]::WriteAllText(
    __RESULT_PATH__,
    ([ordered]@{
        installer_attempt = $installerAttempt -join ' '
        manifest_attempt = $manifestAttempt -join ' '
        parent_attempt = $parentAttempt -join ' '
        process_id = $PID
    } | ConvertTo-Json -Compress),
    [Text.UTF8Encoding]::new($false, $true)
)
'@
        $fakeInstallerText = $fakeInstallerTemplate
        foreach ($entry in ([ordered]@{
            '__POWERSHELL_PATH__' = ConvertTo-PowerShellSingleQuotedLiteral $runtime.Path
            '__HELPER_PATH__' = ConvertTo-PowerShellSingleQuotedLiteral $replaceHelperPath
            '__INSTALLER_REPLACEMENT__' = ConvertTo-PowerShellSingleQuotedLiteral $fakeInstallerReplacement
            '__MANIFEST_REPLACEMENT__' = ConvertTo-PowerShellSingleQuotedLiteral $manifestReplacement
            '__LAUNCHER_ROOT__' = ConvertTo-PowerShellSingleQuotedLiteral $launcherTestRoot
            '__LAUNCHER_ROOT_REPLACEMENT__' = ConvertTo-PowerShellSingleQuotedLiteral $launcherRootReplacement
            '__RESULT_PATH__' = ConvertTo-PowerShellSingleQuotedLiteral $fakeInstallerResult
        }).GetEnumerator()) {
            $fakeInstallerText = $fakeInstallerText.Replace(
                [string]$entry.Key, [string]$entry.Value
            )
        }
        $fakeInstallerBytes = [Text.UTF8Encoding]::new($false, $true).GetBytes(
            (($fakeInstallerText -replace '\r?\n', [Environment]::NewLine).TrimEnd() +
                [Environment]::NewLine)
        )
        Write-BytesAtomic -Path $fakeInstallerPath -Bytes $fakeInstallerBytes -Security $fileSecurity
        Write-BytesAtomic -Path $fakeInstallerReplacement -Bytes (
            [Text.UTF8Encoding]::new($false, $true).GetBytes('tampered installer')
        ) -Security $fileSecurity
        Write-BytesAtomic -Path $manifestReplacement -Bytes (
            [Text.UTF8Encoding]::new($false, $true).GetBytes('tampered manifest')
        ) -Security $fileSecurity

        $userCommand = New-BrokerUpgradeUserCommandText -Action upgrade -PowerShellPath $runtime.Path -PowerShellSha256 $runtime.Sha256 -InstallerPath $fakeInstallerPath -InstallerSha256 (Get-BytesSha256 -Bytes $fakeInstallerBytes) -AuthorityManifestPath $launcherManifestPath -AuthorityManifestSha256 $launcherManifestHash -GoalId 'goal_0123456789' -SourceFingerprint ('a' * 64)
        $installUserCommand = New-BrokerUpgradeUserCommandText `
            -Action install -PowerShellPath $runtime.Path `
            -PowerShellSha256 $runtime.Sha256 `
            -InstallerPath $fakeInstallerPath `
            -InstallerSha256 (Get-BytesSha256 -Bytes $fakeInstallerBytes) `
            -AuthorityManifestPath $launcherManifestPath `
            -AuthorityManifestSha256 $launcherManifestHash `
            -GoalId 'goal_0123456789' -SourceFingerprint ('a' * 64)
        if (-not $installUserCommand.Contains("`$action = 'install'") -or
            -not $installUserCommand.Contains('InstallAuthorityManifestPath') -or
            -not $installUserCommand.Contains('InstallAuthorityManifestSha256')) {
            throw 'Trusted fresh-install user command lost its fixed action binding.'
        }
        if ($userCommand -match '(?i)Start-Process|RunAs|\.cmd|Get-CimInstance|Get-WmiObject|Win32_Process' -or
            $userCommand.Contains('Get-Item -LiteralPath') -or
            -not $userCommand.Contains('[IO.FileShare]::Read') -or
            -not $userCommand.Contains('[IO.FileOptions]0x00200000') -or
            -not $userCommand.Contains('[IO.File]::GetAttributes($handle)') -or
            -not $userCommand.Contains('[Environment]::GetCommandLineArgs()') -or
            -not $userCommand.Contains($launcherManifestHash)) {
            throw 'Trusted user command lost its profile-free lexical no-reparse binding.'
        }
        $commandTokens = $null
        $commandErrors = $null
        [void][Management.Automation.Language.Parser]::ParseInput(
            $userCommand, [ref]$commandTokens, [ref]$commandErrors
        )
        if (@($commandErrors).Count -ne 0) {
            throw "Trusted user command has parse errors: $(@($commandErrors) -join '; ')"
        }

        $wrongHashCommand = New-BrokerUpgradeUserCommandText -Action upgrade -PowerShellPath $runtime.Path -PowerShellSha256 $runtime.Sha256 -InstallerPath $fakeInstallerPath -InstallerSha256 (Get-BytesSha256 -Bytes $fakeInstallerBytes) -AuthorityManifestPath $launcherManifestPath -AuthorityManifestSha256 ('0' * 64) -GoalId 'goal_0123456789' -SourceFingerprint ('a' * 64)
        try {
            & ([scriptblock]::Create($wrongHashCommand)) 2>$null
        } catch { }
        if (Test-Path -LiteralPath $fakeInstallerResult) {
            throw 'Trusted user command executed after authority-manifest hash drift.'
        }

        $missingNoProfileCommand = $userCommand.Replace(
            'foreach ($argument in [Environment]::GetCommandLineArgs()) {',
            "foreach (`$argument in @('pwsh.exe')) {"
        )
        if ($missingNoProfileCommand -ceq $userCommand) {
            throw 'Trusted user command no-profile negative probe was not injected.'
        }
        $missingNoProfileError = $null
        try { & ([scriptblock]::Create($missingNoProfileCommand)) }
        catch { $missingNoProfileError = $_.Exception.Message }
        if ($null -eq $missingNoProfileError -or
            -not $missingNoProfileError.Contains(
                'requires PowerShell 7 launched with -NoProfile',
                [StringComparison]::Ordinal
            ) -or (Test-Path -LiteralPath $fakeInstallerResult)) {
            throw "Trusted user command accepted a profile-loaded host: $missingNoProfileError"
        }

        $raceInstallerOriginal = Join-Path `
            $launcherTestRoot 'fake-installer-race-original.ps1'
        $raceBoundary = '    Open-Namespace $fullPath'
        $raceInjection = @'
    Open-Namespace $fullPath
    if ($fullPath -ceq __RACE_PATH__) {
        [IO.File]::Move($fullPath, __RACE_ORIGINAL__)
        [void][IO.File]::CreateSymbolicLink($fullPath, __RACE_ORIGINAL__)
    }
'@
        $raceInjection = $raceInjection.Replace(
            '__RACE_PATH__',
            (ConvertTo-PowerShellSingleQuotedLiteral $fakeInstallerPath)
        ).Replace(
            '__RACE_ORIGINAL__',
            (ConvertTo-PowerShellSingleQuotedLiteral $raceInstallerOriginal)
        )
        $raceCommand = $userCommand.Replace($raceBoundary, $raceInjection.TrimEnd())
        if ($raceCommand -ceq $userCommand) {
            throw 'Trusted user command lexical leaf race probe was not injected.'
        }
        $raceError = $null
        try { & ([scriptblock]::Create($raceCommand)) }
        catch { $raceError = $_.Exception.Message }
        finally {
            if (Test-Path -LiteralPath $fakeInstallerPath) {
                $raceItem = Get-Item -LiteralPath $fakeInstallerPath -Force
                if (($raceItem.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
                    Remove-Item -LiteralPath $fakeInstallerPath -Force
                }
            }
            if (Test-Path -LiteralPath $raceInstallerOriginal) {
                [IO.File]::Move($raceInstallerOriginal, $fakeInstallerPath, $true)
            }
        }
        if ($null -eq $raceError -or
            -not $raceError.Contains(
                'Pinned launch input is not an ordinary file',
                [StringComparison]::Ordinal
            ) -or (Test-Path -LiteralPath $fakeInstallerResult)) {
            throw "Trusted user command followed a symlink inserted at leaf open: $raceError"
        }

        $junctionTarget = Join-Path $launcherTestRoot 'launcher-junction-target'
        [void](New-ManagedDirectory -Path $junctionTarget `
            -Security $readOnlySecurity -OwnerSid $testUserSid `
            -Label 'Trusted command junction target')
        $junctionManifestTarget = Join-Path $junctionTarget 'authority.json'
        Write-BytesAtomic -Path $junctionManifestTarget `
            -Bytes $launcherManifestBytes -Security $fileSecurity
        $junctionEntry = Join-Path $launcherTestRoot 'launcher-junction-entry'
        [void](New-Item -ItemType Junction -Path $junctionEntry `
            -Target $junctionTarget -ErrorAction Stop)
        $junctionManifestPath = Join-Path $junctionEntry 'authority.json'
        $junctionCommand = New-BrokerUpgradeUserCommandText `
            -Action upgrade `
            -PowerShellPath $runtime.Path -PowerShellSha256 $runtime.Sha256 `
            -InstallerPath $fakeInstallerPath `
            -InstallerSha256 (Get-BytesSha256 -Bytes $fakeInstallerBytes) `
            -AuthorityManifestPath $junctionManifestPath `
            -AuthorityManifestSha256 $launcherManifestHash `
            -GoalId 'goal_0123456789' -SourceFingerprint ('a' * 64)
        $junctionError = $null
        try { & ([scriptblock]::Create($junctionCommand)) }
        catch { $junctionError = $_.Exception.Message }
        finally { [IO.Directory]::Delete($junctionEntry) }
        if ($null -eq $junctionError -or
            -not $junctionError.Contains(
                'Pinned launch ancestor is not an ordinary directory',
                [StringComparison]::Ordinal
            ) -or (Test-Path -LiteralPath $fakeInstallerResult)) {
            throw "Trusted user command followed a junction ancestor: $junctionError"
        }

        & ([scriptblock]::Create($userCommand))
        $launcherReport = Read-JsonDocument -Path $fakeInstallerResult -Label 'Trusted user command result'
        foreach ($attempt in @(
            [string]$launcherReport.installer_attempt,
            [string]$launcherReport.manifest_attempt,
            [string]$launcherReport.parent_attempt
        )) {
            if ($attempt -notmatch '^blocked:(5|32)$') {
                throw "Trusted user command did not block a real cross-process replacement: $attempt"
            }
        }
        if (-not (Test-Path -LiteralPath $fakeInstallerReplacement) -or
            -not (Test-Path -LiteralPath $manifestReplacement)) {
            throw 'Trusted user command replacement probes unexpectedly consumed their source files.'
        }

        [IO.File]::Move($fakeInstallerReplacement, $fakeInstallerPath, $true)
        [IO.File]::Move($manifestReplacement, $launcherManifestPath, $true)
        [IO.Directory]::Move($launcherTestRoot, $launcherRootReplacement)
        [IO.Directory]::Move($launcherRootReplacement, $launcherTestRoot)
        if ((Get-FileSha256 -Path $fakeInstallerPath) -ceq
                (Get-BytesSha256 -Bytes $fakeInstallerBytes) -or
            (Get-FileSha256 -Path $launcherManifestPath) -ceq
                $launcherManifestHash) {
            throw 'Trusted user command did not release its pinned handles after completion.'
        }

        $nativeGuardParent = Join-Path $launcherTestRoot 'native-guard-parent'
        $nativeGuardChild = Join-Path $nativeGuardParent 'child'
        [void](New-ManagedDirectory -Path $nativeGuardParent `
            -Security $readOnlySecurity -OwnerSid $testUserSid `
            -Label 'Native guard parent')
        [void](New-ManagedDirectory -Path $nativeGuardChild `
            -Security $readOnlySecurity -OwnerSid $testUserSid `
            -Label 'Native guard child')
        $nativeGuardFile = Join-Path $nativeGuardChild 'held.bin'
        Write-BytesAtomic -Path $nativeGuardFile `
            -Bytes ([Text.UTF8Encoding]::new($false).GetBytes('held')) `
            -Security $fileSecurity
        $transientShareFile = Join-Path $nativeGuardChild 'transient-share.bin'
        Write-BytesAtomic -Path $transientShareFile `
            -Bytes ([Text.UTF8Encoding]::new($false).GetBytes('transient')) `
            -Security $fileSecurity
        $transientShareBlocker = [IO.FileStream]::new(
            $transientShareFile, [IO.FileMode]::Open,
            [IO.FileAccess]::Read, [IO.FileShare]::None
        )
        [Rayman.CodexBrokerInstallerNative]::DisposeAfter(
            $transientShareBlocker, 150
        )
        $transientTimer = [Diagnostics.Stopwatch]::StartNew()
        $transientShareHeld = Open-BrokerPinnedFile `
            -Path $transientShareFile -Label 'Native transient pinned open' `
            -MaximumBytes 64KB -OpenRetryTimeoutMilliseconds 2000 `
            -OpenRetryDelayMilliseconds 20
        try {
            if ($transientTimer.ElapsedMilliseconds -lt 100) {
                throw 'Native transient pinned open did not exercise bounded retry.'
            }
        } finally {
            Close-BrokerPinnedFile -Held $transientShareHeld
            $transientShareBlocker.Dispose()
        }
        $persistentShareBlocker = [IO.FileStream]::new(
            $transientShareFile, [IO.FileMode]::Open,
            [IO.FileAccess]::Read, [IO.FileShare]::None
        )
        $persistentShareError = $null
        try {
            [void](Open-BrokerPinnedFile `
                -Path $transientShareFile `
                -Label 'Native persistent pinned open' -MaximumBytes 64KB `
                -OpenRetryTimeoutMilliseconds 75 `
                -OpenRetryDelayMilliseconds 10)
        } catch { $persistentShareError = $_.Exception.Message }
        finally { $persistentShareBlocker.Dispose() }
        if ($null -eq $persistentShareError -or
            -not $persistentShareError.Contains(
                'Native persistent pinned open pinned open failed',
                [StringComparison]::Ordinal
            ) -or
            -not $persistentShareError.Contains(
                "path=$transientShareFile", [StringComparison]::Ordinal
            ) -or
            -not $persistentShareError.Contains(
                'win32=32', [StringComparison]::Ordinal
            ) -or
            -not $persistentShareError.Contains(
                'attempts=', [StringComparison]::Ordinal
            )) {
            throw "Native persistent pinned-open diagnostic drifted: $persistentShareError"
        }
        $nativeGuardReplacement = $nativeGuardParent + '.renamed'
        $nativeHeld = Open-BrokerPinnedFile -Path $nativeGuardFile `
            -Label 'Native ancestor guard behavior' -MaximumBytes 64KB
        try {
            $nativeGuardArguments = @(
                '-NoProfile', '-NonInteractive', '-File', $replaceHelperPath,
                '-Target', $nativeGuardParent,
                '-Replacement', $nativeGuardReplacement, '-Directory'
            )
            $nativeGuardAttempt = @(& $runtime.Path @nativeGuardArguments)
            if (($nativeGuardAttempt -join ' ') -notmatch '^blocked:(5|32)$') {
                throw "Native ancestor guard allowed parent rename: $($nativeGuardAttempt -join ' ')"
            }
        } finally { Close-BrokerPinnedFile -Held $nativeHeld }
        [IO.Directory]::Move($nativeGuardParent, $nativeGuardReplacement)
        [IO.Directory]::Move($nativeGuardReplacement, $nativeGuardParent)

        $nativeRaceFile = Join-Path $nativeGuardChild 'leaf-race.bin'
        $nativeRaceOriginal = $nativeRaceFile + '.original'
        $nativeRaceBytes = [Text.UTF8Encoding]::new($false).GetBytes('leaf-race')
        Write-BytesAtomic -Path $nativeRaceFile -Bytes $nativeRaceBytes `
            -Security $fileSecurity
        $nativeRaceAction = {
            [IO.File]::Move($nativeRaceFile, $nativeRaceOriginal)
            [void][IO.File]::CreateSymbolicLink(
                $nativeRaceFile, $nativeRaceOriginal
            )
        }.GetNewClosure()
        $nativeRaceHeld = $null
        $nativeRaceError = $null
        try {
            $nativeRaceHeld = Open-BrokerPinnedFile -Path $nativeRaceFile `
                -Label 'Native lexical leaf race' -MaximumBytes 64KB `
                -BeforeLeafOpenSelfTestAction $nativeRaceAction
        } catch { $nativeRaceError = $_.Exception.Message }
        finally {
            if ($null -ne $nativeRaceHeld) {
                Close-BrokerPinnedFile -Held $nativeRaceHeld
            }
            if (Test-Path -LiteralPath $nativeRaceFile) {
                $nativeRaceItem = Get-Item -LiteralPath $nativeRaceFile -Force
                if (($nativeRaceItem.Attributes -band
                        [IO.FileAttributes]::ReparsePoint) -ne 0) {
                    Remove-Item -LiteralPath $nativeRaceFile -Force
                }
            }
            if (Test-Path -LiteralPath $nativeRaceOriginal) {
                [IO.File]::Move($nativeRaceOriginal, $nativeRaceFile, $true)
            }
        }
        if ($null -eq $nativeRaceError -or
            -not $nativeRaceError.Contains(
                'pinned file entry is not the required ordinary object type',
                [StringComparison]::Ordinal
            )) {
            throw "Native pinned file followed a symlink inserted at leaf open: $nativeRaceError"
        }

        $nativeJunctionTarget = Join-Path $launcherTestRoot 'native-junction-target'
        $nativeJunctionChild = Join-Path $nativeJunctionTarget 'child'
        [void](New-ManagedDirectory -Path $nativeJunctionTarget `
            -Security $readOnlySecurity -OwnerSid $testUserSid `
            -Label 'Native junction target')
        [void](New-ManagedDirectory -Path $nativeJunctionChild `
            -Security $readOnlySecurity -OwnerSid $testUserSid `
            -Label 'Native junction target child')
        $nativeJunctionFile = Join-Path $nativeJunctionChild 'held.bin'
        Write-BytesAtomic -Path $nativeJunctionFile `
            -Bytes ([Text.UTF8Encoding]::new($false).GetBytes('junction-held')) `
            -Security $fileSecurity
        $nativeJunctionEntry = Join-Path $launcherTestRoot 'native-junction-entry'
        [void](New-Item -ItemType Junction -Path $nativeJunctionEntry `
            -Target $nativeJunctionTarget -ErrorAction Stop)
        $nativeDirectGuard = $null
        $nativeDirectGuardError = $null
        try {
            $nativeDirectGuard = `
                [Rayman.CodexBrokerInstallerNative]::OpenDirectoryGuard(
                    $nativeJunctionEntry
                )
        } catch { $nativeDirectGuardError = $_.Exception.Message }
        finally {
            if ($null -ne $nativeDirectGuard) { $nativeDirectGuard.Dispose() }
        }
        if ($null -eq $nativeDirectGuardError -or
            -not $nativeDirectGuardError.Contains(
                'protected directory guard is not the required ordinary object type',
                [StringComparison]::Ordinal
            )) {
            throw "Native directory guard followed a junction entry: $nativeDirectGuardError"
        }
        $nativeJunctionPath = Join-Path `
            (Join-Path $nativeJunctionEntry 'child') 'held.bin'
        $nativeJunctionHeld = $null
        $nativeJunctionError = $null
        try {
            $nativeJunctionHeld = Open-BrokerPinnedFile `
                -Path $nativeJunctionPath `
                -Label 'Native junction ancestor' -MaximumBytes 64KB
        } catch { $nativeJunctionError = $_.Exception.Message }
        finally {
            if ($null -ne $nativeJunctionHeld) {
                Close-BrokerPinnedFile -Held $nativeJunctionHeld
            }
            [IO.Directory]::Delete($nativeJunctionEntry)
        }
        if ($null -eq $nativeJunctionError) {
            throw "Native pinned file followed a junction ancestor: $nativeJunctionError"
        }

        $windowsPowerShell = 'C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe'
        $windowsPowerShellOutput = @(
            & $windowsPowerShell -NoProfile -NonInteractive -Command $userCommand 2>&1
        )
        if ($LASTEXITCODE -eq 0) {
            throw "Windows PowerShell unexpectedly accepted the PowerShell 7-only command: $($windowsPowerShellOutput -join ' ')"
        }
        $expectedMutexSecurity = New-BrokerInstallationMutexSecurity
        $mutexTrust = Get-BrokerInstallationMutexTrust
        $assertMutexRejected = {
            param(
                [Parameter(Mandatory = $true)]$Security,
                [Parameter(Mandatory = $true)][string]$ExpectedError,
                [Parameter(Mandatory = $true)][string]$Label
            )
            $failure = $null
            try {
                Assert-BrokerInstallationMutexSecurityContract `
                    -ActualSecurity $Security `
                    -ExpectedSecurity $expectedMutexSecurity `
                    -Name $Label
            } catch { $failure = $_.Exception.Message }
            if ($null -eq $failure -or
                -not $failure.Contains($ExpectedError, [StringComparison]::Ordinal)) {
                throw "$Label did not fail with ${ExpectedError}: $failure"
            }
        }
        foreach ($requiredOwner in @('S-1-5-18', 'S-1-5-32-544')) {
            if ([string]$requiredOwner -notin @($mutexTrust.AllowedOwnerSids)) {
                throw "Installation mutex trust omitted a privileged Windows default owner: $requiredOwner"
            }
        }
        foreach ($ownerSid in @($mutexTrust.AllowedOwnerSids)) {
            $ownerProbe = New-BrokerInstallationMutexSecurity
            $ownerProbe.SetOwner(
                [Security.Principal.SecurityIdentifier]::new([string]$ownerSid)
            )
            Assert-BrokerInstallationMutexSecurityContract `
                -ActualSecurity $ownerProbe `
                -ExpectedSecurity $expectedMutexSecurity `
                -Name ('self-test-owner-' + [string]$ownerSid)
        }
        $invalidMutexOwnerSids = [Collections.Generic.List[string]]::new()
        foreach ($ownerSid in @('S-1-1-0', 'S-1-5-11', 'S-1-5-32-545')) {
            $invalidMutexOwnerSids.Add($ownerSid)
        }
        try {
            $sandboxGroupOwnerSid = [string]([Security.Principal.NTAccount]::new(
                'QIN5521\CodexSandboxUsers'
            ).Translate([Security.Principal.SecurityIdentifier]).Value)
            if ($sandboxGroupOwnerSid -notin @($invalidMutexOwnerSids)) {
                $invalidMutexOwnerSids.Add($sandboxGroupOwnerSid)
            }
        } catch [Security.Principal.IdentityNotMappedException] { }
        foreach ($ownerSid in @($invalidMutexOwnerSids)) {
            $ownerProbe = New-BrokerInstallationMutexSecurity
            $ownerProbe.SetOwner(
                [Security.Principal.SecurityIdentifier]::new([string]$ownerSid)
            )
            & $assertMutexRejected `
                -Security $ownerProbe -ExpectedError 'owner_not_allowed' `
                -Label ('installation mutex invalid owner ' + [string]$ownerSid)
        }
        $unprotectedMutexSecurity = New-BrokerInstallationMutexSecurity
        $unprotectedMutexSecurity.SetOwner(
            [Security.Principal.SecurityIdentifier]::new('S-1-5-32-544')
        )
        $unprotectedMutexSecurity.SetAccessRuleProtection($false, $false)
        & $assertMutexRejected `
            -Security $unprotectedMutexSecurity `
            -ExpectedError 'dacl_not_protected' `
            -Label 'installation mutex unprotected DACL'
        $extraAceMutexSecurity = New-BrokerInstallationMutexSecurity
        $extraAceMutexSecurity.SetOwner(
            [Security.Principal.SecurityIdentifier]::new('S-1-5-32-544')
        )
        $extraAceMutexSecurity.AddAccessRule(
            [Security.AccessControl.MutexAccessRule]::new(
                [Security.Principal.SecurityIdentifier]::new('S-1-5-32-545'),
                [Security.AccessControl.MutexRights]::ReadPermissions,
                [Security.AccessControl.AccessControlType]::Allow
            )
        )
        & $assertMutexRejected `
            -Security $extraAceMutexSecurity -ExpectedError 'dacl_mismatch' `
            -Label 'installation mutex extra ACE'
        $guardProbe = Open-BrokerInstallationGuard `
            -Root $testRoot -TimeoutMilliseconds 0
        try {
            $guardCheckOutput = @(
                & $runtime.Path -NoProfile -NonInteractive -File $PSCommandPath `
                    -Check -InstallRoot $testRoot `
                    -RequestRoot (Join-Path $testRoot 'guard-requests') `
                    -TaskName '\Rayman-Broker-Guard-SelfTest' 2>&1
            )
            if ($LASTEXITCODE -ne 0) {
                throw "Installation guard child failed: $($guardCheckOutput -join ' ')"
            }
            $guardCheck = ($guardCheckOutput -join "`n") | ConvertFrom-Json `
                -Depth 8 -NoEnumerate -DateKind String
            if ([bool]$guardCheck.transaction_in_progress -ne $true -or
                $null -ne $guardCheck.installed -or
                [bool]$guardCheck.operational_ready -ne $false -or
                [string]$guardCheck.receipt_state -cne 'not_checked') {
                throw 'Cross-process installation guard exposed a mixed-epoch check.'
            }
        } finally {
            Close-BrokerInstallationGuard -Guard $guardProbe
        }

        $authorityNow = [DateTimeOffset]::UtcNow
        $authorityLifetime = [pscustomobject]@{
            schema_version = 1
            nonce = [Guid]::NewGuid().ToString('N')
            created_at_utc = $authorityNow.ToString('o')
            expires_at_utc = $authorityNow.AddMinutes(30).ToString('o')
        }
        [void](Assert-BrokerUpgradeAuthorityLifetime `
            -Manifest $authorityLifetime -Now $authorityNow)
        foreach ($invalidLifetime in @(
            [pscustomobject]@{
                schema_version = 1; nonce = [Guid]::NewGuid().ToString('N')
                created_at_utc = $authorityNow.AddMinutes(-40).ToString('o')
                expires_at_utc = $authorityNow.AddMinutes(-10).ToString('o')
            },
            [pscustomobject]@{
                schema_version = 1; nonce = [Guid]::NewGuid().ToString('N')
                created_at_utc = $authorityNow.AddMinutes(1).ToString('o')
                expires_at_utc = $authorityNow.AddMinutes(2).ToString('o')
            },
            [pscustomobject]@{
                schema_version = 1; nonce = [Guid]::NewGuid().ToString('N')
                created_at_utc = $authorityNow.ToString('o')
                expires_at_utc = $authorityNow.AddHours(2).ToString('o')
            }
        )) {
            & $assertRejected {
                [void](Assert-BrokerUpgradeAuthorityLifetime `
                    -Manifest $invalidLifetime -Now $authorityNow)
            } 'upgrade authority lifetime'
        }
        $savedGitEnvironment = [ordered]@{}
        foreach ($entry in @(Get-ChildItem Env: | Where-Object {
            $_.Name.StartsWith('GIT_', [StringComparison]::OrdinalIgnoreCase) -or
            $_.Name -in @('SSH_ASKPASS', 'GCM_INTERACTIVE')
        })) {
            $savedGitEnvironment[[string]$entry.Name] = [string]$entry.Value
        }
        try {
        $env:GIT_CONFIG_COUNT = '1'
        $env:GIT_CONFIG_KEY_0 = 'core.fsmonitor'
        $env:GIT_CONFIG_VALUE_0 = 'injected-executable-hook'
        $env:GIT_DIR = 'injected-git-dir'
        Clear-BrokerGitEnvironment
        if ($null -ne [Environment]::GetEnvironmentVariable(
                'GIT_CONFIG_COUNT', [EnvironmentVariableTarget]::Process
            ) -or
            $null -ne [Environment]::GetEnvironmentVariable(
                'GIT_CONFIG_KEY_0', [EnvironmentVariableTarget]::Process
            ) -or
            $null -ne [Environment]::GetEnvironmentVariable(
                'GIT_CONFIG_VALUE_0', [EnvironmentVariableTarget]::Process
            ) -or
            $null -ne [Environment]::GetEnvironmentVariable(
                'GIT_DIR', [EnvironmentVariableTarget]::Process
            ) -or
            $env:GIT_CONFIG_NOSYSTEM -cne '1' -or
            $env:GIT_CONFIG_GLOBAL -cne 'NUL' -or
            $env:GIT_OPTIONAL_LOCKS -cne '0') {
            throw 'Upgrade authority did not remove inherited Git execution inputs.'
        }
        } finally {
            foreach ($entry in @(Get-ChildItem Env: | Where-Object {
                $_.Name.StartsWith(
                    'GIT_', [StringComparison]::OrdinalIgnoreCase
                ) -or $_.Name -in @('SSH_ASKPASS', 'GCM_INTERACTIVE')
            })) {
                Remove-Item -LiteralPath ('Env:' + [string]$entry.Name) `
                    -ErrorAction Stop
            }
            foreach ($name in @($savedGitEnvironment.Keys)) {
                [Environment]::SetEnvironmentVariable(
                    $name, [string]$savedGitEnvironment[$name],
                    [EnvironmentVariableTarget]::Process
                )
            }
        }

        $goalRuns = @(
            [pscustomobject]@{
                exit_code = 0
                workspace_fingerprint_before = 'a' * 64
                workspace_fingerprint_after = 'a' * 64
            },
            [pscustomobject]@{
                exit_code = 0
                workspace_fingerprint_before = 'a' * 64
                workspace_fingerprint_after = 'a' * 64
            }
        )
        $goalAuthorityProbe = [pscustomobject]@{
            id = 'goal_0123456789'
            status = 'active'
            lifecycle = 'current'
            authority_receipts = @([pscustomobject]@{
                repeat = 2
                workspace_fingerprint = 'a' * 64
                runs = $goalRuns
            })
            review_receipts = @([pscustomobject]@{
                source_fingerprint = 'a' * 64
            })
            requirements = @(
                [pscustomobject]@{ id = 'req_1'; status = 'done' },
                [pscustomobject]@{ id = 'req_2'; status = 'done' },
                [pscustomobject]@{ id = 'req_3'; status = 'done' },
                [pscustomobject]@{ id = 'req_4'; status = 'open' },
                [pscustomobject]@{ id = 'req_5'; status = 'open' }
            )
        }
        Assert-BrokerUpgradeGoalAuthority -Goal $goalAuthorityProbe `
            -GoalId 'goal_0123456789' -SourceFingerprint ('a' * 64) `
            -Action upgrade
        $goalAuthorityProbe.requirements[3].status = 'done'
        Assert-BrokerUpgradeGoalAuthority -Goal $goalAuthorityProbe `
            -GoalId 'goal_0123456789' -SourceFingerprint ('a' * 64) `
            -Action install
        $goalAuthorityProbe.requirements[3].status = 'open'
        $goalAuthorityProbe.status = 'success'
        & $assertRejected {
            Assert-BrokerUpgradeGoalAuthority -Goal $goalAuthorityProbe `
                -GoalId 'goal_0123456789' -SourceFingerprint ('a' * 64) `
                -Action upgrade
        } 'closed Goal authority replay'
        $goalAuthorityProbe.status = 'active'
        $goalAuthorityProbe.requirements += [pscustomobject]@{
            id = 'req_5'; status = 'open'
        }
        & $assertRejected {
            Assert-BrokerUpgradeGoalAuthority -Goal $goalAuthorityProbe `
                -GoalId 'goal_0123456789' -SourceFingerprint ('a' * 64) `
                -Action upgrade
        } 'duplicate Goal requirement authority'

        $frontierBlocker = [pscustomobject]@{
            contract_version = 2
            id = 'pending_0123456789'
            goal_id = 'goal_0123456789'
            owner = 'human'
            consultation_timing = 'immediate'
            capability_key = $script:UpgradePendingCapabilityKey
            boundary_class = $script:UpgradePendingBoundaryClass
            package_sha256 = 'b' * 64
            background_mechanism = $null
            background_authority_evidence = $null
            background_isolation_evidence = $null
        }
        $frontierProbe = [pscustomobject]@{
            goal_id = 'goal_0123456789'
            decision = 'ask_user'
            ask_user_allowed = $true
            execution = 'paused_for_user'
            consultation = 'ready'
            background_execution_allowed = $false
            blockers = @($frontierBlocker)
        }
        [void](Assert-BrokerUpgradeFrontierReport `
            -Frontier $frontierProbe -GoalId 'goal_0123456789' `
            -ExpectedCapabilityKey $script:UpgradePendingCapabilityKey `
            -ExpectedBoundaryClass $script:UpgradePendingBoundaryClass)
        $frontierProbe.decision = 'wait_external'
        & $assertRejected {
            [void](Assert-BrokerUpgradeFrontierReport `
                -Frontier $frontierProbe -GoalId 'goal_0123456789' `
                -ExpectedCapabilityKey $script:UpgradePendingCapabilityKey `
                -ExpectedBoundaryClass $script:UpgradePendingBoundaryClass)
        } 'impossible mixed Goal frontier tuple'
        $frontierProbe.decision = 'ask_user'
        $pendingProbe = [pscustomobject]@{ items = @($frontierBlocker) }
        [void](Assert-BrokerUpgradePendingBoundary `
            -Pending $pendingProbe -GoalId 'goal_0123456789' `
            -CapabilityKey $script:UpgradePendingCapabilityKey `
            -BoundaryClass $script:UpgradePendingBoundaryClass `
            -ExpectedPendingId 'pending_0123456789' `
            -ExpectedPackageSha256 ('b' * 64))

        $nonceProbe = [Guid]::NewGuid().ToString('N')
        $nonceTransaction = [Guid]::NewGuid().ToString('N')
        [void](Write-BrokerUpgradeConsumedNonce `
            -Root $testRoot -Nonce $nonceProbe `
            -TransactionId $nonceTransaction `
            -GoalId 'goal_0123456789' -SourceFingerprint ('a' * 64) `
            -FileSecurity $fileSecurity -UserSid $testUserSid)
        & $assertRejected {
            Assert-BrokerUpgradeNonceUnconsumed `
                -Root $testRoot -Nonce $nonceProbe
        } 'consumed upgrade authority replay'

        $checkRoot = Join-Path $testRoot 'check-recovery-debt'
        [void](New-ManagedDirectory -Path $checkRoot `
            -Security $readOnlySecurity -OwnerSid $testUserSid `
            -Label 'Check recovery debt root')
        Write-BytesAtomic -Path (Join-Path $checkRoot `
            $script:UpgradeRecoveryJournalName) `
            -Bytes ([Text.UTF8Encoding]::new($false).GetBytes(
                '{"schema_version":2}'
            )) -Security $fileSecurity
        $checkDebt = Get-InstallationState -Root $checkRoot `
            -Requests (Join-Path $checkRoot 'requests') `
            -Name '\Rayman-Broker-Check-Debt-SelfTest'
        if ([bool]$checkDebt.recovery_required -ne $true -or
            [string]$checkDebt.recovery_journal_state -cne 'invalid' -or
            [bool]$checkDebt.operational_ready -ne $false -or
            [string]$checkDebt.error -notmatch
                'retained upgrade recovery journal') {
            throw 'Public -Check hid a retained upgrade recovery debt.'
        }
        if (-not (Ensure-BrokerUpgradeResultsRoot `
            -Root $testRoot -UserSid $testUserSid `
            -SandboxSid $testSandboxSid)) {
            throw 'Upgrade result-root recovery self-test did not create the missing root.'
        }
        if (Ensure-BrokerUpgradeResultsRoot `
            -Root $testRoot -UserSid $testUserSid `
            -SandboxSid $testSandboxSid) {
            throw 'Upgrade result-root recovery self-test did not become idempotent.'
        }
        $resultRootProbe = Join-Path $testRoot 'results'
        $resultRootGuard = Open-InstallerDirectoryGuard `
            -Path $resultRootProbe -Label 'Result-root guard self-test'
        try {
            & $assertRejected {
                Remove-Item -LiteralPath $resultRootProbe -Recurse -Force
            } 'held upgrade result root removal'
        } finally { $resultRootGuard.Dispose() }
        Remove-Item -LiteralPath $resultRootProbe -Recurse -Force

        $receiptReplaceProbe = Join-Path $testRoot 'receipt-replace-probe.json'
        $receiptBefore = [Text.UTF8Encoding]::new($false).GetBytes('{"value":"before"}')
        $receiptAfter = [Text.UTF8Encoding]::new($false).GetBytes('{"value":"after"}')
        Write-BytesAtomic -Path $receiptReplaceProbe `
            -Bytes $receiptBefore -Security $fileSecurity
        $receiptBlocker = [IO.FileStream]::new(
            $receiptReplaceProbe, [IO.FileMode]::Open,
            [IO.FileAccess]::Read, [IO.FileShare]::Read
        )
        try {
            & $assertRejected {
                Wait-BrokerReceiptReplaceReady `
                    -Path $receiptReplaceProbe -TimeoutMilliseconds 50
            } 'held receipt replace probe'
        } finally { $receiptBlocker.Dispose() }

        $receiptBlocker = [IO.FileStream]::new(
            $receiptReplaceProbe, [IO.FileMode]::Open,
            [IO.FileAccess]::Read, [IO.FileShare]::Read
        )
        [Rayman.CodexBrokerInstallerNative]::DisposeAfter($receiptBlocker, 150)
        $releasedForward = Publish-BrokerReceiptForward `
            -Path $receiptReplaceProbe -OldBytes $receiptBefore `
            -NewBytes $receiptAfter -Security $fileSecurity `
            -OwnerSid $testUserSid -ProbeTimeoutMilliseconds 1000
        if ([string]$releasedForward.State -cne 'published_new') {
            throw 'Receipt replace self-test did not wait for the blocking handle.'
        }
        $restored = Restore-BrokerReceiptFromJournal `
            -Path $receiptReplaceProbe -OldBytes $receiptBefore `
            -NewBytes $receiptAfter -Security $fileSecurity `
            -OwnerSid $testUserSid -ProbeTimeoutMilliseconds 50
        if ([string]$restored.State -notlike 'old_restored*') {
            throw 'Receipt rollback self-test did not restore old bytes.'
        }
        $skipped = Restore-BrokerReceiptFromJournal `
            -Path $receiptReplaceProbe -OldBytes $receiptBefore `
            -NewBytes $receiptAfter -Security $fileSecurity `
            -OwnerSid $testUserSid -ProbeTimeoutMilliseconds 50
        if ([string]$skipped.State -cne 'old_already_restored' -or
            [int]$skipped.Attempts -ne 0) {
            throw 'Receipt rollback self-test rewrote unchanged old bytes.'
        }

        try {
            $script:InstallerSelfTestReceiptFaultPhase = 'forward_old_unchanged'
            & $assertRejected {
                [void](Publish-BrokerReceiptForward `
                    -Path $receiptReplaceProbe -OldBytes $receiptBefore `
                    -NewBytes $receiptAfter -Security $fileSecurity `
                    -OwnerSid $testUserSid -ProbeTimeoutMilliseconds 50)
            } 'forward failure with old receipt unchanged'
        } finally { $script:InstallerSelfTestReceiptFaultPhase = $null }
        if ((Get-FileSha256 -Path $receiptReplaceProbe) -cne
            (Get-BytesSha256 -Bytes $receiptBefore)) {
            throw 'Forward old-unchanged self-test mutated the receipt.'
        }

        try {
            $script:InstallerSelfTestReceiptFaultPhase = 'forward_new_written'
            $writtenAfterError = Publish-BrokerReceiptForward `
                -Path $receiptReplaceProbe -OldBytes $receiptBefore `
                -NewBytes $receiptAfter -Security $fileSecurity `
                -OwnerSid $testUserSid -ProbeTimeoutMilliseconds 50
        } finally { $script:InstallerSelfTestReceiptFaultPhase = $null }
        if ([string]$writtenAfterError.State -cne 'published_new_after_error') {
            throw 'Forward new-written self-test did not continue from actual bytes.'
        }
        [void](Restore-BrokerReceiptFromJournal `
            -Path $receiptReplaceProbe -OldBytes $receiptBefore `
            -NewBytes $receiptAfter -Security $fileSecurity `
            -OwnerSid $testUserSid -ProbeTimeoutMilliseconds 50)

        try {
            $script:InstallerSelfTestReceiptFaultPhase = 'forward_unknown'
            & $assertRejected {
                [void](Publish-BrokerReceiptForward `
                    -Path $receiptReplaceProbe -OldBytes $receiptBefore `
                    -NewBytes $receiptAfter -Security $fileSecurity `
                    -OwnerSid $testUserSid -ProbeTimeoutMilliseconds 50)
            } 'forward failure with unknown receipt bytes'
        } finally { $script:InstallerSelfTestReceiptFaultPhase = $null }
        & $assertRejected {
            [void](Restore-BrokerReceiptFromJournal `
                -Path $receiptReplaceProbe -OldBytes $receiptBefore `
                -NewBytes $receiptAfter -Security $fileSecurity `
                -OwnerSid $testUserSid -ProbeTimeoutMilliseconds 50)
        } 'unknown receipt rollback classification'
        Remove-Item -LiteralPath $receiptReplaceProbe -Force

        $realCrashCases = @(
            'staged',
            'old_stopped',
            'new_receipt_published',
            'new_task_registered',
            'new_task_started',
            'new_heartbeat_verified',
            'guard_released',
            'ready_published'
        )
        $fixtureIndeterminateCases = @(
            'unknown_receipt',
            'malformed_ready',
            'wrong_acl_ready',
            'directory_ready',
            'reparse_ready'
        )
        foreach ($previousSchema in @(
            $script:LegacySchemaVersion,
            $script:SchemaVersion
        )) {
        foreach ($phase in @($realCrashCases + $fixtureIndeterminateCases)) {
            $token = [Guid]::NewGuid().ToString('N')
            $caseRoot = Get-BrokerSelfTestCaseRoot `
                -ManagedRoot $managedRoot -Token $token `
                -Label 'Upgrade crash parent case root'
            try {
                $fixturePhase = if ($phase -in $realCrashCases) {
                    'staged'
                } else { $phase }
                Initialize-BrokerRecoverySelfTestFixture `
                    -CaseRoot $caseRoot -Phase $fixturePhase `
                    -Account ([string]$identity.Name) `
                    -UserSid $testUserSid -Group 'BUILTIN\Users' `
                    -SandboxSid $testSandboxSid `
                    -PreviousSchema $previousSchema
                if ($phase -in $realCrashCases) {
                    $crashOutput = @(& $runtime.Path -NoProfile -File $PSCommandPath `
                        -UpgradeCrashSelfTestChild `
                        -UpgradeCrashSelfTestToken $token `
                        -UpgradeCrashSelfTestPhase $phase 2>&1)
                    if ($LASTEXITCODE -eq 0) {
                        throw "Upgrade crash child did not crash: phase=$phase output=$($crashOutput -join ' ')"
                    }
                    $casePath = Join-Path $caseRoot 'case.json'
                    $caseDocument = Read-JsonDocument `
                        -Path $casePath -Label 'Post-crash recovery case'
                    $caseDocument.phase = $phase
                    $caseDocument.expected = if ($phase -ceq 'ready_published') {
                        'committed_v3'
                    } elseif ($previousSchema -eq $script:SchemaVersion) {
                        'rolled_back_v3'
                    } else { 'rolled_back_v2' }
                    $caseDocument.expect_ready_retained =
                        $previousSchema -eq $script:SchemaVersion
                    Write-JsonAtomic -Path $casePath -Document $caseDocument `
                        -Replace -Security $fileSecurity
                }
                $childOutput = & $runtime.Path -NoProfile -File $PSCommandPath `
                    -RecoverySelfTestChild -RecoverySelfTestToken $token 2>&1
                if ($LASTEXITCODE -ne 0) {
                    throw "Recovery child process failed: previous_schema=$previousSchema phase=$phase output=$($childOutput -join ' ')"
                }
                $child = ($childOutput -join "`n") | ConvertFrom-Json `
                    -Depth 8 -NoEnumerate -DateKind String
                if ([string]$child.phase -cne $phase -or
                    [string]$child.powershell_path -cne $runtime.Path -or
                    [string]$child.powershell_sha256 -cne $runtime.Sha256) {
                    throw "Recovery child result drifted: phase=$phase"
                }
            } finally {
                Remove-Item -LiteralPath $caseRoot -Recurse -Force `
                    -ErrorAction SilentlyContinue
            }
        }
        }

        $postCreateFailurePath = Join-Path $testRoot 'post-create-failure'
        & $assertRejected {
            [void](New-ManagedDirectory -Path $postCreateFailurePath `
                -Security $readOnlySecurity -OwnerSid 'S-1-5-18' `
                -Label 'Post-create cleanup self-test directory')
        } 'managed directory post-create verification failure'
        if (Test-Path -LiteralPath $postCreateFailurePath) {
            throw 'Managed-directory post-create failure retained its verified empty directory.'
        }

        $atomicFailurePath = Join-Path $testRoot 'atomic-failure.bin'
        try {
            $script:InstallerSelfTestAtomicWriteFaultPhase = 'after_create'
            & $assertRejected {
                Write-BytesAtomic -Path $atomicFailurePath `
                    -Bytes ([byte[]](1, 2, 3)) -Security $fileSecurity
            } 'atomic write failure after staging create'
        } finally {
            $script:InstallerSelfTestAtomicWriteFaultPhase = $null
        }
        if ((Test-Path -LiteralPath $atomicFailurePath) -or
            @(Get-ChildItem -LiteralPath $testRoot -Force `
                -Filter '.atomic-failure.bin.stage-*').Count -ne 0) {
            throw 'Atomic-write failure self-test retained target or staging bytes.'
        }

        $readyProbeInstallId = [Guid]::NewGuid().ToString('N')
        $readyProbeWorkerHash = '1' * 64
        $readyProbeManifestHash = '2' * 64
        $readyProbeReceipt = [pscustomobject]@{
            install_id = $readyProbeInstallId
            worker_sha256 = $readyProbeWorkerHash
            git_capability_manifest_sha256 = $readyProbeManifestHash
            user_sid = $testUserSid
            sandbox_group_sid = $testSandboxSid
        }
        $readyProbePath = Join-Path $testRoot $script:GitCapabilityReadyName
        $readyProbe = New-GitCapabilityReadyDocument `
            -InstallId $readyProbeInstallId `
            -WorkerHash $readyProbeWorkerHash `
            -ManifestHash $readyProbeManifestHash
        Write-JsonAtomic -Path $readyProbePath -Document $readyProbe `
            -Security $fileSecurity
        [void](Assert-GitCapabilityReadyInstalled `
            -Receipt $readyProbeReceipt -Root $testRoot)
        $readyProbe.worker_sha256 = '3' * 64
        Write-JsonAtomic -Path $readyProbePath -Document $readyProbe `
            -Replace -Security $fileSecurity
        & $assertRejected {
            [void](Assert-GitCapabilityReadyInstalled `
                -Receipt $readyProbeReceipt -Root $testRoot)
        } 'ready marker worker binding drift'
        Remove-Item -LiteralPath $readyProbePath -Force

        $recoveryAccount = [string]$identity.Name
        $recoveryGroup = 'BUILTIN\Users'
        $recoveryTaskName = '\Rayman-Broker-Recovery-SelfTest'
        $recoveryRequests = Join-Path $testRoot 'requests'
        $recoveryRuntime = Get-CurrentPowerShellRuntime
        $legacyRecoveryHash = '4' * 64
        $stagedRecoveryHash = '5' * 64
        $legacyRecoveryReceipt = [ordered]@{
            schema_version = $script:LegacySchemaVersion
            install_id = [Guid]::NewGuid().ToString('N')
            install_root = $testRoot
            installed_at_utc = [DateTimeOffset]::UtcNow.ToString('o')
            task_name = $recoveryTaskName
            user_account = $recoveryAccount
            user_sid = $testUserSid
            sandbox_group = $recoveryGroup
            sandbox_group_sid = $testSandboxSid
            worker_path = Join-Path (Join-Path (Join-Path $testRoot 'versions') `
                $legacyRecoveryHash) 'codex-powershell-broker.ps1'
            worker_sha256 = $legacyRecoveryHash
            powershell_path = $recoveryRuntime.Path
            powershell_sha256 = $recoveryRuntime.Sha256
            request_root = $recoveryRequests
            result_root = Join-Path $testRoot 'results'
            capabilities = @('identity_probe')
        }
        $stagedRecoveryReceipt = [ordered]@{
            schema_version = $script:SchemaVersion
            install_id = [Guid]::NewGuid().ToString('N')
            install_root = $testRoot
            installed_at_utc = [DateTimeOffset]::UtcNow.ToString('o')
            task_name = $recoveryTaskName
            user_account = $recoveryAccount
            user_sid = $testUserSid
            sandbox_group = $recoveryGroup
            sandbox_group_sid = $testSandboxSid
            worker_path = Join-Path (Join-Path (Join-Path $testRoot 'versions') `
                $stagedRecoveryHash) 'codex-powershell-broker.ps1'
            worker_sha256 = $stagedRecoveryHash
            powershell_path = $recoveryRuntime.Path
            powershell_sha256 = $recoveryRuntime.Sha256
            request_root = $recoveryRequests
            result_root = Join-Path $testRoot 'results'
            capabilities = @('identity_probe', $script:GitCapabilityId)
            git_capability_manifest_path =
                Join-Path $testRoot $script:GitCapabilityManifestName
            git_capability_manifest_sha256 = '6' * 64
        }
        $legacyRecoveryBytes = [Text.UTF8Encoding]::new($false, $true).GetBytes(
            ($legacyRecoveryReceipt | ConvertTo-Json -Depth 16 -Compress)
        )
        $stagedRecoveryBytes = [Text.UTF8Encoding]::new($false, $true).GetBytes(
            ($stagedRecoveryReceipt | ConvertTo-Json -Depth 16 -Compress)
        )
        $legacyRecoveryTaskXml = New-BrokerTaskXml `
            -PowerShellPath $recoveryRuntime.Path `
            -WorkerPath ([string]$legacyRecoveryReceipt.worker_path) `
            -BrokerRoot $testRoot -Requests $recoveryRequests `
            -Account $recoveryAccount -Name $recoveryTaskName
        $journalOwnedPath = Join-Path $testRoot 'journal-owned.bin'
        Write-BytesAtomic -Path $journalOwnedPath `
            -Bytes ([Text.UTF8Encoding]::new($false).GetBytes('owned')) `
            -Security $fileSecurity
        $journalOwned = New-BrokerUpgradeOwnedObjectBinding `
            -Path $journalOwnedPath -Type file
        $legacyIdentityProbe = Get-BrokerFileSnapshot `
            -Path $journalOwnedPath -Label 'Recovery legacy identity probe' `
            -MaximumBytes 64KB
        $journalNonce = [Guid]::NewGuid().ToString('N')
        $recoveryJournalProbe = New-BrokerUpgradeRecoveryJournalDocument `
            -Root $testRoot -Requests $recoveryRequests `
            -Name $recoveryTaskName -Account $recoveryAccount `
            -UserSid $testUserSid -Group $recoveryGroup `
            -SandboxSid $testSandboxSid `
            -LegacyReceiptBytes $legacyRecoveryBytes `
            -LegacyReceiptIdentity $legacyIdentityProbe.Identity `
            -StagedReceiptBytes $stagedRecoveryBytes `
            -LegacyTaskXml $legacyRecoveryTaskXml `
            -TransactionId ([string]$stagedRecoveryReceipt.install_id) `
            -InstallationGuardName (Get-BrokerInstallationMutexName `
                -Root $testRoot) `
            -LauncherNonce $journalNonce `
            -GoalId 'goal_0123456789' -SourceFingerprint ('a' * 64) `
            -StagingObjects @([ordered]@{
                path = $journalOwned.path
                type = $journalOwned.type
                identity = $journalOwned.identity
                sha256 = $journalOwned.sha256
                owner_sid = $journalOwned.owner_sid
                access_sddl = $journalOwned.access_sddl
            })
        $recoveryJournalProbePath = Join-Path `
            $testRoot $script:UpgradeRecoveryJournalName
        Write-JsonAtomic -Path $recoveryJournalProbePath `
            -Document $recoveryJournalProbe -Security $fileSecurity
        $recoveryRead = Read-BrokerUpgradeRecoveryJournal `
            -Root $testRoot -Requests $recoveryRequests `
            -Name $recoveryTaskName -Account $recoveryAccount `
            -UserSid $testUserSid -Group $recoveryGroup `
            -SandboxSid $testSandboxSid -ExpectedFileSecurity $fileSecurity
        if ([string]$recoveryRead.LegacyReceipt.install_id -cne
                [string]$legacyRecoveryReceipt.install_id -or
            [string]$recoveryRead.StagedReceipt.install_id -cne
                [string]$stagedRecoveryReceipt.install_id) {
            throw 'Upgrade recovery journal self-test lost its exact receipt tuple.'
        }
        $validDebtState = Get-InstallationState `
            -Root $testRoot -Requests $recoveryRequests `
            -Name $recoveryTaskName
        if ([bool]$validDebtState.recovery_required -ne $true -or
            [string]$validDebtState.recovery_journal_state -cne
                ('present_valid_schema_' +
                    $script:UpgradeRecoveryJournalSchemaVersion) -or
            [bool]$validDebtState.operational_ready -ne $false) {
            throw 'Public -Check hid a valid pending upgrade recovery journal.'
        }
        $recoveryJournalProbe.staged_receipt_sha256 = '7' * 64
        Write-JsonAtomic -Path $recoveryJournalProbePath `
            -Document $recoveryJournalProbe -Replace -Security $fileSecurity
        & $assertRejected {
            [void](Read-BrokerUpgradeRecoveryJournal `
                -Root $testRoot -Requests $recoveryRequests `
                -Name $recoveryTaskName -Account $recoveryAccount `
                -UserSid $testUserSid -Group $recoveryGroup `
                -SandboxSid $testSandboxSid `
                -ExpectedFileSecurity $fileSecurity)
        } 'upgrade recovery journal receipt hash drift'
        & $assertRejected {
            Remove-BrokerUpgradeRecoveryJournal -Recovery $recoveryRead
        } 'stale recovery journal exact-delete binding'
        $recoveryJournalProbe.staged_receipt_sha256 = `
            Get-BytesSha256 -Bytes $stagedRecoveryBytes
        Write-JsonAtomic -Path $recoveryJournalProbePath `
            -Document $recoveryJournalProbe -Replace -Security $fileSecurity
        $recoveryCleanup = Read-BrokerUpgradeRecoveryJournal `
            -Root $testRoot -Requests $recoveryRequests `
            -Name $recoveryTaskName -Account $recoveryAccount `
            -UserSid $testUserSid -Group $recoveryGroup `
            -SandboxSid $testSandboxSid -ExpectedFileSecurity $fileSecurity
        Remove-BrokerUpgradeRecoveryJournal -Recovery $recoveryCleanup

        $stagedBytes = [IO.File]::ReadAllBytes($script:BrokerSource)
        $stagedHash = Get-BytesSha256 -Bytes $stagedBytes
        $stagedVersions = Join-Path $testRoot 'versions'
        [void](New-ManagedDirectory -Path $stagedVersions `
            -Security $readOnlySecurity -OwnerSid $testUserSid `
            -Label 'Interrupted upgrade self-test versions')
        $legacyBytes = [Text.UTF8Encoding]::new($false).GetBytes(
            'legacy schema-v2 worker'
        )
        $legacyHash = Get-BytesSha256 -Bytes $legacyBytes
        $legacyVersion = Join-Path $stagedVersions $legacyHash
        [void](New-ManagedDirectory -Path $legacyVersion `
            -Security $readOnlySecurity -OwnerSid $testUserSid `
            -Label 'Formal schema-v2 self-test version')
        $legacyWorkerPath = Join-Path $legacyVersion 'codex-powershell-broker.ps1'
        Write-BytesAtomic -Path $legacyWorkerPath -Bytes $legacyBytes `
            -Security $fileSecurity
        $legacyReceiptProbe = [pscustomobject]@{
            install_id = [Guid]::NewGuid().ToString('N')
            worker_path = $legacyWorkerPath
            worker_sha256 = $legacyHash
        }
        $stagedVersion = Join-Path $stagedVersions $stagedHash
        [void](New-ManagedDirectory -Path $stagedVersion `
            -Security $readOnlySecurity -OwnerSid $testUserSid `
            -Label 'Interrupted upgrade self-test version')
        Write-BytesAtomic `
            -Path (Join-Path $stagedVersion 'codex-powershell-broker.ps1') `
            -Bytes $stagedBytes -Security $fileSecurity
        $stagedTransactions = Join-Path $testRoot $script:GitTransactionDirectoryName
        [void](New-ManagedDirectory -Path $stagedTransactions `
            -Security $readOnlySecurity -OwnerSid $testUserSid `
            -Label 'Interrupted upgrade self-test transactions')
        Write-BytesAtomic -Path (Join-Path $stagedTransactions 'transaction.lock') `
            -Bytes ([byte[]]::new(0)) -Security $fileSecurity
        $stagedHooks = Join-Path $testRoot 'empty-hooks'
        [void](New-ManagedDirectory -Path $stagedHooks `
            -Security $readOnlySecurity -OwnerSid $testUserSid `
            -Label 'Interrupted upgrade self-test hooks')
        $stagedInstallId = [Guid]::NewGuid().ToString('N')
        $stagedManifest = New-GitCapabilityManifest `
            -InstallId $stagedInstallId -HooksRoot $stagedHooks `
            -TransactionsRoot $stagedTransactions
        Write-JsonAtomic `
            -Path (Join-Path $testRoot $script:GitCapabilityManifestName) `
            -Document $stagedManifest -Security $fileSecurity
        if (-not (Clear-ExactInterruptedUpgradeStaging `
            -Root $testRoot -LegacyReceipt $legacyReceiptProbe `
            -UserSid $testUserSid `
            -SandboxSid $testSandboxSid)) {
            throw 'Exact interrupted-upgrade staging was not recovered.'
        }
        foreach ($path in @(
            $stagedVersion, $stagedTransactions, $stagedHooks,
            (Join-Path $testRoot $script:GitCapabilityManifestName)
        )) {
            if (Test-Path -LiteralPath $path) {
                throw "Interrupted-upgrade self-test cleanup left staging: $path"
            }
        }
        $guardRoot = Join-Path $testRoot 'transaction-guard-self-test'
        [void](New-ManagedDirectory -Path $guardRoot `
            -Security $readOnlySecurity -OwnerSid $testUserSid `
            -Label 'Git transaction guard self-test root')
        $guardPath = Join-Path $guardRoot 'transaction.lock'
        Write-BytesAtomic -Path $guardPath -Bytes ([byte[]]::new(0)) `
            -Security $fileSecurity
        $guard = Open-GitTransactionGuard -Path $guardPath
        try {
            $heldGuardBinding = `
                New-BrokerUpgradeOwnedFileBindingFromHeldStream `
                    -Path $guardPath -Stream $guard `
                    -Label 'Held transaction guard self-test binding'
            if ([string]$heldGuardBinding.path -cne $guardPath -or
                [string]$heldGuardBinding.type -cne 'file' -or
                [string]$heldGuardBinding.sha256 -cne
                    (Get-BytesSha256 -Bytes ([byte[]]::new(0))) -or
                [string]$heldGuardBinding.identity -notmatch '^[0-9a-f]{16}:[0-9a-f]{32}$') {
                throw 'Held transaction guard binding self-test drifted.'
            }
            $competingGuardOpened = $false
            try {
                $competingGuard = Open-GitTransactionGuard -Path $guardPath
                $competingGuardOpened = $true
                $competingGuard.Dispose()
            } catch [IO.IOException] { }
            if ($competingGuardOpened) {
                throw 'Git transaction guard self-test allowed a competing writer.'
            }
            & $assertRejected {
                Assert-GitTransactionDirectorySafeForInstallerCleanup `
                    -Path $guardRoot
            } 'held Git transaction guard cleanup'
        } finally { $guard.Dispose() }
        Assert-GitTransactionDirectorySafeForInstallerCleanup -Path $guardRoot
        $journalProbe = Join-Path $guardRoot 'recovery.journal.json'
        [IO.File]::WriteAllText(
            $journalProbe, '{}', [Text.UTF8Encoding]::new($false)
        )
        & $assertRejected {
            Assert-GitTransactionDirectorySafeForInstallerCleanup `
                -Path $guardRoot
        } 'transaction recovery evidence cleanup'
        Remove-Item -LiteralPath $journalProbe -Force
        Remove-Item -LiteralPath $guardRoot -Recurse -Force

        $terminalRoot = Join-Path $testRoot 'terminal-journal-self-test'
        [void](New-ManagedDirectory -Path $terminalRoot `
            -Security $readOnlySecurity -OwnerSid $testUserSid `
            -Label 'Terminal journal self-test root')
        $terminalTransactions = Join-Path $terminalRoot `
            $script:GitTransactionDirectoryName
        $terminalResults = Join-Path $terminalRoot 'results'
        $terminalRequests = Join-Path $terminalRoot 'requests'
        foreach ($directory in @(
            $terminalTransactions, $terminalResults, $terminalRequests
        )) {
            [void](New-ManagedDirectory -Path $directory `
                -Security $readOnlySecurity -OwnerSid $testUserSid `
                -Label 'Terminal journal self-test directory')
        }
        $terminalLock = Join-Path $terminalTransactions 'transaction.lock'
        Write-BytesAtomic -Path $terminalLock -Bytes ([byte[]]::new(0)) `
            -Security $fileSecurity
        $terminalId = [Guid]::NewGuid().ToString('N')
        $terminalInstallId = [Guid]::NewGuid().ToString('N')
        $terminalRequestHash = '1' * 64
        $terminalHead = '2' * 40
        $terminalTree = '3' * 40
        $terminalCommit = '4' * 40
        $terminalOutput = [ordered]@{
            transaction_id = $terminalId
            repository_id = '5' * 32
            ref = 'refs/heads/main'
            parent = $terminalHead
            head_before = $terminalHead
            commit_oid = $terminalCommit
            tree_oid = $terminalTree
            head_after = $terminalCommit
            changed_paths = @('tracked.txt')
            changed_paths_digest = '6' * 64
            changed_path_count = 1
            commit_message_sha256 = '7' * 64
            index_before_sha256 = '8' * 64
            index_after_sha256 = '9' * 64
            mutation_phase = 'verified'
            recovery_state = 'none'
            git_sha256 = 'a' * 64
            remote_refs_sha256 = 'b' * 64
            other_refs_sha256 = 'c' * 64
            hooks_disabled = $true
            child_process_policy = 'single_process'
            network_operation = $false
            post_clean = $true
        }
        $terminalJournal = [ordered]@{
            schema_version = 1
            install_id = $terminalInstallId
            request_id = $terminalId
            request_sha256 = $terminalRequestHash
            phase = 'verified'
            created_at_utc = [DateTimeOffset]::UtcNow.ToString('o')
            updated_at_utc = [DateTimeOffset]::UtcNow.ToString('o')
            head_before = $terminalHead
            head_tree_before = 'd' * 40
            index_path = Join-Path (Join-Path $terminalRoot '.git') 'index'
            index_before_sha256 = '8' * 64
            alternate_index_path = Join-Path $terminalTransactions `
                ($terminalId + '.index')
            alternate_index_sha256 = '9' * 64
            stage_index_path = Join-Path (Join-Path $terminalRoot '.git') `
                'index.lock'
            backup_index_path = Join-Path (Join-Path $terminalRoot '.git') `
                ('.rayman-git-local-commit-' + $terminalId + '.backup')
            remote_refs_before_sha256 = 'b' * 64
            other_refs_before_sha256 = 'c' * 64
            tree_oid = $terminalTree
            commit_oid = $terminalCommit
            commit_bytes_base64 = [Convert]::ToBase64String(
                [Text.Encoding]::UTF8.GetBytes('commit')
            )
            commit_message_sha256 = '7' * 64
            output = $terminalOutput
        }
        $terminalReceipt = [pscustomobject]@{
            install_id = $terminalInstallId
            user_account = [string]$identity.Name
            user_sid = $testUserSid
            sandbox_group_sid = $testSandboxSid
            worker_sha256 = 'e' * 64
            powershell_sha256 = 'f' * 64
            request_root = $terminalRequests
            result_root = $terminalResults
        }
        $terminalResult = [ordered]@{
            schema_version = $script:SchemaVersion
            install_id = $terminalInstallId
            request_id = $terminalId
            operation = $script:GitCapabilityId
            status = 'success'
            exit_code = 0
            started_at_utc = [DateTimeOffset]::UtcNow.ToString('o')
            finished_at_utc = [DateTimeOffset]::UtcNow.ToString('o')
            executor_account = [string]$identity.Name
            executor_sid = $testUserSid
            worker_sha256 = 'e' * 64
            powershell_sha256 = 'f' * 64
            request_sha256 = $terminalRequestHash
            output = $terminalOutput
            error_code = ''
            error = ''
        }
        $terminalJournalPath = Join-Path $terminalTransactions `
            ($terminalId + '.journal.json')
        $terminalResultPath = Join-Path $terminalResults `
            ($terminalId + '.result.json')
        $terminalSecondId = [Guid]::NewGuid().ToString('N')
        $terminalSecondOutput = [ordered]@{}
        foreach ($entry in $terminalOutput.GetEnumerator()) {
            $terminalSecondOutput[$entry.Key] = $entry.Value
        }
        $terminalSecondOutput.transaction_id = $terminalSecondId
        $terminalSecondJournal = [ordered]@{}
        foreach ($entry in $terminalJournal.GetEnumerator()) {
            $terminalSecondJournal[$entry.Key] = $entry.Value
        }
        $terminalSecondJournal.request_id = $terminalSecondId
        $terminalSecondJournal.alternate_index_path = Join-Path `
            $terminalTransactions ($terminalSecondId + '.index')
        $terminalSecondJournal.backup_index_path = Join-Path `
            (Join-Path $terminalRoot '.git') `
            ('.rayman-git-local-commit-' + $terminalSecondId + '.backup')
        $terminalSecondJournal.output = $terminalSecondOutput
        $terminalSecondResult = [ordered]@{}
        foreach ($entry in $terminalResult.GetEnumerator()) {
            $terminalSecondResult[$entry.Key] = $entry.Value
        }
        $terminalSecondResult.request_id = $terminalSecondId
        $terminalSecondResult.output = $terminalSecondOutput
        $terminalSecondJournalPath = Join-Path $terminalTransactions `
            ($terminalSecondId + '.journal.json')
        $terminalSecondResultPath = Join-Path $terminalResults `
            ($terminalSecondId + '.result.json')
        $terminalPairs = @(
            [pscustomobject]@{
                JournalPath = $terminalJournalPath
                Journal = $terminalJournal
                ResultPath = $terminalResultPath
                Result = $terminalResult
            },
            [pscustomobject]@{
                JournalPath = $terminalSecondJournalPath
                Journal = $terminalSecondJournal
                ResultPath = $terminalSecondResultPath
                Result = $terminalSecondResult
            }
        )
        foreach ($pair in $terminalPairs) {
            Write-JsonAtomic -Path ([string]$pair.JournalPath) `
                -Document $pair.Journal
            Write-JsonAtomic -Path ([string]$pair.ResultPath) `
                -Document $pair.Result
        }
        $inheritedSnapshots = [Collections.Generic.List[object]]::new()
        foreach ($pair in $terminalPairs) {
            $inheritedSnapshots.Add((Get-BrokerFileSnapshot `
                -Path ([string]$pair.JournalPath) `
                -Label 'Production-inherited terminal journal simulation'))
            $inheritedSnapshots.Add((Get-BrokerFileSnapshot `
                -Path ([string]$pair.ResultPath) `
                -Label 'Production-inherited terminal result simulation'))
        }
        foreach ($snapshot in $inheritedSnapshots) {
            if ([bool]$snapshot.AccessRulesProtected -or
                -not (Test-BrokerExactInheritedFileSecurity `
                    -AccessSddl ([string]$snapshot.AccessSddl) `
                    -Expected $fileSecurity)) {
                throw 'Production-inherited terminal artifact simulation did not reproduce the worker ACL shape.'
            }
        }
        $terminalState = Get-GitTransactionOperationalState `
            -Path $terminalTransactions -Root $terminalRoot `
            -Receipt $terminalReceipt -FileSecurity $fileSecurity `
            -UserSid $testUserSid -RepositoryRoot $terminalRoot
        $terminalGuard = Open-GitTransactionGuard -Path $terminalLock
        try {
            $retiredCount = Remove-TerminalGitTransactionJournalsForUpgrade `
                -ExpectedState $terminalState -Path $terminalTransactions `
                -Root $terminalRoot -Receipt $terminalReceipt `
                -FileSecurity $fileSecurity -UserSid $testUserSid `
                -RepositoryRoot $terminalRoot
        } finally { $terminalGuard.Dispose() }
        $remainingTerminalJournals = @($terminalPairs | Where-Object {
            Test-Path -LiteralPath ([string]$_.JournalPath)
        })
        $missingTerminalResults = @($terminalPairs | Where-Object {
            -not (Test-Path -LiteralPath ([string]$_.ResultPath) -PathType Leaf)
        })
        if ($retiredCount -ne 2 -or
            $remainingTerminalJournals.Count -ne 0 -or
            $missingTerminalResults.Count -ne 0) {
            throw 'Terminal journal self-test did not retire exactly two verified inherited journals while preserving both results.'
        }
        Write-JsonAtomic -Path $terminalJournalPath `
            -Document $terminalJournal -Security $fileSecurity
        $terminalResult.request_sha256 = '0' * 64
        Write-JsonAtomic -Path $terminalResultPath `
            -Document $terminalResult -Replace -Security $fileSecurity
        & $assertRejected {
            [void](Get-GitTransactionOperationalState `
                -Path $terminalTransactions -Root $terminalRoot `
                -Receipt $terminalReceipt -FileSecurity $fileSecurity `
                -UserSid $testUserSid -RepositoryRoot $terminalRoot)
        } 'terminal journal mismatched result'
        if (-not (Test-Path -LiteralPath $terminalJournalPath -PathType Leaf)) {
            throw 'Terminal journal mismatch self-test removed recovery evidence.'
        }
        $terminalResult.request_sha256 = $terminalRequestHash
        Write-JsonAtomic -Path $terminalResultPath `
            -Document $terminalResult -Replace -Security $fileSecurity
        $terminalJournal.phase = 'index_published'
        Write-JsonAtomic -Path $terminalJournalPath `
            -Document $terminalJournal -Replace -Security $fileSecurity
        & $assertRejected {
            [void](Get-GitTransactionOperationalState `
                -Path $terminalTransactions -Root $terminalRoot `
                -Receipt $terminalReceipt -FileSecurity $fileSecurity `
                -UserSid $testUserSid -RepositoryRoot $terminalRoot)
        } 'nonterminal journal retirement'
        if (-not (Test-Path -LiteralPath $terminalJournalPath -PathType Leaf)) {
            throw 'Nonterminal journal self-test removed recovery evidence.'
        }

        $terminalJournal.phase = 'verified'
        Write-JsonAtomic -Path $terminalJournalPath `
            -Document $terminalJournal -Replace
        Write-JsonAtomic -Path $terminalResultPath `
            -Document $terminalResult -Replace
        $journalAclDrift = Get-Acl -LiteralPath $terminalJournalPath
        Add-ManagedRule -Security $journalAclDrift -Sid 'S-1-5-32-545' `
            -Rights ([Security.AccessControl.FileSystemRights]::ReadData)
        Set-Acl -LiteralPath $terminalJournalPath -AclObject $journalAclDrift
        & $assertRejected {
            [void](Get-GitTransactionOperationalState `
                -Path $terminalTransactions -Root $terminalRoot `
                -Receipt $terminalReceipt -FileSecurity $fileSecurity `
                -UserSid $testUserSid -RepositoryRoot $terminalRoot)
        } 'terminal inherited journal with explicit ACL drift'
        if (-not (Test-Path -LiteralPath $terminalJournalPath -PathType Leaf)) {
            throw 'Journal ACL-drift self-test removed recovery evidence.'
        }

        Write-JsonAtomic -Path $terminalJournalPath `
            -Document $terminalJournal -Replace
        $resultAclDrift = Get-Acl -LiteralPath $terminalResultPath
        Add-ManagedRule -Security $resultAclDrift -Sid 'S-1-5-32-545' `
            -Rights ([Security.AccessControl.FileSystemRights]::ReadData)
        Set-Acl -LiteralPath $terminalResultPath -AclObject $resultAclDrift
        & $assertRejected {
            [void](Get-GitTransactionOperationalState `
                -Path $terminalTransactions -Root $terminalRoot `
                -Receipt $terminalReceipt -FileSecurity $fileSecurity `
                -UserSid $testUserSid -RepositoryRoot $terminalRoot)
        } 'terminal inherited result with explicit ACL drift'
        if (-not (Test-Path -LiteralPath $terminalJournalPath -PathType Leaf) -or
            -not (Test-Path -LiteralPath $terminalResultPath -PathType Leaf)) {
            throw 'Result ACL-drift self-test removed terminal evidence.'
        }
        Remove-Item -LiteralPath $terminalRoot -Recurse -Force

        [void](New-ManagedDirectory -Path $stagedHooks `
            -Security $readOnlySecurity -OwnerSid $testUserSid `
            -Label 'Incomplete interrupted upgrade self-test hooks')
        & $assertRejected {
            [void](Clear-ExactInterruptedUpgradeStaging `
                -Root $testRoot -LegacyReceipt $legacyReceiptProbe `
                -UserSid $testUserSid `
                -SandboxSid $testSandboxSid)
        } 'incomplete interrupted upgrade staging'
        Remove-Item -LiteralPath $stagedHooks -Recurse -Force

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
        $duplicateJson = Join-Path $testRoot 'duplicate.json'
        [IO.File]::WriteAllText(
            $duplicateJson, '{"schema_version":3,"Schema_Version":3}',
            [Text.UTF8Encoding]::new($false)
        )
        & $assertRejected {
            [void](Read-JsonDocument -Path $duplicateJson `
                -Label 'Installer duplicate JSON self-test')
        } 'installer case-colliding duplicate JSON property'
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

if ($UpgradeCrashSelfTestChild) {
    Invoke-BrokerUpgradeCrashSelfTestChild `
        -Token $UpgradeCrashSelfTestToken -Phase $UpgradeCrashSelfTestPhase
    throw 'Upgrade crash self-test child returned unexpectedly.'
}
if ($RecoverySelfTestChild) {
    try {
        Invoke-BrokerRecoverySelfTestChild -Token $RecoverySelfTestToken
        return
    } catch {
        [Console]::Error.WriteLine($_.Exception.ToString())
        exit 1
    }
}
if ($SelfTest) { Invoke-SelfTest; return }
$confirmedAction = if ($Install) { 'Broker install' }
    elseif ($Upgrade) { 'Broker upgrade' }
    elseif ($Uninstall) { 'Broker uninstall' }
    elseif ($RecoverPartialUninstall) { 'Broker partial-uninstall recovery' }
    elseif ($PrepareUpgradeLauncher) { 'Broker upgrade launcher preparation' }
    elseif ($PrepareInstallLauncher) { 'Broker install launcher preparation' }
    else { $null }
if ($null -ne $confirmedAction) {
    Assert-BrokerExplicitConfirmation `
        -Confirmed $Yes.IsPresent -Action $confirmedAction
}
if ($PrepareUpgradeLauncher -or $PrepareInstallLauncher) {
    $launcherAction = if ($PrepareInstallLauncher) { 'install' } else { 'upgrade' }
    $forbiddenLauncherParameters = @(
        'InstallRoot', 'RequestRoot', 'TaskName', 'UserAccount', 'SandboxGroup'
    ) | Where-Object { $PSBoundParameters.ContainsKey($_) }
    if (@($forbiddenLauncherParameters).Count -ne 0) {
        throw "Prepare $launcherAction launcher rejects dynamic install parameters: $($forbiddenLauncherParameters -join ', ')"
    }
    (Prepare-BrokerUpgradeLauncher -Action $launcherAction `
        -SourceFingerprint $ExpectedSourceFingerprint `
        -GoalId $ExpectedGoalId) | ConvertTo-Json -Depth 8
    return
}

$normalizedInstall = Get-NormalizedAbsolutePath -Path $InstallRoot -Label 'Broker install root'
$normalizedRequests = Get-NormalizedAbsolutePath -Path $RequestRoot -Label 'Broker request root'
$installationGuard = Open-BrokerInstallationGuard `
    -Root $normalizedInstall `
    -TimeoutMilliseconds $(if ($Check) { 0 } else { 30000 }) `
    -AllowBusy:$Check
try {
    if ($Check -and -not [bool]$installationGuard.Acquired) {
        (Get-InstallationState -Root $normalizedInstall `
            -Requests $normalizedRequests -Name $TaskName `
            -TransactionInProgress) | ConvertTo-Json -Depth 8
        return
    }
    if ($RecoverPartialUninstall) {
        (Recover-PartialUninstall -Root $normalizedInstall -Requests $normalizedRequests `
            -Name $TaskName -Account $UserAccount -Group $SandboxGroup) |
            ConvertTo-Json -Depth 8
        return
    }
    if ($Check) {
        (Get-InstallationState -Root $normalizedInstall `
            -Requests $normalizedRequests -Name $TaskName) |
            ConvertTo-Json -Depth 8
        return
    }
    if ($Install) {
        $installAuthority = $null
        $installRootExists = Test-Path -LiteralPath $normalizedInstall
        $authorityValues = @(
            $InstallAuthorityManifestPath,
            $InstallAuthorityManifestSha256,
            $ExpectedGoalId,
            $ExpectedSourceFingerprint
        )
        if ($installRootExists) {
            if (@($authorityValues | Where-Object {
                    -not [string]::IsNullOrWhiteSpace([string]$_)
                }).Count -ne 0) {
                throw 'Existing-install attestation rejects fresh-install authority arguments.'
            }
        } elseif (@($authorityValues | Where-Object {
                [string]::IsNullOrWhiteSpace([string]$_)
            }).Count -ne 0) {
            throw 'Fresh install requires the complete one-shot authority tuple.'
        } else {
            $installAuthority = Open-BrokerUpgradeAuthority -Action install `
                -ManifestPath $InstallAuthorityManifestPath `
                -ManifestSha256 $InstallAuthorityManifestSha256 `
                -GoalId $ExpectedGoalId `
                -SourceFingerprint $ExpectedSourceFingerprint `
                -Root $normalizedInstall -Requests $normalizedRequests `
                -Name $TaskName -Account $UserAccount `
                -Group $SandboxGroup -InstallationGuard $installationGuard
        }
        try {
            (Install-Broker -Root $normalizedInstall `
                -Requests $normalizedRequests -Name $TaskName `
                -Account $UserAccount -Group $SandboxGroup `
                -SourceAuthority $installAuthority) | ConvertTo-Json -Depth 8
        } finally {
            if ($null -ne $installAuthority) {
                Close-BrokerUpgradeAuthority -Authority $installAuthority
            }
        }
        return
    }
    if ($Upgrade) {
        (Upgrade-Broker -Root $normalizedInstall -Requests $normalizedRequests `
            -Name $TaskName -Account $UserAccount -Group $SandboxGroup `
            -AuthorityManifestPath $UpgradeAuthorityManifestPath `
            -AuthorityManifestSha256 $UpgradeAuthorityManifestSha256 `
            -GoalId $ExpectedGoalId `
            -SourceFingerprint $ExpectedSourceFingerprint `
            -InstallationGuard $installationGuard) | ConvertTo-Json -Depth 8
        return
    }
    if ($Uninstall) {
        (Uninstall-Broker -Root $normalizedInstall -Requests $normalizedRequests `
            -Name $TaskName -Account $UserAccount) | ConvertTo-Json -Depth 8
    }
} finally {
    Close-BrokerInstallationGuard -Guard $installationGuard
}
