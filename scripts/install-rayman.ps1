[CmdletBinding()]
param(
    [string]$BinDirectory,

    [string]$SkillDirectory,

    [switch]$AddToUserPath,

    [switch]$SkipCodexStopHook,

    [switch]$SelfTest,

    [switch]$Yes
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if ($PSVersionTable.PSVersion.Major -lt 7) {
    throw 'install-rayman.ps1 requires PowerShell 7+. Run it with pwsh, not Windows PowerShell.'
}
if (-not $IsWindows -and -not $IsLinux) {
    throw 'install-rayman.ps1 supports identity-bound installation only on Windows and Linux. Build and release verification remain available on other platforms, but installation fails closed before writing.'
}
if ($IsLinux -and
    [Runtime.InteropServices.RuntimeInformation]::ProcessArchitecture -notin @(
        [Runtime.InteropServices.Architecture]::X64,
        [Runtime.InteropServices.Architecture]::Arm64
    )) {
    throw 'Linux identity-bound installation requires a native x64 or ARM64 PowerShell process; refusing before any installer write.'
}
if ($AddToUserPath -and -not $IsWindows) {
    throw '-AddToUserPath is supported only on Windows. Configure PATH before installation on this platform.'
}

$repoRoot = Split-Path -Parent $PSScriptRoot
$artifactName = if ($IsWindows) { 'rayman.exe' } else { 'rayman' }
# Set under StrictMode before any read: true only when nothing resolved `rayman`
# and the destination was made reachable for the post-install verification.
$script:processPathOnly = $false
if ([string]::IsNullOrWhiteSpace($BinDirectory)) {
    $BinDirectory = if ($IsWindows) {
        Join-Path $env:LOCALAPPDATA 'Rayman/bin'
    } else {
        Join-Path $HOME '.local/bin'
    }
}
if ([string]::IsNullOrWhiteSpace($SkillDirectory)) {
    $SkillDirectory = Join-Path $HOME '.codex/skills/raymancodingskill'
}

function Get-CodexSkillResourcePlan {
    param([Parameter(Mandatory = $true)][string]$DestinationRoot)

    $manifestPath = Join-Path $repoRoot 'install-manifest.json'
    if (-not (Test-Path -LiteralPath $manifestPath -PathType Leaf)) {
        throw "Install manifest is missing: $manifestPath"
    }
    try {
        $manifest = Get-Content -LiteralPath $manifestPath -Raw -Encoding utf8 |
            ConvertFrom-Json -ErrorAction Stop
    } catch {
        throw "Install manifest is invalid: $($_.Exception.Message)"
    }
    if ($manifest.schema_version -ne 1 -or
        $manifest.clients.codex.deployment_scope -ne 'global_skill' -or
        $manifest.clients.claude_code.deployment_scope -ne 'repository_entrypoint_only' -or
        $manifest.clients.claude_code.entrypoint -ne 'CLAUDE.md') {
        throw 'Install manifest client deployment scopes are invalid.'
    }

    $resources = @($manifest.codex_skill_resources)
    if ($resources.Count -eq 0) {
        throw 'Install manifest has no Codex skill resources.'
    }
    $plan = @()
    foreach ($resource in $resources) {
        $properties = @($resource.PSObject.Properties.Name | Sort-Object)
        if (($properties -join ',') -ne 'destination,source') {
            throw 'Install manifest resource has unknown or missing fields.'
        }
        $sourceRelative = [string]$resource.source
        $destinationRelative = [string]$resource.destination
        foreach ($candidate in @($sourceRelative, $destinationRelative)) {
            if ([string]::IsNullOrWhiteSpace($candidate) -or
                [IO.Path]::IsPathRooted($candidate) -or
                $candidate.Contains('..') -or
                $candidate.Contains('\')) {
                throw "Install manifest path must be an ordinary forward-slash relative path: $candidate"
            }
        }
        # Both ends must be checked: a manifest entry can name any source and
        # any destination, so guarding the source alone still allowed
        # `{source: X, destination: CLAUDE.md}` to deploy a repository-scoped
        # entrypoint globally. The enforcing twin check existed only in
        # -SelfTest, which is not what runs during an install.
        if ($sourceRelative -eq 'CLAUDE.md' -or $destinationRelative -eq 'CLAUDE.md') {
            throw 'CLAUDE.md is repository-scoped and must not be globally installed.'
        }
        $source = [IO.Path]::GetFullPath((Join-Path $repoRoot $sourceRelative))
        if (-not (Test-Path -LiteralPath $source -PathType Leaf)) {
            throw "Install manifest source is missing: $sourceRelative"
        }
        $destination = [IO.Path]::GetFullPath(
            (Join-Path $DestinationRoot $destinationRelative)
        )
        $destinationPrefix = $DestinationRoot.TrimEnd(
            [IO.Path]::DirectorySeparatorChar
        ) + [IO.Path]::DirectorySeparatorChar
        if (-not $destination.StartsWith(
            $destinationPrefix,
            $(if ($IsWindows) { [StringComparison]::OrdinalIgnoreCase } else { [StringComparison]::Ordinal })
        )) {
            throw "Install manifest destination escaped the skill root: $destinationRelative"
        }
        $plan += [pscustomobject]@{
            SourceRelative = $sourceRelative
            DestinationRelative = $destinationRelative
            Source = $source
            Destination = $destination
        }
    }
    return $plan
}
function Get-CanonicalSkillResource {
    param([Parameter(Mandatory = $true)][array]$ResourcePlan)

    $resources = @(
        $ResourcePlan |
            Where-Object { $_.DestinationRelative -eq 'SKILL.md' }
    )
    if ($resources.Count -ne 1) {
        throw 'Install manifest must contain exactly one canonical SKILL.md destination.'
    }
    return $resources[0]
}


function Invoke-NativeChecked {
    param(
        [Parameter(Mandatory = $true)]
        [string]$FilePath,
        [string[]]$Arguments
    )

    & $FilePath @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "$FilePath $($Arguments -join ' ') failed with exit code $LASTEXITCODE."
    }
}

function Invoke-NativeJsonChecked {
    param(
        [Parameter(Mandatory = $true)]
        [string]$FilePath,
        [string[]]$Arguments
    )

    $output = @(& $FilePath @Arguments 2>&1)
    $exitCode = $LASTEXITCODE
    $text = ($output | Out-String).Trim()
    if ($exitCode -ne 0) {
        throw "$FilePath $($Arguments -join ' ') failed with exit code ${exitCode}:`n$text"
    }
    try {
        return $text | ConvertFrom-Json -ErrorAction Stop
    } catch {
        throw "$FilePath $($Arguments -join ' ') returned invalid JSON: $text"
    }
}

function Invoke-NativeCheckedInDirectory {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Directory,
        [Parameter(Mandatory = $true)]
        [string]$FilePath,
        [string[]]$Arguments
    )

    $startingLocation = (Get-Location).Path
    $pushedLocation = $false
    try {
        Push-Location $Directory
        $pushedLocation = $true
        Invoke-NativeChecked -FilePath $FilePath -Arguments $Arguments
    } finally {
        if ($pushedLocation) {
            Pop-Location
        }
    }
    $comparison = if ($IsWindows) {
        [StringComparison]::OrdinalIgnoreCase
    } else {
        [StringComparison]::Ordinal
    }
    if (-not (Get-Location).Path.Equals($startingLocation, $comparison)) {
        throw 'Native workspace invocation did not restore the caller working directory.'
    }
}

function Resolve-RequiredApplication {
    param([string]$Name, [string]$Label)

    $commands = @(Get-Command $Name -All -ErrorAction Stop)
    if ($commands.Count -eq 0 -or $commands[0].CommandType -ne 'Application') {
        $kind = if ($commands.Count -eq 0) { 'missing' } else { $commands[0].CommandType }
        throw "$Label must resolve directly to an Application; effective command is $kind."
    }
    $path = $commands[0].Source
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
        throw "$Label is missing: $path"
    }
    return (Resolve-Path -LiteralPath $path).ProviderPath
}

function Get-PathComparisonKey {
    param([string]$Entry)

    if ([string]::IsNullOrWhiteSpace($Entry)) {
        return ''
    }
    $candidate = [Environment]::ExpandEnvironmentVariables($Entry.Trim().Trim('"'))
    try {
        $candidate = [IO.Path]::GetFullPath($candidate)
    } catch {
        # Preserve an unusual PATH entry for comparison instead of making path
        # normalization itself an installation side effect.
    }
    $root = [IO.Path]::GetPathRoot($candidate)
    if ($candidate -ne $root) {
        $candidate = $candidate.TrimEnd(
            [IO.Path]::DirectorySeparatorChar,
            [IO.Path]::AltDirectorySeparatorChar
        )
    }
    if ($IsWindows) {
        return $candidate.ToLowerInvariant()
    }
    return $candidate
}

function Get-ProposedUserPath {
    param([AllowNull()][string]$ExistingUserPath, [string]$CliDirectory)

    $targetKey = Get-PathComparisonKey -Entry $CliDirectory
    $entries = @(
        $ExistingUserPath -split [IO.Path]::PathSeparator |
            Where-Object { -not [string]::IsNullOrWhiteSpace($_) }
    )
    $remaining = @($entries | Where-Object {
        (Get-PathComparisonKey -Entry $_) -ne $targetKey
    })
    return (@($CliDirectory) + $remaining) -join [IO.Path]::PathSeparator
}

function Get-ProjectedPersistentPath {
    param(
        [AllowNull()][string]$MachinePath,
        [AllowNull()][string]$UserPath
    )

    $parts = @()
    foreach ($part in @($MachinePath, $UserPath)) {
        if (-not [string]::IsNullOrWhiteSpace($part)) {
            $parts += $part
        }
    }
    return [Environment]::ExpandEnvironmentVariables(
        ($parts -join [IO.Path]::PathSeparator)
    )
}

function Publish-EnvironmentChangeBroadcast {
    # [Environment]::SetEnvironmentVariable(...,'User') broadcasts WM_SETTINGCHANGE
    # for the caller. The registry writes below deliberately bypass that API to
    # preserve REG_EXPAND_SZ, so the notification has to be reissued here.
    # It is advisory only: a failed broadcast must not fail or unwind an install.
    if (-not $IsWindows) {
        return
    }
    try {
        if (-not ('RaymanInstaller.EnvironmentBroadcast' -as [type])) {
            Add-Type -Namespace 'RaymanInstaller' -Name 'EnvironmentBroadcast' -MemberDefinition @'
[System.Runtime.InteropServices.DllImport("user32.dll", SetLastError = true, CharSet = System.Runtime.InteropServices.CharSet.Unicode)]
public static extern System.IntPtr SendMessageTimeoutW(
    System.IntPtr hWnd,
    uint Msg,
    System.IntPtr wParam,
    string lParam,
    uint fuFlags,
    uint uTimeout,
    out System.UIntPtr lpdwResult);
'@
        }
        $result = [UIntPtr]::Zero
        $null = [RaymanInstaller.EnvironmentBroadcast]::SendMessageTimeoutW(
            [IntPtr]0xffff, # HWND_BROADCAST
            0x1A,           # WM_SETTINGCHANGE
            [IntPtr]::Zero,
            'Environment',
            0x0002,         # SMTO_ABORTIFHUNG
            5000,
            [ref]$result
        )
    } catch {
        Write-Warning "Persistent environment change could not be broadcast to running processes: $($_.Exception.Message)"
    }
}

function Initialize-WindowsRegistryCasNative {
    if ('RaymanInstallerV2.WindowsRegistryTransaction' -as [type]) {
        return
    }
    if (-not $IsWindows) {
        throw 'Windows registry compare-exchange is available only on Windows.'
    }

    Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.Runtime.InteropServices;
using Microsoft.Win32;
using Microsoft.Win32.SafeHandles;

namespace RaymanInstallerV2
{
    public sealed class WindowsRegistryStringValue
    {
        public bool Exists { get; private set; }
        public string Value { get; private set; }
        public RegistryValueKind Kind { get; private set; }

        private WindowsRegistryStringValue(bool exists, string value, RegistryValueKind kind)
        {
            Exists = exists;
            Value = value;
            Kind = kind;
        }

        internal static WindowsRegistryStringValue Missing()
        {
            return new WindowsRegistryStringValue(false, null, RegistryValueKind.ExpandString);
        }

        internal static WindowsRegistryStringValue Present(string value, RegistryValueKind kind)
        {
            return new WindowsRegistryStringValue(true, value, kind);
        }
    }

    public sealed class WindowsRegistryTransaction : IDisposable
    {
        private const int ErrorSuccess = 0;
        private const int KeyQueryValue = 0x0001;
        private const int KeySetValue = 0x0002;
        private const uint TransactionTimeoutMilliseconds = 15000;
        private static readonly IntPtr HkeyCurrentUser = new IntPtr(unchecked((int)0x80000001));

        [DllImport("KtmW32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
        private static extern SafeFileHandle CreateTransaction(
            IntPtr transactionAttributes,
            IntPtr unitOfWork,
            uint createOptions,
            uint isolationLevel,
            uint isolationFlags,
            uint timeout,
            string description);

        [DllImport("KtmW32.dll", SetLastError = true)]
        [return: MarshalAs(UnmanagedType.Bool)]
        private static extern bool CommitTransaction(SafeFileHandle transactionHandle);

        [DllImport("KtmW32.dll", SetLastError = true)]
        [return: MarshalAs(UnmanagedType.Bool)]
        private static extern bool RollbackTransaction(SafeFileHandle transactionHandle);

        [DllImport("advapi32.dll", EntryPoint = "RegOpenKeyTransactedW", CharSet = CharSet.Unicode)]
        private static extern int RegOpenKeyTransacted(
            IntPtr key,
            string subKey,
            int options,
            int desiredAccess,
            out SafeRegistryHandle result,
            SafeFileHandle transactionHandle,
            IntPtr extendedParameter);

        private SafeFileHandle transactionHandle;
        private RegistryKey key;
        private bool committed;
        private bool disposed;

        private WindowsRegistryTransaction(SafeFileHandle transactionHandle, RegistryKey key)
        {
            this.transactionHandle = transactionHandle;
            this.key = key;
        }

        public static WindowsRegistryTransaction OpenCurrentUser(string subKey)
        {
            SafeFileHandle transaction = CreateTransaction(
                IntPtr.Zero,
                IntPtr.Zero,
                0,
                0,
                0,
                TransactionTimeoutMilliseconds,
                "Rayman installer registry compare-exchange");
            if (transaction == null || transaction.IsInvalid)
            {
                int error = Marshal.GetLastWin32Error();
                if (transaction != null)
                {
                    transaction.Dispose();
                }
                throw new Win32Exception(error, "Unable to create a Windows registry transaction.");
            }

            try
            {
                SafeRegistryHandle keyHandle;
                int status = RegOpenKeyTransacted(
                    HkeyCurrentUser,
                    subKey,
                    0,
                    KeyQueryValue | KeySetValue,
                    out keyHandle,
                    transaction,
                    IntPtr.Zero);
                if (status != ErrorSuccess)
                {
                    if (keyHandle != null)
                    {
                        keyHandle.Dispose();
                    }
                    throw new Win32Exception(status, "Unable to open the transacted HKCU registry key.");
                }
                try
                {
                    RegistryKey registryKey = RegistryKey.FromHandle(keyHandle, RegistryView.Default);
                    return new WindowsRegistryTransaction(transaction, registryKey);
                }
                catch
                {
                    keyHandle.Dispose();
                    throw;
                }
            }
            catch
            {
                transaction.Dispose();
                throw;
            }
        }

        public WindowsRegistryStringValue ReadStringValue(string name)
        {
            ThrowIfUnavailable();
            object raw = key.GetValue(name, null, RegistryValueOptions.DoNotExpandEnvironmentNames);
            if (raw == null)
            {
                return WindowsRegistryStringValue.Missing();
            }
            RegistryValueKind kind = key.GetValueKind(name);
            if ((kind != RegistryValueKind.String && kind != RegistryValueKind.ExpandString) || !(raw is string))
            {
                throw new InvalidOperationException(
                    "The transacted registry value is not REG_SZ or REG_EXPAND_SZ.");
            }
            return WindowsRegistryStringValue.Present((string)raw, kind);
        }

        public void SetStringValue(string name, string value, RegistryValueKind kind)
        {
            ThrowIfUnavailable();
            if (kind != RegistryValueKind.String && kind != RegistryValueKind.ExpandString)
            {
                throw new InvalidOperationException("Refusing a non-string registry value kind.");
            }
            key.SetValue(name, value, kind);
        }

        public void DeleteValue(string name)
        {
            ThrowIfUnavailable();
            key.DeleteValue(name, false);
        }

        public void Commit()
        {
            ThrowIfUnavailable();
            CloseKey();
            if (!CommitTransaction(transactionHandle))
            {
                throw new Win32Exception(
                    Marshal.GetLastWin32Error(),
                    "The Windows registry transaction did not commit.");
            }
            committed = true;
        }

        private void ThrowIfUnavailable()
        {
            if (disposed || committed || transactionHandle == null || transactionHandle.IsInvalid)
            {
                throw new ObjectDisposedException("WindowsRegistryTransaction");
            }
        }

        private void CloseKey()
        {
            if (key != null)
            {
                key.Dispose();
                key = null;
            }
        }

        public void Dispose()
        {
            if (disposed)
            {
                return;
            }
            CloseKey();
            if (!committed && transactionHandle != null &&
                !transactionHandle.IsInvalid && !transactionHandle.IsClosed)
            {
                RollbackTransaction(transactionHandle);
            }
            if (transactionHandle != null)
            {
                transactionHandle.Dispose();
                transactionHandle = null;
            }
            disposed = true;
        }
    }
}
'@
}

function Assert-PersistentUserEnvironmentCasCapability {
    $transaction = $null
    try {
        Initialize-WindowsRegistryCasNative
        $transaction = [RaymanInstallerV2.WindowsRegistryTransaction]::OpenCurrentUser('Environment')
        $null = $transaction.ReadStringValue('Path')
        $transaction.Commit()
    } catch {
        throw "Windows TxR/KTM registry compare-exchange is required by -AddToUserPath and failed before managed files were changed. Repair Windows transactional-registry support, or rerun without -AddToUserPath and manage PATH externally: $($_.Exception.Message)"
    } finally {
        if ($null -ne $transaction) {
            $transaction.Dispose()
        }
    }
}

function Get-PersistentUserEnvironmentRecord {
    param(
        [Parameter(Mandatory = $true)][string]$Name,
        [AllowNull()]$TestStore
    )

    if ($null -ne $TestStore) {
        if (-not $TestStore.ContainsKey($Name)) {
            return [pscustomobject]@{
                Name = $Name
                Exists = $false
                Value = $null
                Kind = [Microsoft.Win32.RegistryValueKind]::ExpandString
            }
        }
        $stored = $TestStore[$Name]
        return [pscustomobject]@{
            Name = $Name
            Exists = [bool]$stored.Exists
            Value = [string]$stored.Value
            Kind = $stored.Kind
        }
    }

    # [Environment]::GetEnvironmentVariable(...,'User') expands %VAR% references,
    # and its setter always writes REG_SZ. Reading and writing HKCU\Environment
    # directly is the only way to round-trip a user PATH without baking every
    # %JAVA_HOME%-style entry into a literal and downgrading the value type.
    $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment', $false)
    if ($null -eq $key) {
        return [pscustomobject]@{
            Name = $Name
            Exists = $false
            Value = $null
            Kind = [Microsoft.Win32.RegistryValueKind]::ExpandString
        }
    }
    try {
        $value = $key.GetValue(
            $Name,
            $null,
            [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames
        )
        if ($null -eq $value) {
            return [pscustomobject]@{
                Name = $Name
                Exists = $false
                Value = $null
                Kind = [Microsoft.Win32.RegistryValueKind]::ExpandString
            }
        }
        $kind = $key.GetValueKind($Name)
        if ($kind -ne [Microsoft.Win32.RegistryValueKind]::String -and
            $kind -ne [Microsoft.Win32.RegistryValueKind]::ExpandString) {
            throw "HKCU\Environment\$Name has unsupported registry value kind '$kind'; refusing to rewrite it."
        }
        return [pscustomobject]@{
            Name = $Name
            Exists = $true
            Value = [string]$value
            Kind = $kind
        }
    } finally {
        $key.Dispose()
    }
}

function Set-PersistentUserEnvironmentRecord {
    param(
        [Parameter(Mandatory = $true)]$Record,
        [switch]$Broadcast,
        [AllowNull()]$TestStore
    )

    if ($null -ne $TestStore) {
        if ($Record.Exists) {
            if ($Record.Kind -ne [Microsoft.Win32.RegistryValueKind]::String -and
                $Record.Kind -ne [Microsoft.Win32.RegistryValueKind]::ExpandString) {
                throw "Refusing to write test environment record '$($Record.Name)' with registry value kind '$($Record.Kind)'."
            }
            $TestStore[$Record.Name] = [pscustomobject]@{
                Name = [string]$Record.Name
                Exists = $true
                Value = [string]$Record.Value
                Kind = $Record.Kind
            }
        } else {
            $null = $TestStore.Remove([string]$Record.Name)
        }
        return
    }

    $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment', $true)
    if ($null -eq $key) {
        $key = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey('Environment')
    }
    if ($null -eq $key) {
        throw 'Unable to open HKCU\Environment for the persistent user environment update.'
    }
    try {
        if ($Record.Exists) {
            if ($Record.Kind -ne [Microsoft.Win32.RegistryValueKind]::String -and
                $Record.Kind -ne [Microsoft.Win32.RegistryValueKind]::ExpandString) {
                throw "Refusing to write HKCU\Environment\$($Record.Name) with registry value kind '$($Record.Kind)'."
            }
            $key.SetValue($Record.Name, $Record.Value, $Record.Kind)
        } else {
            $key.DeleteValue($Record.Name, $false)
        }
    } finally {
        $key.Dispose()
    }
    if ($Broadcast) {
        Publish-EnvironmentChangeBroadcast
    }
}

function Test-PersistentUserEnvironmentRecord {
    param([Parameter(Mandatory = $true)]$Expected, [Parameter(Mandatory = $true)]$Actual)

    if ($Expected.Exists -ne $Actual.Exists) {
        return $false
    }
    if (-not $Expected.Exists) {
        return $true
    }
    return $Expected.Kind -eq $Actual.Kind -and
        [string]::Equals($Expected.Value, $Actual.Value, [StringComparison]::Ordinal)
}

function Invoke-PersistentUserEnvironmentRecordCas {
    param(
        [Parameter(Mandatory = $true)]$Expected,
        [Parameter(Mandatory = $true)]$Desired,
        [Parameter(Mandatory = $true)][ref]$MutationCommitted,
        [switch]$Broadcast,
        [AllowNull()]$TestStore,
        [AllowNull()][scriptblock]$BeforeCommitTestHook,
        [AllowNull()][scriptblock]$AfterCommitBeforeVerifyTestHook
    )

    $MutationCommitted.Value = $false
    if (-not [string]::Equals([string]$Expected.Name, [string]$Desired.Name, [StringComparison]::Ordinal)) {
        throw 'Registry compare-exchange expected and desired records must name the same value.'
    }
    foreach ($record in @($Expected, $Desired)) {
        if ($record.Exists -and
            $record.Kind -ne [Microsoft.Win32.RegistryValueKind]::String -and
            $record.Kind -ne [Microsoft.Win32.RegistryValueKind]::ExpandString) {
            throw "Refusing registry compare-exchange for unsupported value kind '$($record.Kind)'."
        }
    }

    $name = [string]$Expected.Name
    $alreadyDesired = $false
    if ($null -ne $TestStore) {
        $current = Get-PersistentUserEnvironmentRecord -Name $name -TestStore $TestStore
        $alreadyDesired = Test-PersistentUserEnvironmentRecord -Expected $Desired -Actual $current
        if (-not $alreadyDesired -and
            -not (Test-PersistentUserEnvironmentRecord -Expected $Expected -Actual $current)) {
            throw "HKCU\Environment\$name changed concurrently; refusing transactional compare-exchange."
        }
        if ($null -ne $BeforeCommitTestHook) {
            & $BeforeCommitTestHook | Out-Null
        }
        $currentBeforeCommit = Get-PersistentUserEnvironmentRecord -Name $name -TestStore $TestStore
        $requiredBeforeCommit = if ($alreadyDesired) { $Desired } else { $Expected }
        if (-not (Test-PersistentUserEnvironmentRecord -Expected $requiredBeforeCommit -Actual $currentBeforeCommit)) {
            throw "HKCU\Environment\$name changed concurrently before the test transaction committed."
        }
        if (-not $alreadyDesired) {
            Set-PersistentUserEnvironmentRecord -Record $Desired -TestStore $TestStore
            $MutationCommitted.Value = $true
        }
    } else {
        Initialize-WindowsRegistryCasNative
        $transaction = [RaymanInstallerV2.WindowsRegistryTransaction]::OpenCurrentUser('Environment')
        try {
            $nativeCurrent = $transaction.ReadStringValue($name)
            $current = [pscustomobject]@{
                Name = $name
                Exists = [bool]$nativeCurrent.Exists
                Value = [string]$nativeCurrent.Value
                Kind = $nativeCurrent.Kind
            }
            $alreadyDesired = Test-PersistentUserEnvironmentRecord -Expected $Desired -Actual $current
            if (-not $alreadyDesired -and
                -not (Test-PersistentUserEnvironmentRecord -Expected $Expected -Actual $current)) {
                throw "HKCU\Environment\$name changed concurrently; refusing transactional compare-exchange."
            }
            if (-not $alreadyDesired) {
                if ($Desired.Exists) {
                    $transaction.SetStringValue($name, [string]$Desired.Value, $Desired.Kind)
                } else {
                    $transaction.DeleteValue($name)
                }
            }
            if ($null -ne $BeforeCommitTestHook) {
                & $BeforeCommitTestHook | Out-Null
            }
            try {
                $transaction.Commit()
            } catch {
                throw "HKCU\Environment\$name transactional compare-exchange did not commit; a concurrent registry operation or unavailable TxR prevented publication: $($_.Exception.Message)"
            }
            if (-not $alreadyDesired) {
                $MutationCommitted.Value = $true
            }
        } finally {
            $transaction.Dispose()
        }
    }

    if ($null -ne $AfterCommitBeforeVerifyTestHook) {
        & $AfterCommitBeforeVerifyTestHook | Out-Null
    }
    $actual = Get-PersistentUserEnvironmentRecord -Name $name -TestStore $TestStore
    if (-not (Test-PersistentUserEnvironmentRecord -Expected $Desired -Actual $actual)) {
        throw "HKCU\Environment\$name changed after transactional compare-exchange commit."
    }
    if ($Broadcast -and $MutationCommitted.Value) {
        Publish-EnvironmentChangeBroadcast
    }
    return [pscustomobject]@{
        Changed = [bool]$MutationCommitted.Value
        AlreadyDesired = $alreadyDesired
    }
}

function Restore-PersistentUserEnvironmentRecordCas {
    param(
        [Parameter(Mandatory = $true)]$Original,
        [Parameter(Mandatory = $true)]$Published,
        [switch]$Broadcast,
        [AllowNull()]$TestStore
    )

    $mutationCommitted = $false
    $null = Invoke-PersistentUserEnvironmentRecordCas `
        -Expected $Published `
        -Desired $Original `
        -MutationCommitted ([ref]$mutationCommitted) `
        -Broadcast:$Broadcast `
        -TestStore $TestStore
}

function Resolve-ManagedDirectory {
    param([string]$Path, [string]$Label)

    $fullPath = [IO.Path]::GetFullPath($Path)
    $root = [IO.Path]::GetPathRoot($fullPath)
    if ($fullPath -eq $root) {
        throw "$Label cannot be a filesystem root: $fullPath"
    }

    # Walk from the filesystem root, validating each existing ancestor before
    # creating the next component. New-Item -Force on the final path alone would
    # follow an ancestor junction/symlink and create files outside the named root.
    $separators = [char[]]@(
        [IO.Path]::DirectorySeparatorChar,
        [IO.Path]::AltDirectorySeparatorChar
    )
    $relative = $fullPath.Substring($root.Length)
    $segments = @($relative.Split($separators, [StringSplitOptions]::RemoveEmptyEntries))
    $current = $root
    foreach ($segment in $segments) {
        $current = Join-Path $current $segment
        if (-not (Test-Path -LiteralPath $current)) {
            New-Item -ItemType Directory -Path $current | Out-Null
        }
        $item = Get-Item -LiteralPath $current -Force
        if (-not $item.PSIsContainer -or
            $item.Attributes -band [IO.FileAttributes]::ReparsePoint) {
            throw "$Label ancestor must be a real directory, not a file or reparse point: $current"
        }
    }

    # Recheck the full chain and canonical equality after creation. This closes
    # ordinary symlink/junction escapes; a hostile same-user TOCTOU race remains
    # outside PowerShell's ability to make a multi-file install fully race-free.
    $current = $root
    foreach ($segment in $segments) {
        $current = Join-Path $current $segment
        $item = Get-Item -LiteralPath $current -Force
        if (-not $item.PSIsContainer -or
            $item.Attributes -band [IO.FileAttributes]::ReparsePoint) {
            throw "$Label ancestor changed into a file or reparse point: $current"
        }
    }
    $resolved = (Resolve-Path -LiteralPath $fullPath).ProviderPath
    $comparison = if ($IsWindows) {
        [StringComparison]::OrdinalIgnoreCase
    } else {
        [StringComparison]::Ordinal
    }
    if (-not $resolved.Equals($fullPath, $comparison)) {
        throw "$Label canonical path escaped the explicitly named destination: $fullPath -> $resolved"
    }
    return $resolved
}

function Resolve-ExistingRealDirectory {
    param([string]$Path, [string]$Label)

    $fullPath = [IO.Path]::GetFullPath($Path)
    $root = [IO.Path]::GetPathRoot($fullPath)
    if ($fullPath -eq $root) {
        throw "$Label cannot be a filesystem root: $fullPath"
    }
    $fullPath = $fullPath.TrimEnd(
        [IO.Path]::DirectorySeparatorChar,
        [IO.Path]::AltDirectorySeparatorChar
    )
    $separators = [char[]]@(
        [IO.Path]::DirectorySeparatorChar,
        [IO.Path]::AltDirectorySeparatorChar
    )
    $relative = $fullPath.Substring($root.Length)
    $segments = @($relative.Split($separators, [StringSplitOptions]::RemoveEmptyEntries))
    $current = $root
    foreach ($segment in $segments) {
        $current = Join-Path $current $segment
        if (-not (Test-Path -LiteralPath $current -PathType Container)) {
            throw "$Label is missing or is not a directory: $current"
        }
        $item = Get-Item -LiteralPath $current -Force
        if (-not $item.PSIsContainer -or $item.Attributes -band [IO.FileAttributes]::ReparsePoint) {
            throw "$Label ancestor must be a real directory, not a file or reparse point: $current"
        }
    }
    $resolved = (Resolve-Path -LiteralPath $fullPath).ProviderPath
    $comparison = if ($IsWindows) { [StringComparison]::OrdinalIgnoreCase } else { [StringComparison]::Ordinal }
    if (-not $resolved.Equals($fullPath, $comparison)) {
        throw "$Label canonical path escaped the explicitly named directory: $fullPath -> $resolved"
    }
    return $resolved
}

function Test-SameOrDescendantPath {
    param([string]$Candidate, [string]$Parent)

    $comparison = if ($IsWindows) {
        [StringComparison]::OrdinalIgnoreCase
    } else {
        [StringComparison]::Ordinal
    }
    if ($Candidate.Equals($Parent, $comparison)) {
        return $true
    }
    $prefix = $Parent.TrimEnd(
        [IO.Path]::DirectorySeparatorChar,
        [IO.Path]::AltDirectorySeparatorChar
    ) + [IO.Path]::DirectorySeparatorChar
    return $Candidate.StartsWith($prefix, $comparison)
}


function Assert-HookInstallationReports {
    param(
        [Parameter(Mandatory = $true)]$InstallReport,
        [Parameter(Mandatory = $true)]$StatusReport
    )

    if (-not $InstallReport.installed -or
        -not $StatusReport.installed -or
        [string]::IsNullOrWhiteSpace([string]$InstallReport.command) -or
        -not [string]::Equals(
            [string]$InstallReport.command,
            [string]$StatusReport.command,
            [StringComparison]::Ordinal
        ) -or
        -not [string]::Equals(
            [string]$InstallReport.hooks_path,
            [string]$StatusReport.hooks_path,
            $(if ($IsWindows) { [StringComparison]::OrdinalIgnoreCase } else { [StringComparison]::Ordinal })
        )) {
        throw 'Codex Stop hook status does not exactly match the completed install report.'
    }
}

function Assert-ReplaceableFile {
    param([string]$Path, [string]$Label)

    $item = Get-Item -LiteralPath $Path -Force -ErrorAction SilentlyContinue
    if ($null -eq $item) {
        return
    }
    if ($item.PSIsContainer -or $item.Attributes -band [IO.FileAttributes]::ReparsePoint) {
        throw "$Label destination must be a regular file: $Path"
    }
}

function Assert-ExpectedFileHash {
    param([string]$Path, [string]$ExpectedHash, [string]$Label)

    Assert-ReplaceableFile -Path $Path -Label $Label
    $actual = (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($actual -ne $ExpectedHash) {
        throw "$Label hash drifted after verification: $actual != $ExpectedHash ($Path)"
    }
}

function Initialize-HandleCasNative {
    if ('RaymanInstallerV2.WindowsDirectoryLease' -as [type]) {
        return
    }

    Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.IO;
using System.Runtime.InteropServices;
using System.Security.Cryptography;
using Microsoft.Win32.SafeHandles;

namespace RaymanInstallerV2
{
    public sealed class WindowsFileSnapshot
    {
        public uint VolumeSerialNumber { get; internal set; }
        public ulong FileIndex { get; internal set; }
        public ulong Size { get; internal set; }
        public uint Attributes { get; internal set; }
        public string ContentSha256 { get; internal set; }
        public string SecurityDescriptorSha256 { get; internal set; }
    }

    public sealed class WindowsDirectorySnapshot
    {
        public uint VolumeSerialNumber { get; internal set; }
        public ulong FileIndex { get; internal set; }
        public uint Attributes { get; internal set; }
    }

    internal static class WindowsNative
    {
        internal const uint GenericRead = 0x80000000;
        internal const uint GenericWrite = 0x40000000;
        internal const uint DeleteAccess = 0x00010000;
        internal const uint ReadControl = 0x00020000;
        internal const uint Synchronize = 0x00100000;
        internal const uint FileReadData = 0x00000001;
        internal const uint FileListDirectory = 0x00000001;
        internal const uint FileAddFile = 0x00000002;
        internal const uint FileAddSubdirectory = 0x00000004;
        internal const uint FileReadAttributes = 0x00000080;
        internal const uint FileWriteAttributes = 0x00000100;
        internal const uint FileShareRead = 0x00000001;
        internal const uint FileShareWrite = 0x00000002;
        internal const uint FileShareDelete = 0x00000004;
        internal const uint OpenExisting = 3;
        internal const uint FileFlagBackupSemantics = 0x02000000;
        internal const uint FileFlagOpenReparsePoint = 0x00200000;
        internal const uint FileAttributeNormal = 0x00000080;
        internal const uint FileAttributeDirectory = 0x00000010;
        internal const uint FileAttributeReparsePoint = 0x00000400;
        internal const uint FileDirectoryFile = 0x00000001;
        internal const uint FileSynchronousIoNonAlert = 0x00000020;
        internal const uint FileNonDirectoryFile = 0x00000040;
        internal const uint FileOpenReparsePoint = 0x00200000;
        internal const uint FileOpen = 1;
        internal const uint FileCreate = 2;
        internal const uint ObjCaseInsensitive = 0x00000040;
        internal const uint OwnerSecurityInformation = 0x00000001;
        internal const uint GroupSecurityInformation = 0x00000002;
        internal const uint DaclSecurityInformation = 0x00000004;
        internal const int SeFileObject = 1;
        internal const int FileRenameInfo = 3;
        internal const int FileDispositionInfo = 4;
        internal const int NativeFileRenameInformation = 10;

        [StructLayout(LayoutKind.Sequential)]
        internal struct NativeFileTime
        {
            internal uint LowDateTime;
            internal uint HighDateTime;
        }

        [StructLayout(LayoutKind.Sequential)]
        internal struct ByHandleFileInformation
        {
            internal uint FileAttributes;
            internal NativeFileTime CreationTime;
            internal NativeFileTime LastAccessTime;
            internal NativeFileTime LastWriteTime;
            internal uint VolumeSerialNumber;
            internal uint FileSizeHigh;
            internal uint FileSizeLow;
            internal uint NumberOfLinks;
            internal uint FileIndexHigh;
            internal uint FileIndexLow;
        }

        [StructLayout(LayoutKind.Sequential)]
        internal struct UnicodeString
        {
            internal ushort Length;
            internal ushort MaximumLength;
            internal IntPtr Buffer;
        }

        [StructLayout(LayoutKind.Sequential)]
        internal struct ObjectAttributes
        {
            internal int Length;
            internal IntPtr RootDirectory;
            internal IntPtr ObjectName;
            internal uint Attributes;
            internal IntPtr SecurityDescriptor;
            internal IntPtr SecurityQualityOfService;
        }

        [StructLayout(LayoutKind.Sequential)]
        internal struct IoStatusBlock
        {
            internal IntPtr Status;
            internal UIntPtr Information;
        }

        [StructLayout(LayoutKind.Sequential)]
        internal struct FileDispositionInformation
        {
            [MarshalAs(UnmanagedType.U1)]
            internal bool DeleteFile;
        }

        [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
        internal static extern SafeFileHandle CreateFileW(
            string fileName,
            uint desiredAccess,
            uint shareMode,
            IntPtr securityAttributes,
            uint creationDisposition,
            uint flagsAndAttributes,
            IntPtr templateFile);

        [DllImport("kernel32.dll", SetLastError = true)]
        [return: MarshalAs(UnmanagedType.Bool)]
        internal static extern bool GetFileInformationByHandle(
            SafeFileHandle file,
            out ByHandleFileInformation information);

        [DllImport("kernel32.dll", SetLastError = true)]
        [return: MarshalAs(UnmanagedType.Bool)]
        internal static extern bool SetFileInformationByHandle(
            SafeFileHandle file,
            int informationClass,
            IntPtr information,
            uint bufferSize);

        [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
        internal static extern uint GetFinalPathNameByHandleW(
            SafeFileHandle file,
            System.Text.StringBuilder path,
            uint pathLength,
            uint flags);

        [DllImport("ntdll.dll")]
        internal static extern int NtCreateFile(
            out SafeFileHandle fileHandle,
            uint desiredAccess,
            ref ObjectAttributes objectAttributes,
            out IoStatusBlock ioStatusBlock,
            IntPtr allocationSize,
            uint fileAttributes,
            uint shareAccess,
            uint createDisposition,
            uint createOptions,
            IntPtr eaBuffer,
            uint eaLength);

        [DllImport("ntdll.dll")]
        internal static extern uint RtlNtStatusToDosError(int status);

        [DllImport("ntdll.dll")]
        internal static extern int NtSetInformationFile(
            SafeFileHandle fileHandle,
            out IoStatusBlock ioStatusBlock,
            IntPtr fileInformation,
            uint length,
            int fileInformationClass);

        [DllImport("advapi32.dll", SetLastError = true)]
        internal static extern uint GetSecurityInfo(
            IntPtr handle,
            int objectType,
            uint securityInformation,
            out IntPtr owner,
            out IntPtr group,
            out IntPtr dacl,
            out IntPtr sacl,
            out IntPtr securityDescriptor);

        [DllImport("advapi32.dll", SetLastError = true)]
        internal static extern uint GetSecurityDescriptorLength(IntPtr securityDescriptor);

        [DllImport("kernel32.dll", SetLastError = true)]
        internal static extern IntPtr LocalFree(IntPtr memory);

        internal static void ValidateLeaf(string leaf)
        {
            if (String.IsNullOrWhiteSpace(leaf) || leaf == "." || leaf == ".." ||
                leaf.IndexOf('\0') >= 0 || leaf.IndexOf('\\') >= 0 ||
                leaf.IndexOf('/') >= 0 || leaf.IndexOf(':') >= 0 ||
                !String.Equals(Path.GetFileName(leaf), leaf, StringComparison.Ordinal))
            {
                throw new ArgumentException("Installer mutation requires one ordinary Windows leaf name: '" + leaf + "'");
            }
        }

        internal static SafeFileHandle OpenRelative(
            SafeFileHandle root,
            string leaf,
            uint desiredAccess,
            uint shareAccess,
            uint disposition,
            uint options,
            bool missingReturnsNull)
        {
            ValidateLeaf(leaf);
            IntPtr nameBuffer = Marshal.StringToHGlobalUni(leaf);
            IntPtr unicodeBuffer = IntPtr.Zero;
            SafeFileHandle result = null;
            try
            {
                UnicodeString unicode = new UnicodeString
                {
                    Length = checked((ushort)(leaf.Length * sizeof(char))),
                    MaximumLength = checked((ushort)((leaf.Length + 1) * sizeof(char))),
                    Buffer = nameBuffer
                };
                unicodeBuffer = Marshal.AllocHGlobal(Marshal.SizeOf(typeof(UnicodeString)));
                Marshal.StructureToPtr(unicode, unicodeBuffer, false);
                ObjectAttributes attributes = new ObjectAttributes
                {
                    Length = Marshal.SizeOf(typeof(ObjectAttributes)),
                    RootDirectory = root.DangerousGetHandle(),
                    ObjectName = unicodeBuffer,
                    Attributes = ObjCaseInsensitive,
                    SecurityDescriptor = IntPtr.Zero,
                    SecurityQualityOfService = IntPtr.Zero
                };
                IoStatusBlock statusBlock;
                int status = NtCreateFile(
                    out result,
                    desiredAccess,
                    ref attributes,
                    out statusBlock,
                    IntPtr.Zero,
                    FileAttributeNormal,
                    shareAccess,
                    disposition,
                    options,
                    IntPtr.Zero,
                    0);
                if (status < 0)
                {
                    if (result != null)
                    {
                        result.Dispose();
                    }
                    uint error = RtlNtStatusToDosError(status);
                    if (missingReturnsNull && (error == 2 || error == 3))
                    {
                        return null;
                    }
                    throw new Win32Exception((int)error, "Relative NtCreateFile failed for leaf '" + leaf + "'");
                }
                if (result == null || result.IsInvalid)
                {
                    if (result != null)
                    {
                        result.Dispose();
                    }
                    throw new IOException("Relative NtCreateFile returned an invalid handle for leaf '" + leaf + "'");
                }
                return result;
            }
            finally
            {
                if (unicodeBuffer != IntPtr.Zero)
                {
                    Marshal.FreeHGlobal(unicodeBuffer);
                }
                Marshal.FreeHGlobal(nameBuffer);
            }
        }

        internal static ByHandleFileInformation GetInformation(SafeFileHandle handle, string label)
        {
            ByHandleFileInformation information;
            if (!GetFileInformationByHandle(handle, out information))
            {
                throw new Win32Exception(Marshal.GetLastWin32Error(), "Unable to query handle identity for " + label);
            }
            return information;
        }

        internal static string GetFinalPath(SafeFileHandle handle, string label)
        {
            System.Text.StringBuilder value = new System.Text.StringBuilder(32768);
            uint length = GetFinalPathNameByHandleW(handle, value, (uint)value.Capacity, 0);
            if (length == 0 || length >= value.Capacity)
            {
                throw new Win32Exception(Marshal.GetLastWin32Error(), "Unable to bind directory handle path for " + label);
            }
            string path = value.ToString();
            if (path.StartsWith("\\\\?\\UNC\\", StringComparison.OrdinalIgnoreCase))
            {
                path = "\\\\" + path.Substring(8);
            }
            else if (path.StartsWith("\\\\?\\", StringComparison.OrdinalIgnoreCase))
            {
                path = path.Substring(4);
            }
            return NormalizeDirectoryPath(path);
        }

        internal static string NormalizeDirectoryPath(string path)
        {
            if (path.StartsWith("\\\\?\\UNC\\", StringComparison.OrdinalIgnoreCase))
            {
                path = "\\\\" + path.Substring(8);
            }
            else if (path.StartsWith("\\\\?\\", StringComparison.OrdinalIgnoreCase))
            {
                path = path.Substring(4);
            }
            string full = Path.GetFullPath(path);
            string root = Path.GetPathRoot(full);
            return String.Equals(full, root, StringComparison.OrdinalIgnoreCase)
                ? full
                : full.TrimEnd(Path.DirectorySeparatorChar, Path.AltDirectorySeparatorChar);
        }

        internal static void RawRenameHandle(
            SafeFileHandle source,
            SafeFileHandle targetDirectory,
            string targetLeaf,
            string label)
        {
            ValidateLeaf(targetLeaf);
            byte[] name = System.Text.Encoding.Unicode.GetBytes(targetLeaf);
            int rootOffset = IntPtr.Size == 8 ? 8 : 4;
            int lengthOffset = rootOffset + IntPtr.Size;
            int nameOffset = lengthOffset + sizeof(uint);
            IntPtr buffer = Marshal.AllocHGlobal(nameOffset + name.Length);
            try
            {
                for (int index = 0; index < nameOffset + name.Length; index++)
                {
                    Marshal.WriteByte(buffer, index, 0);
                }
                Marshal.WriteByte(buffer, 0, 0);
                Marshal.WriteIntPtr(buffer, rootOffset, targetDirectory.DangerousGetHandle());
                Marshal.WriteInt32(buffer, lengthOffset, name.Length);
                Marshal.Copy(name, 0, IntPtr.Add(buffer, nameOffset), name.Length);
                IoStatusBlock ioStatusBlock;
                int status = NtSetInformationFile(
                    source,
                    out ioStatusBlock,
                    buffer,
                    (uint)(nameOffset + name.Length),
                    NativeFileRenameInformation);
                if (status < 0)
                {
                    uint error = RtlNtStatusToDosError(status);
                    throw new Win32Exception((int)error, "Relative no-replace rename failed for " + label + " to leaf '" + targetLeaf + "'; win32_error=" + error + "; ntstatus=0x" + unchecked((uint)status).ToString("x8"));
                }
            }
            finally
            {
                Marshal.FreeHGlobal(buffer);
            }
        }

        internal static void RawMarkDelete(SafeFileHandle handle, string label)
        {
            int size = Marshal.SizeOf(typeof(FileDispositionInformation));
            if (size != 1)
            {
                throw new InvalidOperationException("FILE_DISPOSITION_INFO must marshal to one byte; observed " + size);
            }
            FileDispositionInformation disposition = new FileDispositionInformation { DeleteFile = true };
            IntPtr buffer = Marshal.AllocHGlobal(size);
            try
            {
                Marshal.StructureToPtr(disposition, buffer, false);
                if (!SetFileInformationByHandle(handle, FileDispositionInfo, buffer, (uint)size))
                {
                    throw new Win32Exception(Marshal.GetLastWin32Error(), "Identity-bound delete disposition failed for " + label);
                }
            }
            finally
            {
                Marshal.FreeHGlobal(buffer);
            }
        }
    }

    public sealed class WindowsDirectoryLease : IDisposable
    {
        private SafeFileHandle handle;
        private string boundPath;
        private WindowsDirectorySnapshot identity;

        private WindowsDirectoryLease(SafeFileHandle handle, string path)
        {
            this.handle = handle;
            this.boundPath = WindowsNative.NormalizeDirectoryPath(path);
            this.identity = null;
        }

        internal SafeFileHandle Handle { get { EnsureOpen(); return handle; } }
        public string BoundPath { get { return boundPath; } }

        public static WindowsDirectoryLease Open(string path)
        {
            string fullPath = WindowsNative.NormalizeDirectoryPath(path);
            SafeFileHandle handle = WindowsNative.CreateFileW(
                fullPath,
                WindowsNative.FileListDirectory | WindowsNative.FileAddFile |
                    WindowsNative.FileAddSubdirectory | WindowsNative.FileReadAttributes |
                    WindowsNative.ReadControl | WindowsNative.Synchronize,
                WindowsNative.FileShareRead | WindowsNative.FileShareWrite,
                IntPtr.Zero,
                WindowsNative.OpenExisting,
                WindowsNative.FileFlagBackupSemantics | WindowsNative.FileFlagOpenReparsePoint,
                IntPtr.Zero);
            if (handle.IsInvalid)
            {
                int error = Marshal.GetLastWin32Error();
                handle.Dispose();
                throw new Win32Exception(error, "Unable to lease terminal directory '" + fullPath + "'");
            }
            WindowsDirectoryLease lease = new WindowsDirectoryLease(handle, fullPath);
            try
            {
                lease.CaptureInitialIdentity();
                lease.AssertPathBinding();
                return lease;
            }
            catch
            {
                lease.Dispose();
                throw;
            }
        }

        public WindowsFileLease RawCreateFileExclusive(string leaf)
        {
            EnsureOpen();
            SafeFileHandle file = WindowsNative.OpenRelative(
                handle,
                leaf,
                WindowsNative.GenericRead | WindowsNative.GenericWrite |
                    WindowsNative.ReadControl | WindowsNative.DeleteAccess | WindowsNative.Synchronize,
                WindowsNative.FileShareRead,
                WindowsNative.FileCreate,
                WindowsNative.FileNonDirectoryFile | WindowsNative.FileSynchronousIoNonAlert |
                    WindowsNative.FileOpenReparsePoint,
                false);
            return new WindowsFileLease(file, boundPath, leaf, true);
        }

        public WindowsDirectoryLease RawCreateDirectoryExclusive(string leaf)
        {
            EnsureOpen();
            WindowsNative.ValidateLeaf(leaf);
            SafeFileHandle directory = WindowsNative.OpenRelative(
                handle,
                leaf,
                WindowsNative.FileListDirectory | WindowsNative.FileAddFile |
                    WindowsNative.FileAddSubdirectory | WindowsNative.FileReadAttributes |
                    WindowsNative.ReadControl | WindowsNative.DeleteAccess | WindowsNative.Synchronize,
                WindowsNative.FileShareRead | WindowsNative.FileShareWrite |
                    WindowsNative.FileShareDelete,
                WindowsNative.FileCreate,
                WindowsNative.FileDirectoryFile | WindowsNative.FileSynchronousIoNonAlert |
                    WindowsNative.FileOpenReparsePoint,
                false);
            return new WindowsDirectoryLease(directory, Path.Combine(boundPath, leaf));
        }

        public WindowsFileLease TryOpenFile(string leaf, bool deleteAccess)
        {
            EnsureOpen();
            uint access = WindowsNative.GenericRead | WindowsNative.ReadControl | WindowsNative.Synchronize;
            if (deleteAccess)
            {
                access |= WindowsNative.DeleteAccess;
            }
            SafeFileHandle file = WindowsNative.OpenRelative(
                handle,
                leaf,
                access,
                WindowsNative.FileShareRead,
                WindowsNative.FileOpen,
                WindowsNative.FileNonDirectoryFile | WindowsNative.FileSynchronousIoNonAlert |
                    WindowsNative.FileOpenReparsePoint,
                true);
            return file == null ? null : new WindowsFileLease(file, boundPath, leaf, deleteAccess);
        }

        public WindowsDirectoryLease TryOpenDirectory(string leaf, bool deleteAccess)
        {
            EnsureOpen();
            uint access = WindowsNative.FileListDirectory | WindowsNative.FileReadAttributes |
                WindowsNative.ReadControl | WindowsNative.Synchronize;
            if (deleteAccess)
            {
                access |= WindowsNative.DeleteAccess;
            }
            SafeFileHandle directory = WindowsNative.OpenRelative(
                handle,
                leaf,
                access,
                WindowsNative.FileShareRead | WindowsNative.FileShareWrite |
                    WindowsNative.FileShareDelete,
                WindowsNative.FileOpen,
                WindowsNative.FileDirectoryFile | WindowsNative.FileSynchronousIoNonAlert |
                    WindowsNative.FileOpenReparsePoint,
                true);
            if (directory == null) return null;
            WindowsDirectoryLease lease = new WindowsDirectoryLease(directory, Path.Combine(boundPath, leaf));
            try
            {
                lease.CaptureInitialIdentity();
                lease.AssertPathBinding();
                return lease;
            }
            catch
            {
                lease.Dispose();
                throw;
            }
        }

        public void RawRenameNoReplaceTo(WindowsDirectoryLease target, string targetLeaf)
        {
            EnsureOpen();
            WindowsNative.RawRenameHandle(handle, target.Handle, targetLeaf, "directory '" + boundPath + "'");
        }

        public void AcceptRenamedPath(string newPath)
        {
            EnsureOpen();
            boundPath = WindowsNative.NormalizeDirectoryPath(newPath);
        }

        public WindowsDirectorySnapshot CaptureIdentity()
        {
            if (identity == null)
            {
                throw new InvalidOperationException("Directory identity baseline has not been captured for '" + boundPath + "'");
            }
            WindowsDirectorySnapshot current = CaptureIdentityInternal();
            if (current.VolumeSerialNumber != identity.VolumeSerialNumber ||
                current.FileIndex != identity.FileIndex || current.Attributes != identity.Attributes)
            {
                throw new IOException("Terminal directory identity changed for '" + boundPath + "'");
            }
            return current;
        }

        public string GetCurrentPath()
        {
            EnsureOpen();
            return WindowsNative.GetFinalPath(handle, boundPath);
        }

        public void AssertPathBinding()
        {
            CaptureIdentity();
            string current = GetCurrentPath();
            if (!String.Equals(current, boundPath, StringComparison.OrdinalIgnoreCase))
            {
                throw new IOException("Terminal directory path binding changed: expected '" + boundPath + "', observed '" + current + "'");
            }
        }

        public static void ValidateLeaf(string leaf)
        {
            WindowsNative.ValidateLeaf(leaf);
        }

        public WindowsDirectorySnapshot CaptureInitialIdentity()
        {
            EnsureOpen();
            if (identity != null)
            {
                throw new InvalidOperationException("Directory identity baseline was already captured for '" + boundPath + "'");
            }
            identity = CaptureIdentityInternal();
            return identity;
        }

        private WindowsDirectorySnapshot CaptureIdentityInternal()
        {
            WindowsNative.ByHandleFileInformation information = WindowsNative.GetInformation(handle, boundPath);
            if ((information.FileAttributes & WindowsNative.FileAttributeDirectory) == 0 ||
                (information.FileAttributes & WindowsNative.FileAttributeReparsePoint) != 0)
            {
                throw new IOException("Directory lease target must be a real non-reparse directory: " + boundPath);
            }
            return new WindowsDirectorySnapshot
            {
                VolumeSerialNumber = information.VolumeSerialNumber,
                FileIndex = ((ulong)information.FileIndexHigh << 32) | information.FileIndexLow,
                Attributes = information.FileAttributes
            };
        }

        private void EnsureOpen()
        {
            if (handle == null || handle.IsClosed || handle.IsInvalid)
            {
                throw new ObjectDisposedException("WindowsDirectoryLease");
            }
        }

        public void Dispose()
        {
            if (handle != null)
            {
                handle.Dispose();
                handle = null;
            }
        }
    }

    public sealed class WindowsFileLease : IDisposable
    {
        private SafeFileHandle handle;
        private readonly string parentPath;
        private readonly string leaf;
        private readonly bool deleteAccess;

        internal WindowsFileLease(SafeFileHandle handle, string parentPath, string leaf, bool deleteAccess)
        {
            this.handle = handle;
            this.parentPath = parentPath;
            this.leaf = leaf;
            this.deleteAccess = deleteAccess;
        }

        public static WindowsFileLease OpenSource(string path)
        {
            string fullPath = Path.GetFullPath(path);
            SafeFileHandle handle = WindowsNative.CreateFileW(
                fullPath,
                WindowsNative.GenericRead | WindowsNative.ReadControl | WindowsNative.Synchronize,
                WindowsNative.FileShareRead,
                IntPtr.Zero,
                WindowsNative.OpenExisting,
                WindowsNative.FileFlagOpenReparsePoint,
                IntPtr.Zero);
            if (handle.IsInvalid)
            {
                int error = Marshal.GetLastWin32Error();
                handle.Dispose();
                throw new Win32Exception(error, "Unable to identity-open source file '" + fullPath + "'");
            }
            return new WindowsFileLease(handle, Path.GetDirectoryName(fullPath), Path.GetFileName(fullPath), false);
        }

        public WindowsFileSnapshot CaptureSnapshot()
        {
            WindowsNative.ByHandleFileInformation before = GetInformation();
            string securityBefore = HashSecurityDescriptor();
            string content = HashContent();
            WindowsNative.ByHandleFileInformation after = GetInformation();
            string securityAfter = HashSecurityDescriptor();
            ulong beforeIndex = ((ulong)before.FileIndexHigh << 32) | before.FileIndexLow;
            ulong afterIndex = ((ulong)after.FileIndexHigh << 32) | after.FileIndexLow;
            ulong beforeSize = ((ulong)before.FileSizeHigh << 32) | before.FileSizeLow;
            ulong afterSize = ((ulong)after.FileSizeHigh << 32) | after.FileSizeLow;
            if (before.VolumeSerialNumber != after.VolumeSerialNumber ||
                beforeIndex != afterIndex || beforeSize != afterSize ||
                before.FileAttributes != after.FileAttributes ||
                !String.Equals(securityBefore, securityAfter, StringComparison.Ordinal))
            {
                throw new IOException("File identity or security metadata changed while open: " + Description);
            }
            return new WindowsFileSnapshot
            {
                VolumeSerialNumber = before.VolumeSerialNumber,
                FileIndex = beforeIndex,
                Size = beforeSize,
                Attributes = before.FileAttributes,
                ContentSha256 = content,
                SecurityDescriptorSha256 = securityBefore
            };
        }

        public void CopyFrom(WindowsFileLease source)
        {
            EnsureOpen();
            source.EnsureOpen();
            bool sourceReference = false;
            bool targetReference = false;
            source.handle.DangerousAddRef(ref sourceReference);
            handle.DangerousAddRef(ref targetReference);
            try
            {
                using (SafeFileHandle borrowedSource = new SafeFileHandle(source.handle.DangerousGetHandle(), false))
                using (SafeFileHandle borrowedTarget = new SafeFileHandle(handle.DangerousGetHandle(), false))
                using (FileStream input = new FileStream(borrowedSource, FileAccess.Read, 65536, false))
                using (FileStream output = new FileStream(borrowedTarget, FileAccess.ReadWrite, 65536, false))
                {
                    input.Seek(0, SeekOrigin.Begin);
                    output.Seek(0, SeekOrigin.Begin);
                    output.SetLength(0);
                    input.CopyTo(output, 65536);
                    output.Flush(true);
                }
            }
            finally
            {
                if (targetReference) handle.DangerousRelease();
                if (sourceReference) source.handle.DangerousRelease();
            }
        }

        public void WriteAllBytes(byte[] bytes)
        {
            EnsureOpen();
            bool targetReference = false;
            handle.DangerousAddRef(ref targetReference);
            try
            {
                using (SafeFileHandle borrowedTarget = new SafeFileHandle(handle.DangerousGetHandle(), false))
                using (FileStream output = new FileStream(borrowedTarget, FileAccess.ReadWrite, 65536, false))
                {
                    output.Seek(0, SeekOrigin.Begin);
                    output.SetLength(0);
                    output.Write(bytes, 0, bytes.Length);
                    output.Flush(true);
                }
            }
            finally
            {
                if (targetReference) handle.DangerousRelease();
            }
        }

        public void RawRenameNoReplaceTo(WindowsDirectoryLease target, string targetLeaf)
        {
            EnsureOpen();
            if (!deleteAccess)
            {
                throw new InvalidOperationException("Leaf lease lacks rename access: " + Description);
            }
            WindowsNative.RawRenameHandle(handle, target.Handle, targetLeaf, Description);
        }

        public void RawMarkDelete()
        {
            EnsureOpen();
            if (!deleteAccess)
            {
                throw new InvalidOperationException("Leaf lease lacks delete access: " + Description);
            }
            WindowsNative.RawMarkDelete(handle, Description);
        }

        public static int FileDispositionInfoSize
        {
            get { return Marshal.SizeOf(typeof(WindowsNative.FileDispositionInformation)); }
        }

        private WindowsNative.ByHandleFileInformation GetInformation()
        {
            EnsureOpen();
            WindowsNative.ByHandleFileInformation information = WindowsNative.GetInformation(handle, Description);
            if ((information.FileAttributes & (WindowsNative.FileAttributeDirectory |
                WindowsNative.FileAttributeReparsePoint)) != 0)
            {
                throw new IOException("Leaf CAS target must be a regular non-reparse file: " + Description);
            }
            return information;
        }

        private string HashContent()
        {
            bool reference = false;
            handle.DangerousAddRef(ref reference);
            try
            {
                using (SafeFileHandle borrowed = new SafeFileHandle(handle.DangerousGetHandle(), false))
                using (FileStream stream = new FileStream(borrowed, FileAccess.Read, 65536, false))
                using (SHA256 sha = SHA256.Create())
                {
                    stream.Seek(0, SeekOrigin.Begin);
                    return ToHex(sha.ComputeHash(stream));
                }
            }
            finally
            {
                if (reference) handle.DangerousRelease();
            }
        }

        private string HashSecurityDescriptor()
        {
            IntPtr owner;
            IntPtr group;
            IntPtr dacl;
            IntPtr sacl;
            IntPtr descriptor;
            uint result = WindowsNative.GetSecurityInfo(
                handle.DangerousGetHandle(),
                WindowsNative.SeFileObject,
                WindowsNative.OwnerSecurityInformation | WindowsNative.GroupSecurityInformation |
                    WindowsNative.DaclSecurityInformation,
                out owner,
                out group,
                out dacl,
                out sacl,
                out descriptor);
            if (result != 0)
            {
                throw new Win32Exception((int)result, "Unable to query owner/group/DACL for " + Description);
            }
            try
            {
                uint length = WindowsNative.GetSecurityDescriptorLength(descriptor);
                if (length == 0)
                {
                    throw new Win32Exception(Marshal.GetLastWin32Error(), "Unable to measure security descriptor for " + Description);
                }
                byte[] bytes = new byte[length];
                Marshal.Copy(descriptor, bytes, 0, bytes.Length);
                using (SHA256 sha = SHA256.Create())
                {
                    return ToHex(sha.ComputeHash(bytes));
                }
            }
            finally
            {
                if (descriptor != IntPtr.Zero) WindowsNative.LocalFree(descriptor);
            }
        }

        private string Description
        {
            get { return Path.Combine(parentPath, leaf); }
        }

        private void EnsureOpen()
        {
            if (handle == null || handle.IsClosed || handle.IsInvalid)
            {
                throw new ObjectDisposedException("WindowsFileLease");
            }
        }

        private static string ToHex(byte[] bytes)
        {
            return BitConverter.ToString(bytes).Replace("-", "").ToLowerInvariant();
        }

        public void Dispose()
        {
            if (handle != null)
            {
                handle.Dispose();
                handle = null;
            }
        }
    }
}
'@ | Out-Null
}

function Initialize-LinuxHandleCasNative {
    if ('RaymanInstallerV2.LinuxDirectoryLease' -as [type]) {
        return
    }

    Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.IO;
using System.Runtime.InteropServices;
using System.Security.Cryptography;
using Microsoft.Win32.SafeHandles;

namespace RaymanInstallerV2
{
    public sealed class LinuxFileSnapshot
    {
        public ulong DeviceId { get; internal set; }
        public ulong Inode { get; internal set; }
        public uint Mode { get; internal set; }
        public uint UserId { get; internal set; }
        public uint GroupId { get; internal set; }
        public long Size { get; internal set; }
        public string ContentSha256 { get; internal set; }
        public string ExtendedMetadataSha256 { get; internal set; }
    }

    public sealed class LinuxDirectorySnapshot
    {
        public ulong DeviceId { get; internal set; }
        public ulong Inode { get; internal set; }
        public uint Mode { get; internal set; }
    }

    internal sealed class LinuxMetadata
    {
        internal ulong DeviceId;
        internal ulong Inode;
        internal uint Mode;
        internal uint UserId;
        internal uint GroupId;
        internal long Size;
    }

    internal static class LinuxNative
    {
        internal const int AtFdcwd = -100;
        internal const int AtEmptyPath = 0x1000;
        internal const int AtSymlinkFollow = 0x400;
        internal const int OpenReadOnly = 0;
        internal const int OpenWriteOnly = 1;
        internal const int OpenReadWrite = 2;
        internal const int OpenCreate = 0x40;
        internal const int OpenExclusive = 0x80;
        internal const int OpenNonBlocking = 0x800;
        internal const int OpenDirectory = 0x10000;
        internal const int OpenNoFollow = 0x20000;
        internal const int OpenCloseOnExec = 0x80000;
        internal const uint RenameNoReplace = 1;
        internal const uint StatxType = 0x0001;
        internal const uint StatxMode = 0x0002;
        internal const uint StatxUid = 0x0008;
        internal const uint StatxGid = 0x0010;
        internal const uint StatxIno = 0x0100;
        internal const uint StatxSize = 0x0200;
        internal const uint RequiredStatxMask = StatxType | StatxMode | StatxUid |
            StatxGid | StatxIno | StatxSize;
        internal const uint FileTypeMask = 0xF000;
        internal const uint RegularFile = 0x8000;
        internal const uint DirectoryFile = 0x4000;
        internal const int StatxBufferSize = 256;

        [DllImport("libc", CharSet = CharSet.Ansi, SetLastError = true, EntryPoint = "open")]
        internal static extern int OpenNative(string path, int flags);

        [DllImport("libc", CharSet = CharSet.Ansi, SetLastError = true, EntryPoint = "openat")]
        internal static extern int OpenAtNative(int directory, string path, int flags, uint mode);

        [DllImport("libc", CharSet = CharSet.Ansi, SetLastError = true, EntryPoint = "mkdirat")]
        internal static extern int MkdirAtNative(int directory, string path, uint mode);

        [DllImport("libc", CharSet = CharSet.Ansi, SetLastError = true, EntryPoint = "statx")]
        internal static extern int StatxNative(
            int directory,
            string path,
            int flags,
            uint mask,
            IntPtr status);

        [DllImport("libc", SetLastError = true, EntryPoint = "flistxattr")]
        internal static extern long FListXAttrNative(int file, IntPtr list, UIntPtr size);

        [DllImport("libc", SetLastError = true, EntryPoint = "fgetxattr")]
        internal static extern long FGetXAttrNative(
            int file,
            IntPtr name,
            IntPtr value,
            UIntPtr size);

        [DllImport("libc", SetLastError = true, EntryPoint = "fsetxattr")]
        internal static extern int FSetXAttrNative(
            int file,
            IntPtr name,
            IntPtr value,
            UIntPtr size,
            int flags);

        [DllImport("libc", CharSet = CharSet.Ansi, SetLastError = true, EntryPoint = "linkat")]
        internal static extern int LinkAtNative(
            int oldDirectory,
            string oldPath,
            int newDirectory,
            string newPath,
            int flags);

        [DllImport("libc", CharSet = CharSet.Ansi, SetLastError = true, EntryPoint = "renameat2")]
        internal static extern int RenameAt2Native(
            int oldDirectory,
            string oldPath,
            int newDirectory,
            string newPath,
            uint flags);

        [DllImport("libc", CharSet = CharSet.Ansi, SetLastError = true, EntryPoint = "readlink")]
        internal static extern long ReadLinkNative(string path, byte[] buffer, ulong size);

        [DllImport("libc", SetLastError = true, EntryPoint = "fchmod")]
        internal static extern int FChmodNative(int file, uint mode);

        internal static void AssertRuntime()
        {
            if (!RuntimeInformation.IsOSPlatform(OSPlatform.Linux) ||
                (RuntimeInformation.ProcessArchitecture != Architecture.X64 &&
                 RuntimeInformation.ProcessArchitecture != Architecture.Arm64))
            {
                throw new PlatformNotSupportedException(
                    "Linux handle CAS requires a native x64 or ARM64 process.");
            }
        }

        internal static void ValidateLeaf(string leaf)
        {
            if (String.IsNullOrWhiteSpace(leaf) || leaf == "." || leaf == ".." ||
                leaf.IndexOf('\0') >= 0 || leaf.IndexOf('/') >= 0 ||
                !String.Equals(Path.GetFileName(leaf), leaf, StringComparison.Ordinal))
            {
                throw new ArgumentException("Installer mutation requires one ordinary Linux leaf name: '" + leaf + "'");
            }
        }

        internal static LinuxMetadata ReadMetadata(SafeFileHandle handle, string label)
        {
            IntPtr status = Marshal.AllocHGlobal(StatxBufferSize);
            try
            {
                byte[] zero = new byte[StatxBufferSize];
                Marshal.Copy(zero, 0, status, zero.Length);
                if (StatxNative(
                        handle.DangerousGetHandle().ToInt32(),
                        "",
                        AtEmptyPath,
                        RequiredStatxMask,
                        status) != 0)
                {
                    throw LastError("statx(AT_EMPTY_PATH) failed for " + label);
                }
                uint returnedMask = unchecked((uint)Marshal.ReadInt32(status, 0));
                ValidateStatxMask(returnedMask);
                uint deviceMajor = unchecked((uint)Marshal.ReadInt32(status, 136));
                uint deviceMinor = unchecked((uint)Marshal.ReadInt32(status, 140));
                return new LinuxMetadata
                {
                    DeviceId = ((ulong)deviceMajor << 32) | deviceMinor,
                    Inode = unchecked((ulong)Marshal.ReadInt64(status, 32)),
                    Mode = unchecked((ushort)Marshal.ReadInt16(status, 28)),
                    UserId = unchecked((uint)Marshal.ReadInt32(status, 20)),
                    GroupId = unchecked((uint)Marshal.ReadInt32(status, 24)),
                    Size = Marshal.ReadInt64(status, 40)
                };
            }
            finally
            {
                Marshal.FreeHGlobal(status);
            }
        }

        internal static void ValidateStatxMask(uint mask)
        {
            if ((mask & RequiredStatxMask) != RequiredStatxMask)
            {
                throw new IOException(
                    "statx returned an incomplete mask: required=0x" +
                    RequiredStatxMask.ToString("x") + ", observed=0x" + mask.ToString("x"));
            }
        }

        internal static string CurrentDescriptorPath(SafeFileHandle handle, string label)
        {
            string descriptorPath = "/proc/self/fd/" + handle.DangerousGetHandle().ToInt32();
            byte[] buffer = new byte[32768];
            long length = ReadLinkNative(descriptorPath, buffer, (ulong)buffer.Length);
            if (length < 0)
            {
                throw LastError("Unable to resolve held directory through " + descriptorPath + " for " + label);
            }
            if (length >= buffer.Length)
            {
                throw new IOException("Held-directory path exceeded the readlink buffer for " + label);
            }
            return NormalizeDirectoryPath(System.Text.Encoding.UTF8.GetString(buffer, 0, checked((int)length)));
        }

        internal static string NormalizeDirectoryPath(string path)
        {
            string full = Path.GetFullPath(path);
            string root = Path.GetPathRoot(full);
            return String.Equals(full, root, StringComparison.Ordinal)
                ? full
                : full.TrimEnd(Path.DirectorySeparatorChar, Path.AltDirectorySeparatorChar);
        }

        internal static Exception LastError(string message)
        {
            return new Win32Exception(Marshal.GetLastWin32Error(), message);
        }
    }

    public sealed class LinuxDirectoryLease : IDisposable
    {
        private SafeFileHandle handle;
        private string boundPath;
        private LinuxDirectorySnapshot identity;

        private LinuxDirectoryLease(SafeFileHandle handle, string path)
        {
            this.handle = handle;
            this.boundPath = LinuxNative.NormalizeDirectoryPath(path);
            this.identity = null;
        }

        internal SafeFileHandle Handle { get { EnsureOpen(); return handle; } }
        internal int Descriptor { get { EnsureOpen(); return handle.DangerousGetHandle().ToInt32(); } }
        public string BoundPath { get { return boundPath; } }

        public static LinuxDirectoryLease Open(string path)
        {
            LinuxNative.AssertRuntime();
            string fullPath = LinuxNative.NormalizeDirectoryPath(path);
            int descriptor = LinuxNative.OpenNative(
                fullPath,
                LinuxNative.OpenReadOnly | LinuxNative.OpenDirectory |
                    LinuxNative.OpenNoFollow | LinuxNative.OpenCloseOnExec);
            if (descriptor < 0)
            {
                throw LinuxNative.LastError("Unable to lease terminal directory '" + fullPath + "'");
            }
            LinuxDirectoryLease lease = new LinuxDirectoryLease(
                new SafeFileHandle(new IntPtr(descriptor), true),
                fullPath);
            try
            {
                lease.CaptureInitialIdentity();
                lease.AssertPathBinding();
                return lease;
            }
            catch
            {
                lease.Dispose();
                throw;
            }
        }

        public LinuxFileLease RawCreateFileExclusive(string leaf)
        {
            EnsureOpen();
            LinuxNative.ValidateLeaf(leaf);
            int descriptor = LinuxNative.OpenAtNative(
                Descriptor,
                leaf,
                LinuxNative.OpenReadWrite | LinuxNative.OpenCreate | LinuxNative.OpenExclusive |
                    LinuxNative.OpenNoFollow | LinuxNative.OpenCloseOnExec | LinuxNative.OpenNonBlocking,
                448);
            if (descriptor < 0)
            {
                throw LinuxNative.LastError("Exclusive openat stage creation failed for leaf '" + leaf + "'");
            }
            return new LinuxFileLease(
                new SafeFileHandle(new IntPtr(descriptor), true),
                boundPath,
                leaf);
        }

        public void RawCreateDirectoryExclusive(string leaf)
        {
            EnsureOpen();
            LinuxNative.ValidateLeaf(leaf);
            if (LinuxNative.MkdirAtNative(Descriptor, leaf, 448) != 0)
            {
                throw LinuxNative.LastError("Exclusive mkdirat failed for leaf '" + leaf + "'");
            }
        }

        public LinuxDirectoryLease TryOpenDirectory(string leaf)
        {
            EnsureOpen();
            LinuxNative.ValidateLeaf(leaf);
            int descriptor = LinuxNative.OpenAtNative(
                Descriptor,
                leaf,
                LinuxNative.OpenReadOnly | LinuxNative.OpenDirectory |
                    LinuxNative.OpenNoFollow | LinuxNative.OpenCloseOnExec,
                0);
            if (descriptor < 0)
            {
                int error = Marshal.GetLastWin32Error();
                if (error == 2) return null;
                throw new Win32Exception(error, "Relative directory openat failed for leaf '" + leaf + "'");
            }
            LinuxDirectoryLease lease = new LinuxDirectoryLease(
                new SafeFileHandle(new IntPtr(descriptor), true),
                Path.Combine(boundPath, leaf));
            try
            {
                lease.CaptureInitialIdentity();
                lease.AssertPathBinding();
                return lease;
            }
            catch
            {
                lease.Dispose();
                throw;
            }
        }

        public LinuxFileLease TryOpenFile(string leaf)
        {
            EnsureOpen();
            LinuxNative.ValidateLeaf(leaf);
            int descriptor = LinuxNative.OpenAtNative(
                Descriptor,
                leaf,
                LinuxNative.OpenReadOnly | LinuxNative.OpenNoFollow |
                    LinuxNative.OpenCloseOnExec | LinuxNative.OpenNonBlocking,
                0);
            if (descriptor < 0)
            {
                int error = Marshal.GetLastWin32Error();
                if (error == 2) return null;
                throw new Win32Exception(error, "Relative file openat failed for leaf '" + leaf + "'");
            }
            return new LinuxFileLease(
                new SafeFileHandle(new IntPtr(descriptor), true),
                boundPath,
                leaf);
        }

        public void RawLinkOpenFileNoReplace(LinuxFileLease source, string targetLeaf)
        {
            EnsureOpen();
            LinuxNative.ValidateLeaf(targetLeaf);
            string descriptorPath = "/proc/self/fd/" + source.Descriptor;
            if (LinuxNative.LinkAtNative(
                    LinuxNative.AtFdcwd,
                    descriptorPath,
                    Descriptor,
                    targetLeaf,
                    LinuxNative.AtSymlinkFollow) != 0)
            {
                throw LinuxNative.LastError(
                    "Identity-bound hardlink preflight/publication failed from '" +
                    descriptorPath + "' to leaf '" + targetLeaf + "'");
            }
        }

        public void RawRenameNoReplace(string sourceLeaf, LinuxDirectoryLease target, string targetLeaf)
        {
            EnsureOpen();
            LinuxNative.ValidateLeaf(sourceLeaf);
            LinuxNative.ValidateLeaf(targetLeaf);
            if (LinuxNative.RenameAt2Native(
                    Descriptor,
                    sourceLeaf,
                    target.Descriptor,
                    targetLeaf,
                    LinuxNative.RenameNoReplace) != 0)
            {
                throw LinuxNative.LastError(
                    "Relative renameat2(RENAME_NOREPLACE) failed from leaf '" +
                    sourceLeaf + "' to leaf '" + targetLeaf + "'");
            }
        }

        public LinuxDirectorySnapshot CaptureIdentity()
        {
            if (identity == null)
            {
                throw new InvalidOperationException("Directory identity baseline has not been captured for '" + boundPath + "'");
            }
            LinuxDirectorySnapshot current = CaptureIdentityInternal();
            if (current.DeviceId != identity.DeviceId || current.Inode != identity.Inode ||
                current.Mode != identity.Mode)
            {
                throw new IOException("Terminal directory identity changed for '" + boundPath + "'");
            }
            return current;
        }

        public string GetCurrentPath()
        {
            EnsureOpen();
            return LinuxNative.CurrentDescriptorPath(handle, boundPath);
        }

        public void AssertPathBinding()
        {
            CaptureIdentity();
            string current = GetCurrentPath();
            if (!String.Equals(current, boundPath, StringComparison.Ordinal))
            {
                throw new IOException("Terminal directory path binding changed: expected '" + boundPath + "', observed '" + current + "'");
            }
        }

        public void AcceptRenamedPath(string newPath)
        {
            EnsureOpen();
            boundPath = LinuxNative.NormalizeDirectoryPath(newPath);
        }

        public static void AssertRuntimeSupported()
        {
            LinuxNative.AssertRuntime();
        }

        public static void ValidateLeaf(string leaf)
        {
            LinuxNative.ValidateLeaf(leaf);
        }

        public LinuxDirectorySnapshot CaptureInitialIdentity()
        {
            EnsureOpen();
            if (identity != null)
            {
                throw new InvalidOperationException("Directory identity baseline was already captured for '" + boundPath + "'");
            }
            identity = CaptureIdentityInternal();
            return identity;
        }

        public static uint RequiredStatxMaskForSelfTest
        {
            get { return LinuxNative.RequiredStatxMask; }
        }

        public static int StatxBufferSizeForSelfTest
        {
            get { return LinuxNative.StatxBufferSize; }
        }

        public static void ValidateStatxMaskForSelfTest(uint mask)
        {
            LinuxNative.ValidateStatxMask(mask);
        }

        private LinuxDirectorySnapshot CaptureIdentityInternal()
        {
            LinuxMetadata metadata = LinuxNative.ReadMetadata(handle, boundPath);
            if ((metadata.Mode & LinuxNative.FileTypeMask) != LinuxNative.DirectoryFile)
            {
                throw new IOException("Directory lease target must be a real non-symlink directory: " + boundPath);
            }
            return new LinuxDirectorySnapshot
            {
                DeviceId = metadata.DeviceId,
                Inode = metadata.Inode,
                Mode = metadata.Mode
            };
        }

        private void EnsureOpen()
        {
            if (handle == null || handle.IsClosed || handle.IsInvalid)
            {
                throw new ObjectDisposedException("LinuxDirectoryLease");
            }
        }

        public void Dispose()
        {
            if (handle != null)
            {
                handle.Dispose();
                handle = null;
            }
        }
    }

    public sealed class LinuxFileLease : IDisposable
    {
        private SafeFileHandle handle;
        private readonly string parentPath;
        private readonly string leaf;

        internal LinuxFileLease(SafeFileHandle handle, string parentPath, string leaf)
        {
            this.handle = handle;
            this.parentPath = parentPath;
            this.leaf = leaf;
        }

        internal int Descriptor
        {
            get { EnsureOpen(); return handle.DangerousGetHandle().ToInt32(); }
        }

        public static LinuxFileLease OpenSource(string path)
        {
            LinuxNative.AssertRuntime();
            string fullPath = Path.GetFullPath(path);
            int descriptor = LinuxNative.OpenNative(
                fullPath,
                LinuxNative.OpenReadOnly | LinuxNative.OpenNoFollow |
                    LinuxNative.OpenCloseOnExec | LinuxNative.OpenNonBlocking);
            if (descriptor < 0)
            {
                throw LinuxNative.LastError("Unable to identity-open source file '" + fullPath + "'");
            }
            return new LinuxFileLease(
                new SafeFileHandle(new IntPtr(descriptor), true),
                Path.GetDirectoryName(fullPath),
                Path.GetFileName(fullPath));
        }

        public LinuxFileSnapshot CaptureSnapshot()
        {
            LinuxMetadata before = ReadRegularMetadata();
            string extendedBefore = HashExtendedMetadata();
            string content = HashContent();
            LinuxMetadata after = ReadRegularMetadata();
            string extendedAfter = HashExtendedMetadata();
            LinuxFileSnapshot first = ToSnapshot(before, content, extendedBefore);
            LinuxFileSnapshot second = ToSnapshot(after, content, extendedAfter);
            AssertSame(first, second, "File identity or metadata changed while open: " + Description);
            return first;
        }

        public void CopyFrom(LinuxFileLease source)
        {
            EnsureOpen();
            source.EnsureOpen();
            LinuxMetadata sourceMetadata = source.ReadRegularMetadata();
            bool sourceReference = false;
            bool targetReference = false;
            source.handle.DangerousAddRef(ref sourceReference);
            handle.DangerousAddRef(ref targetReference);
            try
            {
                using (SafeFileHandle borrowedSource = new SafeFileHandle(source.handle.DangerousGetHandle(), false))
                using (SafeFileHandle borrowedTarget = new SafeFileHandle(handle.DangerousGetHandle(), false))
                using (FileStream input = new FileStream(borrowedSource, FileAccess.Read, 65536, false))
                using (FileStream output = new FileStream(borrowedTarget, FileAccess.ReadWrite, 65536, false))
                {
                    input.Seek(0, SeekOrigin.Begin);
                    output.Seek(0, SeekOrigin.Begin);
                    output.SetLength(0);
                    input.CopyTo(output, 65536);
                    output.Flush(true);
                }
                if (LinuxNative.FChmodNative(Descriptor, sourceMetadata.Mode & 0x0fff) != 0)
                {
                    throw LinuxNative.LastError("Unable to preserve source mode on prepared stage " + Description);
                }
            }
            finally
            {
                if (targetReference) handle.DangerousRelease();
                if (sourceReference) source.handle.DangerousRelease();
            }
        }

        public void WriteAllBytes(byte[] bytes)
        {
            EnsureOpen();
            bool targetReference = false;
            handle.DangerousAddRef(ref targetReference);
            try
            {
                using (SafeFileHandle borrowedTarget = new SafeFileHandle(handle.DangerousGetHandle(), false))
                using (FileStream output = new FileStream(borrowedTarget, FileAccess.ReadWrite, 65536, false))
                {
                    output.Seek(0, SeekOrigin.Begin);
                    output.SetLength(0);
                    output.Write(bytes, 0, bytes.Length);
                    output.Flush(true);
                }
            }
            finally
            {
                if (targetReference) handle.DangerousRelease();
            }
        }

        public void SetUserXAttrForSelfTest(string name, byte[] value)
        {
            EnsureOpen();
            if (String.IsNullOrWhiteSpace(name) || !name.StartsWith("user.", StringComparison.Ordinal))
            {
                throw new ArgumentException("Self-test xattr must use the user namespace.");
            }
            byte[] nameBytes = System.Text.Encoding.UTF8.GetBytes(name);
            IntPtr nameBuffer = Marshal.AllocHGlobal(nameBytes.Length + 1);
            IntPtr valueBuffer = value.Length == 0 ? IntPtr.Zero : Marshal.AllocHGlobal(value.Length);
            try
            {
                Marshal.Copy(nameBytes, 0, nameBuffer, nameBytes.Length);
                Marshal.WriteByte(nameBuffer, nameBytes.Length, 0);
                if (value.Length > 0) Marshal.Copy(value, 0, valueBuffer, value.Length);
                if (LinuxNative.FSetXAttrNative(
                        Descriptor,
                        nameBuffer,
                        valueBuffer,
                        new UIntPtr(unchecked((ulong)value.Length)),
                        0) != 0)
                {
                    throw LinuxNative.LastError("Unable to set self-test user xattr on " + Description);
                }
            }
            finally
            {
                if (valueBuffer != IntPtr.Zero) Marshal.FreeHGlobal(valueBuffer);
                Marshal.FreeHGlobal(nameBuffer);
            }
        }

        private LinuxMetadata ReadRegularMetadata()
        {
            LinuxMetadata metadata = LinuxNative.ReadMetadata(handle, Description);
            if ((metadata.Mode & LinuxNative.FileTypeMask) != LinuxNative.RegularFile)
            {
                throw new IOException("Leaf CAS target must remain a regular file: " + Description);
            }
            return metadata;
        }

        private string HashContent()
        {
            bool reference = false;
            handle.DangerousAddRef(ref reference);
            try
            {
                using (SafeFileHandle borrowed = new SafeFileHandle(handle.DangerousGetHandle(), false))
                using (FileStream stream = new FileStream(borrowed, FileAccess.Read, 65536, false))
                using (SHA256 sha = SHA256.Create())
                {
                    stream.Seek(0, SeekOrigin.Begin);
                    return ToHex(sha.ComputeHash(stream));
                }
            }
            finally
            {
                if (reference) handle.DangerousRelease();
            }
        }

        private string HashExtendedMetadata()
        {
            EnsureOpen();
            int descriptor = Descriptor;
            long listLength = LinuxNative.FListXAttrNative(descriptor, IntPtr.Zero, UIntPtr.Zero);
            if (listLength < 0)
            {
                throw LinuxNative.LastError("Unable to enumerate fd xattrs for " + Description);
            }
            byte[] listBytes = new byte[checked((int)listLength)];
            if (listBytes.Length > 0)
            {
                IntPtr list = Marshal.AllocHGlobal(listBytes.Length);
                try
                {
                    long actual = LinuxNative.FListXAttrNative(
                        descriptor,
                        list,
                        new UIntPtr(unchecked((ulong)listBytes.Length)));
                    if (actual != listBytes.Length)
                    {
                        if (actual < 0) throw LinuxNative.LastError("fd xattr names changed for " + Description);
                        throw new IOException("fd xattr names changed for " + Description);
                    }
                    Marshal.Copy(list, listBytes, 0, listBytes.Length);
                }
                finally
                {
                    Marshal.FreeHGlobal(list);
                }
            }

            System.Collections.Generic.List<byte[]> names = new System.Collections.Generic.List<byte[]>();
            int start = 0;
            for (int index = 0; index <= listBytes.Length; index++)
            {
                if (index == listBytes.Length || listBytes[index] == 0)
                {
                    if (index > start)
                    {
                        byte[] name = new byte[index - start];
                        Buffer.BlockCopy(listBytes, start, name, 0, name.Length);
                        names.Add(name);
                    }
                    start = index + 1;
                }
            }
            names.Sort(delegate(byte[] left, byte[] right)
            {
                int shared = Math.Min(left.Length, right.Length);
                for (int index = 0; index < shared; index++)
                {
                    int compared = left[index].CompareTo(right[index]);
                    if (compared != 0) return compared;
                }
                return left.Length.CompareTo(right.Length);
            });

            using (MemoryStream canonical = new MemoryStream())
            using (BinaryWriter writer = new BinaryWriter(canonical, System.Text.Encoding.UTF8, true))
            {
                foreach (byte[] name in names)
                {
                    IntPtr nameBuffer = Marshal.AllocHGlobal(name.Length + 1);
                    try
                    {
                        Marshal.Copy(name, 0, nameBuffer, name.Length);
                        Marshal.WriteByte(nameBuffer, name.Length, 0);
                        long valueLength = LinuxNative.FGetXAttrNative(
                            descriptor,
                            nameBuffer,
                            IntPtr.Zero,
                            UIntPtr.Zero);
                        if (valueLength < 0)
                        {
                            throw LinuxNative.LastError("Unable to read fd xattr for " + Description);
                        }
                        byte[] value = new byte[checked((int)valueLength)];
                        if (value.Length > 0)
                        {
                            IntPtr valueBuffer = Marshal.AllocHGlobal(value.Length);
                            try
                            {
                                long actual = LinuxNative.FGetXAttrNative(
                                    descriptor,
                                    nameBuffer,
                                    valueBuffer,
                                    new UIntPtr(unchecked((ulong)value.Length)));
                                if (actual != value.Length)
                                {
                                    if (actual < 0) throw LinuxNative.LastError("fd xattr value changed for " + Description);
                                    throw new IOException("fd xattr value changed for " + Description);
                                }
                                Marshal.Copy(valueBuffer, value, 0, value.Length);
                            }
                            finally
                            {
                                Marshal.FreeHGlobal(valueBuffer);
                            }
                        }
                        writer.Write(name.Length);
                        writer.Write(name);
                        writer.Write(value.Length);
                        writer.Write(value);
                    }
                    finally
                    {
                        Marshal.FreeHGlobal(nameBuffer);
                    }
                }
                writer.Flush();
                canonical.Position = 0;
                using (SHA256 sha = SHA256.Create())
                {
                    return ToHex(sha.ComputeHash(canonical));
                }
            }
        }

        private static LinuxFileSnapshot ToSnapshot(
            LinuxMetadata metadata,
            string content,
            string extended)
        {
            return new LinuxFileSnapshot
            {
                DeviceId = metadata.DeviceId,
                Inode = metadata.Inode,
                Mode = metadata.Mode,
                UserId = metadata.UserId,
                GroupId = metadata.GroupId,
                Size = metadata.Size,
                ContentSha256 = content,
                ExtendedMetadataSha256 = extended
            };
        }

        private static void AssertSame(
            LinuxFileSnapshot actual,
            LinuxFileSnapshot expected,
            string message)
        {
            if (actual.DeviceId != expected.DeviceId || actual.Inode != expected.Inode ||
                actual.Mode != expected.Mode || actual.UserId != expected.UserId ||
                actual.GroupId != expected.GroupId || actual.Size != expected.Size ||
                !String.Equals(actual.ContentSha256, expected.ContentSha256, StringComparison.OrdinalIgnoreCase) ||
                !String.Equals(actual.ExtendedMetadataSha256, expected.ExtendedMetadataSha256, StringComparison.Ordinal))
            {
                throw new IOException(message);
            }
        }

        private string Description
        {
            get { return Path.Combine(parentPath, leaf); }
        }

        private void EnsureOpen()
        {
            if (handle == null || handle.IsClosed || handle.IsInvalid)
            {
                throw new ObjectDisposedException("LinuxFileLease");
            }
        }

        private static string ToHex(byte[] bytes)
        {
            return BitConverter.ToString(bytes).Replace("-", "").ToLowerInvariant();
        }

        public void Dispose()
        {
            if (handle != null)
            {
                handle.Dispose();
                handle = null;
            }
        }
    }
}
'@ | Out-Null
}

function New-InstallDirectoryLease {
    param([Parameter(Mandatory = $true)][string]$Path)

    Initialize-HandleCasNative
    Initialize-LinuxHandleCasNative
    if ($IsWindows) {
        return [RaymanInstallerV2.WindowsDirectoryLease]::Open($Path)
    }
    return [RaymanInstallerV2.LinuxDirectoryLease]::Open($Path)
}

function Open-InstallSourceLease {
    param([Parameter(Mandatory = $true)][string]$Path)

    Initialize-HandleCasNative
    Initialize-LinuxHandleCasNative
    if ($IsWindows) {
        return [RaymanInstallerV2.WindowsFileLease]::OpenSource($Path)
    }
    return [RaymanInstallerV2.LinuxFileLease]::OpenSource($Path)
}

function Open-InstallLeafLease {
    param(
        [Parameter(Mandatory = $true)]$ParentLease,
        [Parameter(Mandatory = $true)][string]$Leaf,
        [switch]$DeleteAccess
    )

    if ($IsWindows) {
        return $ParentLease.TryOpenFile($Leaf, [bool]$DeleteAccess)
    }
    return $ParentLease.TryOpenFile($Leaf)
}

function Open-InstallDirectoryLeafLease {
    param(
        [Parameter(Mandatory = $true)]$ParentLease,
        [Parameter(Mandatory = $true)][string]$Leaf,
        [switch]$DeleteAccess
    )

    if ($IsWindows) {
        return $ParentLease.TryOpenDirectory($Leaf, [bool]$DeleteAccess)
    }
    return $ParentLease.TryOpenDirectory($Leaf)
}

function Assert-InstallFileSnapshot {
    param(
        [Parameter(Mandatory = $true)]$Actual,
        [Parameter(Mandatory = $true)]$Expected,
        [Parameter(Mandatory = $true)][string]$ExpectedHash,
        [Parameter(Mandatory = $true)][string]$Label
    )

    if ($IsWindows) {
        if ($Actual.VolumeSerialNumber -ne $Expected.VolumeSerialNumber -or
            $Actual.FileIndex -ne $Expected.FileIndex) {
            throw "$Label platform identity changed: volume/file ID differs"
        }
        if ($Actual.Size -ne $Expected.Size -or
            $Actual.Attributes -ne $Expected.Attributes -or
            -not [string]::Equals(
                [string]$Actual.SecurityDescriptorSha256,
                [string]$Expected.SecurityDescriptorSha256,
                [StringComparison]::Ordinal
            )) {
            throw "$Label Windows size, attributes, or owner/group/DACL metadata changed"
        }
    } else {
        if ($Actual.DeviceId -ne $Expected.DeviceId -or $Actual.Inode -ne $Expected.Inode) {
            throw "$Label platform identity changed: device/inode differs"
        }
        if ($Actual.Mode -ne $Expected.Mode -or
            $Actual.UserId -ne $Expected.UserId -or
            $Actual.GroupId -ne $Expected.GroupId -or
            $Actual.Size -ne $Expected.Size -or
            -not [string]::Equals(
                [string]$Actual.ExtendedMetadataSha256,
                [string]$Expected.ExtendedMetadataSha256,
                [StringComparison]::Ordinal
            )) {
            throw "$Label Linux mode/uid/gid/size or fd xattr metadata changed"
        }
    }
    if (-not [string]::Equals(
            [string]$Actual.ContentSha256,
            [string]$Expected.ContentSha256,
            [StringComparison]::OrdinalIgnoreCase
        ) -or
        -not [string]::Equals(
            [string]$Actual.ContentSha256,
            $ExpectedHash,
            [StringComparison]::OrdinalIgnoreCase
        )) {
        throw "$Label content hash changed: $($Actual.ContentSha256) != $ExpectedHash"
    }
}

function Get-InstallFileCasSnapshot {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$ExpectedHash,
        [Parameter(Mandatory = $true)][string]$Label
    )

    $lease = Open-InstallSourceLease -Path $Path
    try {
        $snapshot = $lease.CaptureSnapshot()
        Assert-InstallFileSnapshot -Actual $snapshot -Expected $snapshot -ExpectedHash $ExpectedHash -Label $Label
        return $snapshot
    } finally {
        $lease.Dispose()
    }
}

function Get-InstallPlatformIdentity {
    param([Parameter(Mandatory = $true)]$Snapshot)

    if ($Snapshot.PSObject.Properties.Name -contains 'VolumeSerialNumber') {
        return ('volume=0x{0:x8};file=0x{1:x16}' -f
            [uint32]$Snapshot.VolumeSerialNumber,
            [uint64]$Snapshot.FileIndex)
    }
    return ('device=0x{0:x16};inode={1}' -f
        [uint64]$Snapshot.DeviceId,
        [uint64]$Snapshot.Inode)
}

function New-InstallPathBindingLedger {
    $ledger = [Collections.Generic.List[object]]::new()
    return ,$ledger
}

function Add-InstallLedgerEntry {
    param(
        [Parameter(Mandatory = $true)]$Ledger,
        [Parameter(Mandatory = $true)]$ParentLease,
        [Parameter(Mandatory = $true)][string]$Leaf,
        [Parameter(Mandatory = $true)][string]$Role,
        [Parameter(Mandatory = $true)][string]$State,
        [Parameter(Mandatory = $true)][string]$Reason,
        [ValidateSet('Present', 'Absent', 'Historical')]
        [string]$ExpectedPresence = 'Historical',
        [bool]$Active = $false,
        $ExpectedSnapshot
    )

    $entry = [pscustomobject]@{
        Path = Join-Path $ParentLease.BoundPath $Leaf
        ObservedPath = $null
        Leaf = $Leaf
        Role = $Role
        State = $State
        Reason = $Reason
        ExpectedPresence = $ExpectedPresence
        Active = $Active
        VerificationStatus = 'not_verified'
        ExpectedPlatformIdentity = $null
        ObservedPlatformIdentity = $null
        ExpectedContentHash = $null
        ObservedContentHash = $null
        ExpectedWindowsAttributes = $null
        ObservedWindowsAttributes = $null
        ExpectedWindowsSecurityDescriptorHash = $null
        ObservedWindowsSecurityDescriptorHash = $null
        ExpectedLinuxMode = $null
        ObservedLinuxMode = $null
        ExpectedLinuxUid = $null
        ObservedLinuxUid = $null
        ExpectedLinuxGid = $null
        ObservedLinuxGid = $null
        ExpectedLinuxSize = $null
        ObservedLinuxSize = $null
        ExpectedLinuxXattrMetadataHash = $null
        ObservedLinuxXattrMetadataHash = $null
    }
    if ($null -ne $ExpectedSnapshot) {
        Set-InstallLedgerSnapshot -Entry $entry -Snapshot $ExpectedSnapshot -Kind Expected
    }
    $Ledger.Add($entry)
    return $entry
}

function Set-InstallLedgerSnapshot {
    param(
        [Parameter(Mandatory = $true)]$Entry,
        [Parameter(Mandatory = $true)]$Snapshot,
        [ValidateSet('Expected', 'Observed')]
        [Parameter(Mandatory = $true)][string]$Kind
    )

    $Entry."${Kind}PlatformIdentity" = Get-InstallPlatformIdentity -Snapshot $Snapshot
    $Entry."${Kind}ContentHash" = [string]$Snapshot.ContentSha256
    if ($Snapshot.PSObject.Properties.Name -contains 'VolumeSerialNumber') {
        $Entry."${Kind}WindowsAttributes" = [uint32]$Snapshot.Attributes
        $Entry."${Kind}WindowsSecurityDescriptorHash" = [string]$Snapshot.SecurityDescriptorSha256
    } else {
        $Entry."${Kind}LinuxMode" = [uint32]$Snapshot.Mode
        $Entry."${Kind}LinuxUid" = [uint32]$Snapshot.UserId
        $Entry."${Kind}LinuxGid" = [uint32]$Snapshot.GroupId
        $Entry."${Kind}LinuxSize" = [int64]$Snapshot.Size
        $Entry."${Kind}LinuxXattrMetadataHash" = [string]$Snapshot.ExtendedMetadataSha256
    }
}

function Set-InstallLedgerEntryState {
    param(
        [Parameter(Mandatory = $true)]$Entry,
        [Parameter(Mandatory = $true)][string]$State,
        [ValidateSet('Present', 'Absent', 'Historical')]
        [string]$ExpectedPresence,
        [bool]$Active,
        [string]$Reason
    )

    $Entry.State = $State
    if ($PSBoundParameters.ContainsKey('ExpectedPresence')) {
        $Entry.ExpectedPresence = $ExpectedPresence
    }
    if ($PSBoundParameters.ContainsKey('Active')) {
        $Entry.Active = $Active
    }
    if ($PSBoundParameters.ContainsKey('Reason')) {
        $Entry.Reason = $Reason
    }
}

function Invoke-InstallTestHook {
    param(
        [AllowNull()]$TestHooks,
        [Parameter(Mandatory = $true)][string]$Name,
        $Context
    )

    if ($null -eq $TestHooks -or -not $TestHooks.Contains($Name)) {
        return
    }
    & $TestHooks[$Name] $Context
}

function Confirm-InstallLedgerPresentEntry {
    param(
        [Parameter(Mandatory = $true)]$ParentLease,
        [Parameter(Mandatory = $true)]$Entry,
        [Parameter(Mandatory = $true)]$ExpectedSnapshot,
        [Parameter(Mandatory = $true)][string]$ExpectedHash,
        [Parameter(Mandatory = $true)][string]$Label,
        [switch]$DeleteAccess
    )

    $lease = Open-InstallLeafLease -ParentLease $ParentLease -Leaf $Entry.Leaf -DeleteAccess:$DeleteAccess
    if ($null -eq $lease) {
        $Entry.VerificationStatus = 'missing'
        throw "$Label is missing after raw mutation: $($Entry.Path)"
    }
    try {
        $snapshot = $lease.CaptureSnapshot()
        Set-InstallLedgerSnapshot -Entry $Entry -Snapshot $snapshot -Kind Observed
        $Entry.ObservedPath = Join-Path $ParentLease.GetCurrentPath() $Entry.Leaf
        Assert-InstallFileSnapshot -Actual $snapshot -Expected $ExpectedSnapshot -ExpectedHash $ExpectedHash -Label $Label
        $Entry.VerificationStatus = 'verified'
        return $lease
    } catch {
        $Entry.VerificationStatus = 'identity_or_metadata_mismatch'
        $lease.Dispose()
        throw
    }
}

function Reconcile-InstallPathBindingLedger {
    param(
        [Parameter(Mandatory = $true)]$Ledger,
        [Parameter(Mandatory = $true)]$ParentLease
    )

    $errors = @()
    $observedParent = $null
    try {
        $observedParent = $ParentLease.GetCurrentPath()
        $ParentLease.AssertPathBinding()
    } catch {
        $errors += "terminal parent binding failed: $($_.Exception.Message)"
        try {
            $observedParent = $ParentLease.GetCurrentPath()
        } catch {
            $observedParent = $ParentLease.BoundPath
        }
    }
    foreach ($entry in @($Ledger | Where-Object { $_.Active })) {
        $entry.ObservedPath = Join-Path $observedParent $entry.Leaf
        $lease = $null
        try {
            $lease = Open-InstallLeafLease -ParentLease $ParentLease -Leaf $entry.Leaf
            if ($null -eq $lease) {
                if ($entry.ExpectedPresence -eq 'Absent') {
                    $entry.VerificationStatus = 'verified_absent'
                } else {
                    $entry.VerificationStatus = 'missing'
                    $errors += "active ledger path is missing: role=$($entry.Role) leaf=$($entry.Leaf)"
                }
                continue
            }
            $snapshot = $lease.CaptureSnapshot()
            Set-InstallLedgerSnapshot -Entry $entry -Snapshot $snapshot -Kind Observed
            if ($entry.ExpectedPresence -eq 'Absent') {
                $entry.VerificationStatus = 'unexpected_object_preserved'
                $errors += "expected-vacant ledger leaf is occupied and was preserved: role=$($entry.Role) leaf=$($entry.Leaf) observed=$($entry.ObservedPlatformIdentity)"
                continue
            }
            if ([string]::IsNullOrWhiteSpace([string]$entry.ExpectedPlatformIdentity)) {
                $entry.VerificationStatus = 'present_without_expected_identity'
                $errors += "active present ledger entry lacks expected identity: role=$($entry.Role) leaf=$($entry.Leaf)"
                continue
            }
            $expectedMatches =
                [string]::Equals([string]$entry.ExpectedPlatformIdentity, [string]$entry.ObservedPlatformIdentity, [StringComparison]::Ordinal) -and
                [string]::Equals([string]$entry.ExpectedContentHash, [string]$entry.ObservedContentHash, [StringComparison]::OrdinalIgnoreCase) -and
                $entry.ExpectedWindowsAttributes -eq $entry.ObservedWindowsAttributes -and
                [string]::Equals([string]$entry.ExpectedWindowsSecurityDescriptorHash, [string]$entry.ObservedWindowsSecurityDescriptorHash, [StringComparison]::Ordinal) -and
                $entry.ExpectedLinuxMode -eq $entry.ObservedLinuxMode -and
                $entry.ExpectedLinuxUid -eq $entry.ObservedLinuxUid -and
                $entry.ExpectedLinuxGid -eq $entry.ObservedLinuxGid -and
                $entry.ExpectedLinuxSize -eq $entry.ObservedLinuxSize -and
                [string]::Equals([string]$entry.ExpectedLinuxXattrMetadataHash, [string]$entry.ObservedLinuxXattrMetadataHash, [StringComparison]::Ordinal)
            if ($expectedMatches) {
                $entry.VerificationStatus = 'verified'
            } else {
                $entry.VerificationStatus = 'identity_or_metadata_mismatch'
                $errors += "ledger binding mismatch preserved: role=$($entry.Role) leaf=$($entry.Leaf) expected=$($entry.ExpectedPlatformIdentity) observed=$($entry.ObservedPlatformIdentity)"
            }
        } catch {
            $entry.VerificationStatus = 'reopen_failed'
            $errors += "ledger reopen failed: role=$($entry.Role) leaf=$($entry.Leaf): $($_.Exception.Message)"
        } finally {
            if ($null -ne $lease) {
                $lease.Dispose()
            }
        }
    }
    return $errors
}

function Test-RetainedInstallLedgerEntry {
    param([Parameter(Mandatory = $true)]$Entry)

    if (-not $Entry.Active -or
        $Entry.ExpectedPresence -ne 'Present' -or
        $Entry.VerificationStatus -eq 'missing') {
        return $false
    }
    $retainedState = "$($Entry.Role)|$($Entry.State)"
    return $retainedState -in @(
        'prepared_stage|retained_prepared_stage',
        'prepared_stage|retained_after_failure',
        'preflight_retained_evidence|hardlink_preflight_raw_success',
        'preflight_retained_evidence|retained_verified',
        'retained_original_destination|isolation_raw_success',
        'retained_original_destination|retained_verified',
        'rollback_retained_publication|isolation_raw_success',
        'rollback_retained_publication|retained_verified',
        'rollback_backup|backup_link_raw_success',
        'rollback_backup|retained_after_restore',
        'rollback_backup|rollback_failed_retained',
        'rollback_backup|committed_retained_backup',
        'rollback_backup|committed_cleanup_failed_retained'
    )
}

function Get-RetainedInstallLedgerEntry {
    param([Parameter(Mandatory = $true)]$Ledger)

    return @($Ledger | Where-Object { Test-RetainedInstallLedgerEntry -Entry $_ })
}

function Format-InstallPathBindingLedger {
    param([Parameter(Mandatory = $true)]$Ledger)

    return @($Ledger | ForEach-Object {
        "path=$($_.Path) observed_path=$($_.ObservedPath) leaf=$($_.Leaf) role=$($_.Role) state=$($_.State) reason=$($_.Reason) active=$($_.Active) expected_presence=$($_.ExpectedPresence) verification=$($_.VerificationStatus) expected_identity=$($_.ExpectedPlatformIdentity) observed_identity=$($_.ObservedPlatformIdentity) content=$($_.ObservedContentHash) windows_attributes=$($_.ObservedWindowsAttributes) windows_security=$($_.ObservedWindowsSecurityDescriptorHash) linux_mode=$($_.ObservedLinuxMode) linux_uid=$($_.ObservedLinuxUid) linux_gid=$($_.ObservedLinuxGid) linux_size=$($_.ObservedLinuxSize) linux_xattr=$($_.ObservedLinuxXattrMetadataHash)"
    })
}

function New-InstallLedgerException {
    param(
        [Parameter(Mandatory = $true)][string]$Message,
        [Parameter(Mandatory = $true)]$Ledger,
        [string[]]$RecoveryErrors
    )

    $script:LastInstallPathBindingLedger = @($Ledger)
    $lines = @($Message)
    if (@($RecoveryErrors).Count -gt 0) {
        $lines += 'Recovery/terminal verification errors:'
        $lines += @($RecoveryErrors)
    }
    $lines += 'Incremental path-binding ledger:'
    $lines += @(Format-InstallPathBindingLedger -Ledger $Ledger)
    $exception = [InvalidOperationException]::new(($lines -join [Environment]::NewLine))
    $exception.Data['RaymanInstallPathBindingLedger'] = @($Ledger)
    return $exception
}

function Invoke-InstallRawRenameNoReplace {
    param(
        [Parameter(Mandatory = $true)]$ParentLease,
        [Parameter(Mandatory = $true)]$SourceLease,
        [Parameter(Mandatory = $true)][string]$SourceLeaf,
        [Parameter(Mandatory = $true)][string]$TargetLeaf
    )

    if ($IsWindows) {
        $SourceLease.RawRenameNoReplaceTo($ParentLease, $TargetLeaf)
        return
    }
    $ParentLease.RawRenameNoReplace($SourceLeaf, $ParentLease, $TargetLeaf)
}

function Assert-InstallLeafAbsent {
    param(
        [Parameter(Mandatory = $true)]$ParentLease,
        [Parameter(Mandatory = $true)][string]$Leaf,
        [Parameter(Mandatory = $true)][string]$Label
    )

    $lease = Open-InstallLeafLease -ParentLease $ParentLease -Leaf $Leaf
    if ($null -eq $lease) {
        return
    }
    try {
        $snapshot = $lease.CaptureSnapshot()
        throw "$Label is occupied and was preserved: leaf=$Leaf identity=$(Get-InstallPlatformIdentity -Snapshot $snapshot)"
    } finally {
        $lease.Dispose()
    }
}

function Install-FileWithRollback {
    param(
        [Parameter(Mandatory = $true)][string]$Source,
        [Parameter(Mandatory = $true)][string]$Destination,
        [Parameter(Mandatory = $true)][string]$Nonce,
        [Parameter(Mandatory = $true)][string]$ExpectedHash,
        [AllowNull()]$TestHooks
    )

    $fullDestination = [IO.Path]::GetFullPath($Destination)
    $destinationDirectory = [IO.Path]::GetDirectoryName($fullDestination)
    $destinationLeaf = [IO.Path]::GetFileName($fullDestination)
    $stageLeaf = "$destinationLeaf.install-$Nonce"
    $backupLeaf = "$destinationLeaf.backup-$Nonce"
    $ledger = New-InstallPathBindingLedger
    $parentLease = $null
    $sourceLease = $null
    $stageLease = $null
    $destinationLease = $null
    $record = $null
    try {
        $resolvedDirectory = Resolve-ManagedDirectory -Path $destinationDirectory -Label 'Managed destination directory'
        $parentLease = New-InstallDirectoryLease -Path $resolvedDirectory
        if ($IsWindows) {
            [RaymanInstallerV2.WindowsDirectoryLease]::ValidateLeaf($destinationLeaf)
            [RaymanInstallerV2.WindowsDirectoryLease]::ValidateLeaf($stageLeaf)
            [RaymanInstallerV2.WindowsDirectoryLease]::ValidateLeaf($backupLeaf)
        } else {
            [RaymanInstallerV2.LinuxDirectoryLease]::ValidateLeaf($destinationLeaf)
            [RaymanInstallerV2.LinuxDirectoryLease]::ValidateLeaf($stageLeaf)
            [RaymanInstallerV2.LinuxDirectoryLease]::ValidateLeaf($backupLeaf)
        }
        $record = [pscustomobject]@{
            Destination = $fullDestination
            DestinationDirectory = $resolvedDirectory
            DestinationLeaf = $destinationLeaf
            Backup = Join-Path $resolvedDirectory $backupLeaf
            BackupLeaf = $backupLeaf
            CurrentStageLeaf = $stageLeaf
            ParentLease = $parentLease
            ParentLeaseDisposed = $false
            HadOriginal = $false
            OriginalMoved = $false
            PublishedDestination = $false
            StageActive = $false
            LinuxPreflightComplete = $false
            PublishedHash = $ExpectedHash
            BackupHash = $null
            StageHash = $null
            PublishedIdentity = $null
            BackupIdentity = $null
            StageIdentity = $null
            OriginalEntry = $null
            BackupEntry = $null
            PublicationEntry = $null
            StageEntry = $null
            DestinationVacancyEntry = $null
            PathBindingLedger = $ledger
        }

        Invoke-InstallTestHook -TestHooks $TestHooks -Name 'after_parent_lease' -Context $record
        $parentLease.AssertPathBinding()

        $sourceLease = Open-InstallSourceLease -Path $Source
        $sourceIdentity = $sourceLease.CaptureSnapshot()
        Assert-InstallFileSnapshot -Actual $sourceIdentity -Expected $sourceIdentity -ExpectedHash $ExpectedHash -Label 'Verified install source'

        $stageEntry = Add-InstallLedgerEntry -Ledger $ledger -ParentLease $parentLease -Leaf $stageLeaf -Role 'prepared_stage' -State 'create_planned' -Reason 'exclusive stage reservation'
        $record.StageEntry = $stageEntry
        try {
            $stageLease = $parentLease.RawCreateFileExclusive($stageLeaf)
        } catch {
            Set-InstallLedgerEntryState -Entry $stageEntry -State 'exclusive_create_rejected' -ExpectedPresence Historical -Active $false -Reason $_.Exception.Message
            throw
        }
        $record.StageActive = $true
        Set-InstallLedgerEntryState -Entry $stageEntry -State 'created_raw' -ExpectedPresence Present -Active $true -Reason 'exclusive native stage creation succeeded'
        $emptyStageIdentity = $stageLease.CaptureSnapshot()
        $record.StageIdentity = $emptyStageIdentity
        $record.StageHash = [string]$emptyStageIdentity.ContentSha256
        Set-InstallLedgerSnapshot -Entry $stageEntry -Snapshot $emptyStageIdentity -Kind Expected
        Set-InstallLedgerSnapshot -Entry $stageEntry -Snapshot $emptyStageIdentity -Kind Observed
        $stageEntry.VerificationStatus = 'created_handle_verified'
        Invoke-InstallTestHook -TestHooks $TestHooks -Name 'after_stage_create_raw' -Context $record

        $stageLease.CopyFrom($sourceLease)
        $stageIdentity = $stageLease.CaptureSnapshot()
        Assert-InstallFileSnapshot -Actual $stageIdentity -Expected $stageIdentity -ExpectedHash $ExpectedHash -Label 'Prepared install stage'
        $sourceAfterCopy = $sourceLease.CaptureSnapshot()
        Assert-InstallFileSnapshot -Actual $sourceAfterCopy -Expected $sourceIdentity -ExpectedHash $ExpectedHash -Label 'Verified install source after opened-object copy'
        $record.StageIdentity = $stageIdentity
        $record.StageHash = $ExpectedHash
        Set-InstallLedgerSnapshot -Entry $stageEntry -Snapshot $stageIdentity -Kind Expected
        Set-InstallLedgerSnapshot -Entry $stageEntry -Snapshot $stageIdentity -Kind Observed
        Set-InstallLedgerEntryState -Entry $stageEntry -State 'prepared_verified' -ExpectedPresence Present -Active $true -Reason 'opened stage copied, flushed, captured, and source reverified'
        $stageEntry.VerificationStatus = 'verified'

        if ($IsLinux) {
            $preflightRenameLeaf = "$destinationLeaf.install-$Nonce.preflight"
            $preflightEvidenceLeaf = "$destinationLeaf.install-$Nonce.preflight-retained"
            [RaymanInstallerV2.LinuxDirectoryLease]::ValidateLeaf($preflightRenameLeaf)
            [RaymanInstallerV2.LinuxDirectoryLease]::ValidateLeaf($preflightEvidenceLeaf)
            $renamedStageEntry = Add-InstallLedgerEntry -Ledger $ledger -ParentLease $parentLease -Leaf $preflightRenameLeaf -Role 'prepared_stage' -State 'preflight_rename_planned' -Reason 'renameat2 capability preflight before public mutation' -ExpectedSnapshot $stageIdentity
            Invoke-InstallTestHook -TestHooks $TestHooks -Name 'before_linux_preflight_rename_raw' -Context $record
            Assert-InstallFileSnapshot -Actual $stageLease.CaptureSnapshot() -Expected $stageIdentity -ExpectedHash $ExpectedHash -Label 'Prepared stage immediately before renameat2 preflight'
            $parentLease.RawRenameNoReplace($stageLeaf, $parentLease, $preflightRenameLeaf)
            $record.CurrentStageLeaf = $preflightRenameLeaf
            Set-InstallLedgerEntryState -Entry $stageEntry -State 'renamed_from_raw' -ExpectedPresence Historical -Active $false -Reason 'renameat2 preflight source moved'
            Set-InstallLedgerEntryState -Entry $renamedStageEntry -State 'renameat2_preflight_raw_success' -ExpectedPresence Present -Active $true -Reason 'renameat2 symbol, kernel, and target filesystem accepted RENAME_NOREPLACE'
            $record.StageEntry = $renamedStageEntry
            Invoke-InstallTestHook -TestHooks $TestHooks -Name 'after_linux_preflight_rename_raw' -Context $record
            $verifiedRenamedStage = Confirm-InstallLedgerPresentEntry -ParentLease $parentLease -Entry $renamedStageEntry -ExpectedSnapshot $stageIdentity -ExpectedHash $ExpectedHash -Label 'renameat2-preflight prepared stage'
            $verifiedRenamedStage.Dispose()
            Assert-InstallFileSnapshot -Actual $stageLease.CaptureSnapshot() -Expected $stageIdentity -ExpectedHash $ExpectedHash -Label 'Opened prepared stage after renameat2 preflight'
            Set-InstallLedgerEntryState -Entry $renamedStageEntry -State 'renameat2_preflight_verified' -ExpectedPresence Present -Active $true -Reason 'relative renameat2 capability and identity verified'

            $preflightEvidenceEntry = Add-InstallLedgerEntry -Ledger $ledger -ParentLease $parentLease -Leaf $preflightEvidenceLeaf -Role 'preflight_retained_evidence' -State 'hardlink_preflight_planned' -Reason 'hardlink capability preflight from opened prepared inode' -ExpectedSnapshot $stageIdentity
            Invoke-InstallTestHook -TestHooks $TestHooks -Name 'before_linux_preflight_link_raw' -Context $record
            Assert-InstallFileSnapshot -Actual $stageLease.CaptureSnapshot() -Expected $stageIdentity -ExpectedHash $ExpectedHash -Label 'Prepared stage immediately before hardlink preflight'
            $parentLease.RawLinkOpenFileNoReplace($stageLease, $preflightEvidenceLeaf)
            Set-InstallLedgerEntryState -Entry $preflightEvidenceEntry -State 'hardlink_preflight_raw_success' -ExpectedPresence Present -Active $true -Reason 'hardlink from opened prepared inode succeeded; evidence is retained'
            Invoke-InstallTestHook -TestHooks $TestHooks -Name 'after_linux_preflight_link_raw' -Context $record
            $verifiedEvidence = Confirm-InstallLedgerPresentEntry -ParentLease $parentLease -Entry $preflightEvidenceEntry -ExpectedSnapshot $stageIdentity -ExpectedHash $ExpectedHash -Label 'Linux retained preflight evidence'
            $verifiedEvidence.Dispose()
            Set-InstallLedgerEntryState -Entry $preflightEvidenceEntry -State 'retained_verified' -ExpectedPresence Present -Active $true -Reason 'target filesystem hardlink capability and retained inode identity verified'
            $record.LinuxPreflightComplete = $true
        }

        $destinationLease = Open-InstallLeafLease -ParentLease $parentLease -Leaf $destinationLeaf -DeleteAccess:$IsWindows
        if ($null -ne $destinationLease) {
            $record.HadOriginal = $true
            $backupIdentity = $destinationLease.CaptureSnapshot()
            $record.BackupIdentity = $backupIdentity
            $record.BackupHash = [string]$backupIdentity.ContentSha256
            $originalEntry = Add-InstallLedgerEntry -Ledger $ledger -ParentLease $parentLease -Leaf $destinationLeaf -Role 'original_destination' -State 'captured' -Reason 'original destination captured before backup rename' -ExpectedPresence Present -Active $true -ExpectedSnapshot $backupIdentity
            Set-InstallLedgerSnapshot -Entry $originalEntry -Snapshot $backupIdentity -Kind Observed
            $originalEntry.VerificationStatus = 'verified'
            $record.OriginalEntry = $originalEntry
            Assert-InstallLeafAbsent -ParentLease $parentLease -Leaf $backupLeaf -Label 'Install backup path'
            $backupEntry = Add-InstallLedgerEntry -Ledger $ledger -ParentLease $parentLease -Leaf $backupLeaf -Role 'rollback_backup' -State 'backup_planned' -Reason 'identity-bound no-replace backup publication' -ExpectedSnapshot $backupIdentity
            $record.BackupEntry = $backupEntry
            Invoke-InstallTestHook -TestHooks $TestHooks -Name 'before_backup_rename_raw' -Context $record
            Assert-InstallFileSnapshot -Actual $destinationLease.CaptureSnapshot() -Expected $backupIdentity -ExpectedHash $record.BackupHash -Label 'Original destination immediately before backup publication'
            if ($IsWindows) {
                Invoke-InstallRawRenameNoReplace -ParentLease $parentLease -SourceLease $destinationLease -SourceLeaf $destinationLeaf -TargetLeaf $backupLeaf
                $record.OriginalMoved = $true
                Set-InstallLedgerEntryState -Entry $originalEntry -State 'renamed_from_raw' -ExpectedPresence Historical -Active $false -Reason 'original destination moved by handle-bound raw no-replace rename'
                Set-InstallLedgerEntryState -Entry $backupEntry -State 'backup_raw_success' -ExpectedPresence Present -Active $true -Reason 'handle-bound raw no-replace backup rename succeeded'
            } else {
                $parentLease.RawLinkOpenFileNoReplace($destinationLease, $backupLeaf)
                Set-InstallLedgerEntryState -Entry $backupEntry -State 'backup_link_raw_success' -ExpectedPresence Present -Active $true -Reason 'exact opened original inode hard-linked to rollback backup'
                Invoke-InstallTestHook -TestHooks $TestHooks -Name 'after_backup_link_raw' -Context $record
                $verifiedLinkedBackup = Confirm-InstallLedgerPresentEntry -ParentLease $parentLease -Entry $backupEntry -ExpectedSnapshot $backupIdentity -ExpectedHash $record.BackupHash -Label 'Linux identity-bound rollback backup'
                $verifiedLinkedBackup.Dispose()
                $retainedOriginalLeaf = "$destinationLeaf.original-retained-$Nonce"
                [RaymanInstallerV2.LinuxDirectoryLease]::ValidateLeaf($retainedOriginalLeaf)
                Assert-InstallLeafAbsent -ParentLease $parentLease -Leaf $retainedOriginalLeaf -Label 'Retained original destination path'
                $retainedOriginalEntry = Add-InstallLedgerEntry -Ledger $ledger -ParentLease $parentLease -Leaf $retainedOriginalLeaf -Role 'retained_original_destination' -State 'isolation_planned' -Reason 'public original is retained after an exact backup link exists' -ExpectedSnapshot $backupIdentity
                Assert-InstallFileSnapshot -Actual $destinationLease.CaptureSnapshot() -Expected $backupIdentity -ExpectedHash $record.BackupHash -Label 'Original destination immediately before retained isolation'
                $parentLease.RawRenameNoReplace($destinationLeaf, $parentLease, $retainedOriginalLeaf)
                $record.OriginalMoved = $true
                Set-InstallLedgerEntryState -Entry $originalEntry -State 'isolated_from_raw' -ExpectedPresence Historical -Active $false -Reason 'original public leaf moved to retained evidence after exact backup link'
                Set-InstallLedgerEntryState -Entry $retainedOriginalEntry -State 'isolation_raw_success' -ExpectedPresence Present -Active $true -Reason 'relative renameat2 retained original destination'
                Invoke-InstallTestHook -TestHooks $TestHooks -Name 'after_backup_rename_raw' -Context $record
                $verifiedRetainedOriginal = Confirm-InstallLedgerPresentEntry -ParentLease $parentLease -Entry $retainedOriginalEntry -ExpectedSnapshot $backupIdentity -ExpectedHash $record.BackupHash -Label 'Retained original destination'
                $verifiedRetainedOriginal.Dispose()
                Set-InstallLedgerEntryState -Entry $retainedOriginalEntry -State 'retained_verified' -ExpectedPresence Present -Active $true -Reason 'retained original reopened and matched immutable identity'
            }
            $destinationVacancy = Add-InstallLedgerEntry -Ledger $ledger -ParentLease $parentLease -Leaf $destinationLeaf -Role 'destination_vacancy' -State 'vacated_raw' -Reason 'original destination leaf vacated for publication' -ExpectedPresence Absent -Active $true
            $record.DestinationVacancyEntry = $destinationVacancy
            $destinationLease.Dispose()
            $destinationLease = $null
            if ($IsWindows) {
                Invoke-InstallTestHook -TestHooks $TestHooks -Name 'after_backup_rename_raw' -Context $record
            }
            $verifiedBackup = Confirm-InstallLedgerPresentEntry -ParentLease $parentLease -Entry $backupEntry -ExpectedSnapshot $backupIdentity -ExpectedHash $record.BackupHash -Label 'Rollback backup'
            $verifiedBackup.Dispose()
            Set-InstallLedgerEntryState -Entry $backupEntry -State 'backup_verified' -ExpectedPresence Present -Active $true -Reason 'backup reopened through parent lease and matched immutable original identity'
        } else {
            $record.DestinationVacancyEntry = Add-InstallLedgerEntry -Ledger $ledger -ParentLease $parentLease -Leaf $destinationLeaf -Role 'destination_vacancy' -State 'verified_initially_absent' -Reason 'first installation destination was absent' -ExpectedPresence Absent -Active $true
        }

        Assert-InstallLeafAbsent -ParentLease $parentLease -Leaf $destinationLeaf -Label 'Install publication destination'
        Assert-InstallFileSnapshot -Actual $stageLease.CaptureSnapshot() -Expected $stageIdentity -ExpectedHash $ExpectedHash -Label 'Prepared stage immediately before publication'
        Invoke-InstallTestHook -TestHooks $TestHooks -Name 'before_publication_rename_raw' -Context $record
        Assert-InstallFileSnapshot -Actual $stageLease.CaptureSnapshot() -Expected $stageIdentity -ExpectedHash $ExpectedHash -Label 'Prepared stage after publication hook and before raw publication'
        $publicationEntry = Add-InstallLedgerEntry -Ledger $ledger -ParentLease $parentLease -Leaf $destinationLeaf -Role 'installed_publication' -State 'publication_planned' -Reason 'identity-bound no-replace publication of prepared stage' -ExpectedSnapshot $stageIdentity
        $record.PublicationEntry = $publicationEntry
        if ($IsWindows) {
            Invoke-InstallRawRenameNoReplace -ParentLease $parentLease -SourceLease $stageLease -SourceLeaf $record.CurrentStageLeaf -TargetLeaf $destinationLeaf
        } else {
            $parentLease.RawLinkOpenFileNoReplace($stageLease, $destinationLeaf)
        }
        $record.PublishedDestination = $true
        $record.PublishedIdentity = $stageIdentity
        if ($IsWindows) {
            $record.StageActive = $false
            Set-InstallLedgerEntryState -Entry $record.StageEntry -State 'published_from_raw' -ExpectedPresence Historical -Active $false -Reason 'prepared stage moved by handle-bound relative rename'
        } else {
            Set-InstallLedgerEntryState -Entry $record.StageEntry -State 'retained_prepared_stage' -ExpectedPresence Present -Active $true -Reason 'prepared stage remains as retained exact-inode evidence after link publication'
        }
        Set-InstallLedgerEntryState -Entry $publicationEntry -State 'publication_raw_success' -ExpectedPresence Present -Active $true -Reason $(if ($IsWindows) { 'handle-bound raw relative no-replace publication succeeded' } else { 'opened-inode hardlink publication succeeded' })
        Set-InstallLedgerEntryState -Entry $record.DestinationVacancyEntry -State 'filled_by_publication' -ExpectedPresence Historical -Active $false -Reason 'destination vacancy consumed by exact publication'
        $stageLease.Dispose()
        $stageLease = $null
        Invoke-InstallTestHook -TestHooks $TestHooks -Name 'after_publication_rename_raw' -Context $record
        $verifiedPublication = Confirm-InstallLedgerPresentEntry -ParentLease $parentLease -Entry $publicationEntry -ExpectedSnapshot $stageIdentity -ExpectedHash $ExpectedHash -Label 'Installed publication'
        $verifiedPublication.Dispose()
        Set-InstallLedgerEntryState -Entry $publicationEntry -State 'published_verified' -ExpectedPresence Present -Active $true -Reason 'publication reopened through held parent and matched immutable stage identity'

        $terminalErrors = @(Reconcile-InstallPathBindingLedger -Ledger $ledger -ParentLease $parentLease)
        if ($terminalErrors.Count -gt 0) {
            throw (New-InstallLedgerException -Message 'Install-file terminal binding verification failed before transaction return.' -Ledger $ledger -RecoveryErrors $terminalErrors)
        }
        return $record
    } catch {
        $installFailure = $_.Exception.Message
        foreach ($lease in @($destinationLease, $stageLease, $sourceLease)) {
            if ($null -ne $lease) {
                try { $lease.Dispose() } catch { }
            }
        }
        $destinationLease = $null
        $stageLease = $null
        $sourceLease = $null
        if ($null -eq $record -or $null -eq $parentLease) {
            throw
        }
        $recoveryErrors = @(Invoke-InstallRecordRecovery -InstallRecord $record -TestHooks $TestHooks -ReleaseParent)
        throw (New-InstallLedgerException -Message "Install-file transaction failed: $installFailure" -Ledger $ledger -RecoveryErrors $recoveryErrors)
    } finally {
        foreach ($lease in @($destinationLease, $stageLease, $sourceLease)) {
            if ($null -ne $lease) {
                $lease.Dispose()
            }
        }
    }
}

function Invoke-InstallRecordRecovery {
    param(
        [Parameter(Mandatory = $true)]$InstallRecord,
        [AllowNull()]$TestHooks,
        [switch]$ReleaseParent
    )

    $errors = @()
    $parentLease = $InstallRecord.ParentLease
    if ($InstallRecord.ParentLeaseDisposed -or $null -eq $parentLease) {
        return @('terminal parent lease was already disposed before rollback/cleanup')
    }

    if ($InstallRecord.PublishedDestination) {
        $publicationLease = $null
        try {
            $publicationLease = Open-InstallLeafLease -ParentLease $parentLease -Leaf $InstallRecord.DestinationLeaf -DeleteAccess:$IsWindows
            if ($null -eq $publicationLease) {
                throw 'published destination is missing; refusing to infer successful removal'
            }
            Assert-InstallFileSnapshot -Actual $publicationLease.CaptureSnapshot() -Expected $InstallRecord.PublishedIdentity -ExpectedHash $InstallRecord.PublishedHash -Label 'Rollback publication immediately before raw isolation'
            Invoke-InstallTestHook -TestHooks $TestHooks -Name 'before_rollback_publication_raw' -Context $InstallRecord
            Assert-InstallFileSnapshot -Actual $publicationLease.CaptureSnapshot() -Expected $InstallRecord.PublishedIdentity -ExpectedHash $InstallRecord.PublishedHash -Label 'Rollback publication after hook and before raw isolation'
            if ($IsWindows) {
                $publicationLease.RawMarkDelete()
                $InstallRecord.PublishedDestination = $false
                Set-InstallLedgerEntryState -Entry $InstallRecord.PublicationEntry -State 'delete_disposition_raw_success' -ExpectedPresence Historical -Active $false -Reason 'Windows one-byte identity-bound delete disposition succeeded'
                $vacancy = Add-InstallLedgerEntry -Ledger $InstallRecord.PathBindingLedger -ParentLease $parentLease -Leaf $InstallRecord.DestinationLeaf -Role 'destination_vacancy' -State 'delete_disposition_raw_success' -Reason 'publication leaf expected absent after handle close' -ExpectedPresence Absent -Active $true
                $InstallRecord.DestinationVacancyEntry = $vacancy
                $publicationLease.Dispose()
                $publicationLease = $null
                Invoke-InstallTestHook -TestHooks $TestHooks -Name 'after_rollback_publication_raw' -Context $InstallRecord
            } else {
                $retainedLeaf = "$($InstallRecord.DestinationLeaf).rollback-retained-$([Guid]::NewGuid().ToString('N'))"
                [RaymanInstallerV2.LinuxDirectoryLease]::ValidateLeaf($retainedLeaf)
                Assert-InstallLeafAbsent -ParentLease $parentLease -Leaf $retainedLeaf -Label 'Rollback retained publication path'
                $retainedEntry = Add-InstallLedgerEntry -Ledger $InstallRecord.PathBindingLedger -ParentLease $parentLease -Leaf $retainedLeaf -Role 'rollback_retained_publication' -State 'isolation_planned' -Reason 'Linux rollback retains exact publication instead of unlinking' -ExpectedSnapshot $InstallRecord.PublishedIdentity
                $parentLease.RawRenameNoReplace($InstallRecord.DestinationLeaf, $parentLease, $retainedLeaf)
                $InstallRecord.PublishedDestination = $false
                Set-InstallLedgerEntryState -Entry $InstallRecord.PublicationEntry -State 'isolated_from_raw' -ExpectedPresence Historical -Active $false -Reason 'publication moved to retained evidence'
                Set-InstallLedgerEntryState -Entry $retainedEntry -State 'isolation_raw_success' -ExpectedPresence Present -Active $true -Reason 'relative renameat2 retained rollback publication'
                $vacancy = Add-InstallLedgerEntry -Ledger $InstallRecord.PathBindingLedger -ParentLease $parentLease -Leaf $InstallRecord.DestinationLeaf -Role 'destination_vacancy' -State 'isolated_raw' -Reason 'publication leaf vacated by retained isolation' -ExpectedPresence Absent -Active $true
                $InstallRecord.DestinationVacancyEntry = $vacancy
                Invoke-InstallTestHook -TestHooks $TestHooks -Name 'after_rollback_publication_raw' -Context $InstallRecord
                $verifiedRetained = Confirm-InstallLedgerPresentEntry -ParentLease $parentLease -Entry $retainedEntry -ExpectedSnapshot $InstallRecord.PublishedIdentity -ExpectedHash $InstallRecord.PublishedHash -Label 'Rollback retained publication'
                $verifiedRetained.Dispose()
                Set-InstallLedgerEntryState -Entry $retainedEntry -State 'retained_verified' -ExpectedPresence Present -Active $true -Reason 'rollback publication retained and identity verified'
            }
        } catch {
            $errors += "unable to isolate exact failed/current publication: $($_.Exception.Message)"
        } finally {
            if ($null -ne $publicationLease) {
                $publicationLease.Dispose()
            }
        }
    }

    if ($InstallRecord.OriginalMoved) {
        $backupLease = $null
        try {
            Assert-InstallLeafAbsent -ParentLease $parentLease -Leaf $InstallRecord.DestinationLeaf -Label 'Rollback restoration destination'
            $backupLease = Open-InstallLeafLease -ParentLease $parentLease -Leaf $InstallRecord.BackupLeaf -DeleteAccess:$IsWindows
            if ($null -eq $backupLease) {
                throw "rollback backup is missing: $($InstallRecord.BackupLeaf)"
            }
            Invoke-InstallTestHook -TestHooks $TestHooks -Name 'before_restore_backup_raw' -Context $InstallRecord
            Assert-InstallFileSnapshot -Actual $backupLease.CaptureSnapshot() -Expected $InstallRecord.BackupIdentity -ExpectedHash $InstallRecord.BackupHash -Label 'Rollback backup immediately before restoration'
            $restoreEntry = Add-InstallLedgerEntry -Ledger $InstallRecord.PathBindingLedger -ParentLease $parentLease -Leaf $InstallRecord.DestinationLeaf -Role 'restored_original' -State 'restore_planned' -Reason 'relative no-replace restoration from immutable backup' -ExpectedSnapshot $InstallRecord.BackupIdentity
            if ($IsWindows) {
                Invoke-InstallRawRenameNoReplace -ParentLease $parentLease -SourceLease $backupLease -SourceLeaf $InstallRecord.BackupLeaf -TargetLeaf $InstallRecord.DestinationLeaf
            } else {
                $parentLease.RawLinkOpenFileNoReplace($backupLease, $InstallRecord.DestinationLeaf)
            }
            $InstallRecord.OriginalMoved = $false
            if ($IsWindows) {
                Set-InstallLedgerEntryState -Entry $InstallRecord.BackupEntry -State 'restored_from_raw' -ExpectedPresence Historical -Active $false -Reason 'backup moved back to public destination'
            } else {
                Set-InstallLedgerEntryState -Entry $InstallRecord.BackupEntry -State 'retained_after_restore' -ExpectedPresence Present -Active $true -Reason 'exact Linux rollback backup remains retained after link restoration'
            }
            Set-InstallLedgerEntryState -Entry $restoreEntry -State 'restore_raw_success' -ExpectedPresence Present -Active $true -Reason $(if ($IsWindows) { 'raw handle-bound no-replace restoration succeeded' } else { 'opened-backup-inode hardlink restoration succeeded' })
            if ($null -ne $InstallRecord.DestinationVacancyEntry) {
                Set-InstallLedgerEntryState -Entry $InstallRecord.DestinationVacancyEntry -State 'filled_by_restoration' -ExpectedPresence Historical -Active $false -Reason 'destination vacancy consumed by exact original restoration'
            }
            if ($IsWindows) {
                $null = Add-InstallLedgerEntry -Ledger $InstallRecord.PathBindingLedger -ParentLease $parentLease -Leaf $InstallRecord.BackupLeaf -Role 'backup_vacancy' -State 'restored_from_raw' -Reason 'backup leaf vacated by exact restoration' -ExpectedPresence Absent -Active $true
                $backupLease.Dispose()
                $backupLease = $null
            }
            Invoke-InstallTestHook -TestHooks $TestHooks -Name 'after_restore_backup_raw' -Context $InstallRecord
            $verifiedRestore = Confirm-InstallLedgerPresentEntry -ParentLease $parentLease -Entry $restoreEntry -ExpectedSnapshot $InstallRecord.BackupIdentity -ExpectedHash $InstallRecord.BackupHash -Label 'Restored original destination'
            $verifiedRestore.Dispose()
            if (-not $IsWindows) {
                Assert-InstallFileSnapshot -Actual $backupLease.CaptureSnapshot() -Expected $InstallRecord.BackupIdentity -ExpectedHash $InstallRecord.BackupHash -Label 'Retained backup inode after restoration'
            }
            Set-InstallLedgerEntryState -Entry $restoreEntry -State 'restored_verified' -ExpectedPresence Present -Active $true -Reason 'restored original reopened through parent lease and identity verified'
        } catch {
            $errors += "unable to restore immutable backup without replacing a concurrent destination: $($_.Exception.Message)"
            if ($null -ne $InstallRecord.BackupEntry -and
                $InstallRecord.BackupEntry.Active -and
                $InstallRecord.BackupEntry.ExpectedPresence -eq 'Present') {
                Set-InstallLedgerEntryState `
                    -Entry $InstallRecord.BackupEntry `
                    -State 'rollback_failed_retained' `
                    -ExpectedPresence Present `
                    -Active $true `
                    -Reason 'rollback restoration failed closed; the immutable backup remains retained for review'
            }
        } finally {
            if ($null -ne $backupLease) {
                $backupLease.Dispose()
            }
        }
    }

    if ($InstallRecord.StageActive) {
        $stageLease = $null
        try {
            $stageLease = Open-InstallLeafLease -ParentLease $parentLease -Leaf $InstallRecord.CurrentStageLeaf -DeleteAccess:$IsWindows
            if ($null -eq $stageLease) {
                throw "owned stage is missing: $($InstallRecord.CurrentStageLeaf)"
            }
            Assert-InstallFileSnapshot -Actual $stageLease.CaptureSnapshot() -Expected $InstallRecord.StageIdentity -ExpectedHash $InstallRecord.StageHash -Label 'Failed transaction stage'
            if ($IsWindows) {
                $stageLease.RawMarkDelete()
                $InstallRecord.StageActive = $false
                Set-InstallLedgerEntryState -Entry $InstallRecord.StageEntry -State 'delete_disposition_raw_success' -ExpectedPresence Historical -Active $false -Reason 'failed Windows stage handle marked for deletion'
                $null = Add-InstallLedgerEntry -Ledger $InstallRecord.PathBindingLedger -ParentLease $parentLease -Leaf $InstallRecord.CurrentStageLeaf -Role 'stage_vacancy' -State 'delete_disposition_raw_success' -Reason 'failed stage expected absent after handle close' -ExpectedPresence Absent -Active $true
                $stageLease.Dispose()
                $stageLease = $null
                Invoke-InstallTestHook -TestHooks $TestHooks -Name 'after_stage_cleanup_raw' -Context $InstallRecord
            } else {
                Set-InstallLedgerEntryState -Entry $InstallRecord.StageEntry -State 'retained_after_failure' -ExpectedPresence Present -Active $true -Reason 'Linux prepared stage is retained; no path unlink is permitted'
            }
        } catch {
            $errors += "unable to verify/isolate owned failed stage: $($_.Exception.Message)"
        } finally {
            if ($null -ne $stageLease) {
                $stageLease.Dispose()
            }
        }
    }

    $errors += @(Reconcile-InstallPathBindingLedger -Ledger $InstallRecord.PathBindingLedger -ParentLease $parentLease)
    if ($ReleaseParent) {
        try {
            $parentLease.Dispose()
            $InstallRecord.ParentLeaseDisposed = $true
        } catch {
            $errors += "unable to release terminal parent lease: $($_.Exception.Message)"
        }
    }
    return $errors
}

function Restore-InstalledFile {
    param(
        [Parameter(Mandatory = $true)]$InstallRecord,
        [AllowNull()]$TestHooks
    )

    $errors = @(Invoke-InstallRecordRecovery -InstallRecord $InstallRecord -TestHooks $TestHooks -ReleaseParent)
    if ($errors.Count -gt 0) {
        throw (New-InstallLedgerException -Message "Installation rollback was incomplete for $($InstallRecord.Destination)." -Ledger $InstallRecord.PathBindingLedger -RecoveryErrors $errors)
    }
}

function Invoke-InstallRollback {
    param([array]$InstallRecords)

    $rollback = @($InstallRecords)
    [array]::Reverse($rollback)
    $errors = @()
    foreach ($record in $rollback) {
        try {
            Restore-InstalledFile -InstallRecord $record
        } catch {
            $errors += "destination=$($record.Destination) backup=$($record.Backup): $($_.Exception.Message)"
        }
    }
    return $errors
}

function Remove-CommittedBackup {
    param(
        [Parameter(Mandatory = $true)]$InstallRecord,
        [AllowNull()]$TestHooks
    )

    $errors = @()
    $parentLease = $InstallRecord.ParentLease
    if ($InstallRecord.ParentLeaseDisposed -or $null -eq $parentLease) {
        return "Unable to finalize committed install: terminal parent lease was already disposed for $($InstallRecord.Destination)"
    }
    $destinationLease = $null
    $backupLease = $null
    try {
        $destinationLease = Open-InstallLeafLease -ParentLease $parentLease -Leaf $InstallRecord.DestinationLeaf
        if ($null -eq $destinationLease) {
            throw 'committed publication is missing; backup is retained'
        }
        Assert-InstallFileSnapshot -Actual $destinationLease.CaptureSnapshot() -Expected $InstallRecord.PublishedIdentity -ExpectedHash $InstallRecord.PublishedHash -Label 'Committed publication before cleanup'
        if ($InstallRecord.HadOriginal) {
            $backupLease = Open-InstallLeafLease -ParentLease $parentLease -Leaf $InstallRecord.BackupLeaf -DeleteAccess:$IsWindows
            if ($null -eq $backupLease) {
                throw 'committed backup is missing; cleanup state is incomplete'
            }
            Invoke-InstallTestHook -TestHooks $TestHooks -Name 'before_committed_backup_raw' -Context $InstallRecord
            Assert-InstallFileSnapshot -Actual $backupLease.CaptureSnapshot() -Expected $InstallRecord.BackupIdentity -ExpectedHash $InstallRecord.BackupHash -Label 'Committed backup immediately before raw cleanup'
            if ($IsWindows) {
                $backupLease.RawMarkDelete()
                Set-InstallLedgerEntryState -Entry $InstallRecord.BackupEntry -State 'delete_disposition_raw_success' -ExpectedPresence Historical -Active $false -Reason 'committed backup marked by one-byte identity-bound disposition'
                $null = Add-InstallLedgerEntry -Ledger $InstallRecord.PathBindingLedger -ParentLease $parentLease -Leaf $InstallRecord.BackupLeaf -Role 'backup_vacancy' -State 'delete_disposition_raw_success' -Reason 'committed backup expected absent after handle close' -ExpectedPresence Absent -Active $true
                $backupLease.Dispose()
                $backupLease = $null
                Invoke-InstallTestHook -TestHooks $TestHooks -Name 'after_committed_backup_raw' -Context $InstallRecord
            } else {
                Set-InstallLedgerEntryState -Entry $InstallRecord.BackupEntry -State 'committed_retained_backup' -ExpectedPresence Present -Active $true -Reason 'Linux keeps the exact identity-bound backup leaf; retained evidence is not auto-deleted'
                Invoke-InstallTestHook -TestHooks $TestHooks -Name 'after_committed_backup_raw' -Context $InstallRecord
            }
        }
    } catch {
        $cleanupFailure = $_.Exception.Message
        $errors += $cleanupFailure
        if ($null -ne $InstallRecord.BackupEntry -and
            $InstallRecord.BackupEntry.Active -and
            $InstallRecord.BackupEntry.ExpectedPresence -eq 'Present') {
            Set-InstallLedgerEntryState `
                -Entry $InstallRecord.BackupEntry `
                -State 'committed_cleanup_failed_retained' `
                -ExpectedPresence Present `
                -Active $true `
                -Reason "committed cleanup failed closed; backup remains retained: $cleanupFailure"
        }
    } finally {
        if ($null -ne $backupLease) {
            $backupLease.Dispose()
        }
        if ($null -ne $destinationLease) {
            $destinationLease.Dispose()
        }
    }

    $errors += @(Reconcile-InstallPathBindingLedger -Ledger $InstallRecord.PathBindingLedger -ParentLease $parentLease)
    try {
        $parentLease.Dispose()
        $InstallRecord.ParentLeaseDisposed = $true
    } catch {
        $errors += "unable to release terminal parent lease: $($_.Exception.Message)"
    }
    if ($errors.Count -gt 0) {
        return "Unable to finalize committed backup safely; all objects were preserved or ledgered: $($errors -join ' | ')"
    }
    if ($IsLinux) {
        $retainedCount = @(Get-RetainedInstallLedgerEntry -Ledger $InstallRecord.PathBindingLedger).Count
        return "Linux retained-evidence policy kept $retainedCount identity-bound path(s); retained evidence is not auto-deleted."
    }
    return $null
}

function Assert-InstallDirectorySnapshot {
    param(
        [Parameter(Mandatory = $true)]$Actual,
        [Parameter(Mandatory = $true)]$Expected,
        [Parameter(Mandatory = $true)][string]$Label
    )

    if ($Actual.PSObject.Properties.Name -contains 'VolumeSerialNumber') {
        if ($Actual.VolumeSerialNumber -ne $Expected.VolumeSerialNumber -or
            $Actual.FileIndex -ne $Expected.FileIndex -or
            $Actual.Attributes -ne $Expected.Attributes) {
            throw "$Label directory volume/file ID or attributes changed"
        }
        return
    }
    if ($Actual.DeviceId -ne $Expected.DeviceId -or
        $Actual.Inode -ne $Expected.Inode -or
        $Actual.Mode -ne $Expected.Mode) {
        throw "$Label directory device/inode/mode changed"
    }
}

function New-TemporaryDoctorWorkspace {
    param([string]$TemporaryParent)

    $tempRoot = if ([string]::IsNullOrWhiteSpace($TemporaryParent)) {
        Resolve-ExistingRealDirectory -Path ([IO.Path]::GetTempPath()) -Label 'Native system temporary root'
    } else {
        Resolve-ExistingRealDirectory -Path $TemporaryParent -Label 'Injected doctor temporary root'
    }
    $parentLease = New-InstallDirectoryLease -Path $tempRoot
    $nonce = [Guid]::NewGuid().ToString('N')
    $rootLeaf = "rayman-install-doctor-$PID-$nonce"
    $rootPath = Join-Path $tempRoot $rootLeaf
    $markerLeaf = '.rayman-install-doctor-owner'
    $markerContent = 'rayman-install-doctor-owner:' + $nonce + [Environment]::NewLine
    $record = [pscustomobject]@{
        Root = $rootPath
        TempRoot = $tempRoot
        OriginalRootLeaf = $rootLeaf
        CurrentRootLeaf = $rootLeaf
        ParentLease = $parentLease
        RootLease = $null
        RootIdentity = $null
        MarkerPath = Join-Path $rootPath $markerLeaf
        MarkerLeaf = $markerLeaf
        MarkerContent = $markerContent
        MarkerHash = $null
        MarkerIdentity = $null
        State = 'root_create_planned'
        RetainedRoot = $null
        ObservedRootHandlePath = $null
        TerminalErrors = @()
        DoctorLedger = [Collections.Generic.List[object]]::new()
    }
    $record.DoctorLedger.Add([pscustomobject]@{
        Path = $rootPath
        Leaf = $rootLeaf
        Role = 'doctor_root'
        State = 'create_planned'
        Reason = 'exclusive directory creation under held native temp parent'
        ExpectedIdentity = $null
        ObservedIdentity = $null
        VerificationStatus = 'not_verified'
    })
    try {
        if ($IsWindows) {
            [RaymanInstallerV2.WindowsDirectoryLease]::ValidateLeaf($rootLeaf)
            $rootLease = $parentLease.RawCreateDirectoryExclusive($rootLeaf)
            $record.RootLease = $rootLease
            $record.State = 'root_created_raw'
            $record.DoctorLedger[0].State = 'created_raw'
            $record.RootIdentity = $rootLease.CaptureInitialIdentity()
            $rootLease.AssertPathBinding()
        } else {
            [RaymanInstallerV2.LinuxDirectoryLease]::ValidateLeaf($rootLeaf)
            $parentLease.RawCreateDirectoryExclusive($rootLeaf)
            $record.State = 'root_created_raw'
            $record.DoctorLedger[0].State = 'created_raw'
            $rootLease = $parentLease.TryOpenDirectory($rootLeaf)
            if ($null -eq $rootLease) {
                throw 'doctor root disappeared after exclusive mkdirat'
            }
            $record.RootLease = $rootLease
            $record.RootIdentity = $rootLease.CaptureIdentity()
        }
        $record.DoctorLedger[0].ExpectedIdentity = if ($IsWindows) {
            'volume=0x{0:x8};file=0x{1:x16}' -f $record.RootIdentity.VolumeSerialNumber, $record.RootIdentity.FileIndex
        } else {
            'device=0x{0:x16};inode={1}' -f $record.RootIdentity.DeviceId, $record.RootIdentity.Inode
        }
        $record.DoctorLedger[0].ObservedIdentity = $record.DoctorLedger[0].ExpectedIdentity
        $record.DoctorLedger[0].State = 'created_verified'
        $record.DoctorLedger[0].VerificationStatus = 'verified'

        $markerEntry = Add-InstallLedgerEntry -Ledger $record.DoctorLedger -ParentLease $rootLease -Leaf $markerLeaf -Role 'doctor_owner_marker' -State 'create_planned' -Reason 'exclusive owner marker creation'
        $markerLease = $null
        try {
            $markerLease = $rootLease.RawCreateFileExclusive($markerLeaf)
            Set-InstallLedgerEntryState -Entry $markerEntry -State 'created_raw' -ExpectedPresence Present -Active $true -Reason 'exclusive owner marker leaf created'
            $markerBytes = [Text.UTF8Encoding]::new($false).GetBytes($markerContent)
            $markerLease.WriteAllBytes($markerBytes)
            $markerIdentity = $markerLease.CaptureSnapshot()
            $markerHash = [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData($markerBytes)).ToLowerInvariant()
            Assert-InstallFileSnapshot -Actual $markerIdentity -Expected $markerIdentity -ExpectedHash $markerHash -Label 'Doctor owner marker'
            $record.MarkerIdentity = $markerIdentity
            $record.MarkerHash = $markerHash
            Set-InstallLedgerSnapshot -Entry $markerEntry -Snapshot $markerIdentity -Kind Expected
            Set-InstallLedgerSnapshot -Entry $markerEntry -Snapshot $markerIdentity -Kind Observed
            Set-InstallLedgerEntryState -Entry $markerEntry -State 'marker_verified' -ExpectedPresence Present -Active $true -Reason 'marker bytes flushed and handle identity captured'
            $markerEntry.VerificationStatus = 'verified'
        } finally {
            if ($null -ne $markerLease) {
                $markerLease.Dispose()
            }
        }

        $stateLeaf = '.RaymanCodingSkill'
        if ($IsWindows) {
            $stateLease = $rootLease.RawCreateDirectoryExclusive($stateLeaf)
            $record.DoctorLedger.Add([pscustomobject]@{
                Path = Join-Path $rootPath $stateLeaf
                Leaf = $stateLeaf
                Role = 'doctor_state_root'
                State = 'created_raw'
                Reason = 'exclusive doctor state directory creation'
                ExpectedIdentity = $null
                ObservedIdentity = $null
                VerificationStatus = 'not_verified'
            })
            try {
                $stateIdentity = $stateLease.CaptureInitialIdentity()
                $stateLease.AssertPathBinding()
                $record.DoctorLedger[$record.DoctorLedger.Count - 1].ExpectedIdentity = 'volume=0x{0:x8};file=0x{1:x16}' -f $stateIdentity.VolumeSerialNumber, $stateIdentity.FileIndex
                $record.DoctorLedger[$record.DoctorLedger.Count - 1].ObservedIdentity = $record.DoctorLedger[$record.DoctorLedger.Count - 1].ExpectedIdentity
                $record.DoctorLedger[$record.DoctorLedger.Count - 1].State = 'created_verified'
                $record.DoctorLedger[$record.DoctorLedger.Count - 1].VerificationStatus = 'verified'
            } finally {
                $stateLease.Dispose()
            }
        } else {
            $rootLease.RawCreateDirectoryExclusive($stateLeaf)
            $record.DoctorLedger.Add([pscustomobject]@{
                Path = Join-Path $rootPath $stateLeaf
                Leaf = $stateLeaf
                Role = 'doctor_state_root'
                State = 'created_raw'
                Reason = 'exclusive doctor state directory creation'
                ExpectedIdentity = $null
                ObservedIdentity = $null
                VerificationStatus = 'raw_success'
            })
            $stateLease = $rootLease.TryOpenDirectory($stateLeaf)
            if ($null -eq $stateLease) {
                throw 'doctor state root disappeared after mkdirat'
            }
            try {
                $stateIdentity = $stateLease.CaptureIdentity()
                $record.DoctorLedger[$record.DoctorLedger.Count - 1].ExpectedIdentity = 'device=0x{0:x16};inode={1}' -f $stateIdentity.DeviceId, $stateIdentity.Inode
                $record.DoctorLedger[$record.DoctorLedger.Count - 1].ObservedIdentity = $record.DoctorLedger[$record.DoctorLedger.Count - 1].ExpectedIdentity
                $record.DoctorLedger[$record.DoctorLedger.Count - 1].State = 'created_verified'
                $record.DoctorLedger[$record.DoctorLedger.Count - 1].VerificationStatus = 'verified'
            } finally {
                $stateLease.Dispose()
            }
        }
        $record.State = 'ready'
        return $record
    } catch {
        $failure = $_.Exception.Message
        $warning = Remove-TemporaryDoctorWorkspace -Record $record
        throw "Unable to create temporary doctor workspace: $failure. $warning"
    }
}

function Remove-TemporaryDoctorWorkspace {
    param(
        [Parameter(Mandatory = $true)]$Record,
        [AllowNull()]$TestHooks
    )

    if ($Record.State -eq 'retained_leases_released') {
        return "Temporary doctor workspace remains retained after completed isolation; leases were released and no retry was attempted: $($Record.RetainedRoot)"
    }
    if ($Record.State -eq 'failed_leases_released') {
        $preserved = @($Record.RetainedRoot, $Record.ObservedRootHandlePath, $Record.Root) |
            Where-Object { -not [string]::IsNullOrWhiteSpace([string]$_) } |
            Select-Object -Unique
        $priorErrors = @($Record.TerminalErrors)
        $errorSuffix = if ($priorErrors.Count -gt 0) {
            " Prior terminal errors: $($priorErrors -join ' | ')"
        } else {
            ''
        }
        return "Temporary doctor workspace remains preserved after a prior failed isolation; leases were released and no retry was attempted: $($preserved -join ', ').$errorSuffix"
    }
    $errors = @()
    $parentLease = $Record.ParentLease
    $rootLease = $Record.RootLease
    $retainedLeaf = "$($Record.OriginalRootLeaf).retained-$([Guid]::NewGuid().ToString('N'))"
    $retainedPath = Join-Path $Record.TempRoot $retainedLeaf
    $retainedDirectoryLease = $null
    $markerLease = $null
    try {
        if ($null -eq $parentLease) {
            throw 'doctor temp-parent lease was never acquired'
        }
        $parentLease.AssertPathBinding()
        if ($null -eq $rootLease -or $null -eq $Record.RootIdentity) {
            throw 'doctor root was created but its directory handle identity was not captured'
        }
        Assert-InstallDirectorySnapshot -Actual $rootLease.CaptureIdentity() -Expected $Record.RootIdentity -Label 'Doctor root before retention'
        $markerLease = Open-InstallLeafLease -ParentLease $rootLease -Leaf $Record.MarkerLeaf
        if ($null -eq $markerLease) {
            throw 'doctor owner marker is missing'
        }
        Assert-InstallFileSnapshot -Actual $markerLease.CaptureSnapshot() -Expected $Record.MarkerIdentity -ExpectedHash $Record.MarkerHash -Label 'Doctor owner marker before retention'
        $markerLease.Dispose()
        $markerLease = $null

        Invoke-InstallTestHook -TestHooks $TestHooks -Name 'before_doctor_isolate_raw' -Context $Record
        $namedRootLease = Open-InstallDirectoryLeafLease `
            -ParentLease $parentLease `
            -Leaf $Record.CurrentRootLeaf
        if ($null -eq $namedRootLease) {
            throw 'named doctor root is missing before retained isolation'
        }
        try {
            Assert-InstallDirectorySnapshot -Actual $namedRootLease.CaptureIdentity() -Expected $Record.RootIdentity -Label 'Named doctor root before retained isolation'
        } finally {
            $namedRootLease.Dispose()
        }
        Invoke-InstallTestHook -TestHooks $TestHooks -Name 'after_doctor_source_verify_before_raw' -Context $Record
        if ($IsWindows) {
            [RaymanInstallerV2.WindowsDirectoryLease]::ValidateLeaf($retainedLeaf)
            $rootLease.RawRenameNoReplaceTo($parentLease, $retainedLeaf)
        } else {
            [RaymanInstallerV2.LinuxDirectoryLease]::ValidateLeaf($retainedLeaf)
            $parentLease.RawRenameNoReplace($Record.CurrentRootLeaf, $parentLease, $retainedLeaf)
        }
        $Record.CurrentRootLeaf = $retainedLeaf
        $Record.RetainedRoot = $retainedPath
        $Record.State = 'isolation_raw_success'
        $Record.DoctorLedger[0].State = 'isolation_raw_success'
        $Record.DoctorLedger[0].Path = $retainedPath
        $Record.DoctorLedger[0].Reason = 'doctor root retained by relative no-replace rename; recursive deletion is forbidden'
        $Record.MarkerPath = Join-Path $retainedPath $Record.MarkerLeaf
        foreach ($entry in @($Record.DoctorLedger | Select-Object -Skip 1)) {
            $entry.Path = Join-Path $retainedPath $entry.Leaf
        }
        $rootLease.AcceptRenamedPath($retainedPath)
        Invoke-InstallTestHook -TestHooks $TestHooks -Name 'after_doctor_isolate_raw' -Context $Record

        $retainedDirectoryLease = Open-InstallDirectoryLeafLease `
            -ParentLease $parentLease `
            -Leaf $retainedLeaf
        if ($null -eq $retainedDirectoryLease) {
            throw 'retained doctor root is missing after raw isolation'
        }
        Assert-InstallDirectorySnapshot -Actual $retainedDirectoryLease.CaptureIdentity() -Expected $Record.RootIdentity -Label 'Retained doctor root'
        Assert-InstallDirectorySnapshot -Actual $rootLease.CaptureIdentity() -Expected $Record.RootIdentity -Label 'Held doctor root after retained isolation'
        $rootLease.AssertPathBinding()
        $markerLease = Open-InstallLeafLease -ParentLease $rootLease -Leaf $Record.MarkerLeaf
        if ($null -eq $markerLease) {
            throw 'retained doctor owner marker is missing'
        }
        Assert-InstallFileSnapshot -Actual $markerLease.CaptureSnapshot() -Expected $Record.MarkerIdentity -ExpectedHash $Record.MarkerHash -Label 'Retained doctor owner marker'
        $Record.ObservedRootHandlePath = $rootLease.GetCurrentPath()
        $Record.DoctorLedger[0].ObservedIdentity = $Record.DoctorLedger[0].ExpectedIdentity
        $Record.DoctorLedger[0].State = 'retained_verified'
        $Record.DoctorLedger[0].VerificationStatus = 'verified'
        $Record.State = 'retained_verified'
        $parentLease.AssertPathBinding()
    } catch {
        $errors += $_.Exception.Message
        try {
            if ($null -ne $rootLease) {
                $Record.ObservedRootHandlePath = $rootLease.GetCurrentPath()
            }
        } catch {
            $errors += "unable to resolve held doctor root path: $($_.Exception.Message)"
        }
    } finally {
        if ($null -ne $markerLease) {
            $markerLease.Dispose()
        }
        if ($null -ne $retainedDirectoryLease) {
            $retainedDirectoryLease.Dispose()
        }
        if ($null -ne $rootLease) {
            $rootLease.Dispose()
        }
        if ($null -ne $parentLease) {
            $parentLease.Dispose()
        }
        $Record.TerminalErrors = @($errors)
        $Record.State = if ($errors.Count -eq 0 -and
            -not [string]::IsNullOrWhiteSpace([string]$Record.RetainedRoot)) {
            'retained_leases_released'
        } else {
            'failed_leases_released'
        }
    }

    $reported = @(
        @($Record.RetainedRoot, $Record.ObservedRootHandlePath) |
            Where-Object { -not [string]::IsNullOrWhiteSpace([string]$_) } |
            Select-Object -Unique
    )
    if ($reported.Count -eq 0) {
        $reported = @($Record.Root)
    }
    $suffix = if ($errors.Count -gt 0) {
        " Terminal verification reported: $($errors -join ' | ')"
    } else {
        ' Marker, root identity, and parent binding were terminally verified.'
    }
    return "Temporary doctor workspace retained for review; no recursive deletion was attempted: $($reported -join ', ').$suffix"
}

function Invoke-InstallNamedSelfTest {
    param(
        [Parameter(Mandatory = $true)][string]$Name,
        [Parameter(Mandatory = $true)][scriptblock]$Body,
        [AllowNull()]$Context,
        [string]$SkipReason
    )

    if (-not [string]::IsNullOrWhiteSpace($SkipReason)) {
        Write-Host "SKIP installer_selftest::$Name - $SkipReason"
        return
    }
    try {
        & $Body $Context
        Write-Host "PASS installer_selftest::$Name"
    } catch {
        throw "FAIL installer_selftest::$Name - $($_.Exception.Message)"
    }
}

function Close-InstallSelfTestRecord {
    param([AllowNull()]$InstallRecord)

    if ($null -eq $InstallRecord -or
        $InstallRecord.ParentLeaseDisposed -or
        $null -eq $InstallRecord.ParentLease) {
        return
    }
    $errors = @(Invoke-InstallRecordRecovery -InstallRecord $InstallRecord -ReleaseParent)
    if ($errors.Count -gt 0) {
        Write-Warning "Self-test recovery retained terminal evidence: $($errors -join ' | ')" -WarningAction Continue
    }
}

function Write-InstallSelfTestFile {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$Content
    )

    [IO.File]::WriteAllText($Path, $Content, [Text.UTF8Encoding]::new($false))
}

function New-InstallSelfTestCase {
    param(
        [Parameter(Mandatory = $true)][string]$TestRoot,
        [Parameter(Mandatory = $true)][string]$Name,
        [Parameter(Mandatory = $true)][string]$SourceContent,
        [switch]$WithOriginal,
        [string]$OriginalContent = 'old-data'
    )

    $source = Join-Path $TestRoot "$Name-source.bin"
    $destination = Join-Path $TestRoot "$Name-destination.bin"
    Write-InstallSelfTestFile -Path $source -Content $SourceContent
    if ($WithOriginal) {
        Write-InstallSelfTestFile -Path $destination -Content $OriginalContent
    }
    return [pscustomobject]@{
        Name = $Name
        Source = $source
        Destination = $destination
        SourceContent = $SourceContent
        OriginalContent = $(if ($WithOriginal) { $OriginalContent } else { $null })
        Nonce = "$Name-$([Guid]::NewGuid().ToString('N'))"
        ExpectedHash = (Get-FileHash -LiteralPath $source -Algorithm SHA256).Hash.ToLowerInvariant()
    }
}

function Assert-InstallSelfTestContent {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$Expected,
        [Parameter(Mandatory = $true)][string]$Label
    )

    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        throw "$Label is missing: $Path"
    }
    $actual = [IO.File]::ReadAllText($Path, [Text.Encoding]::UTF8)
    if (-not [string]::Equals($actual, $Expected, [StringComparison]::Ordinal)) {
        throw "$Label content changed: expected='$Expected' actual='$actual'"
    }
}

function Get-InstallSelfTestFailureLedger {
    param([Parameter(Mandatory = $true)][Exception]$Exception)

    if (-not $Exception.Data.Contains('RaymanInstallPathBindingLedger')) {
        throw "installer failure did not publish its incremental path-binding ledger: $($Exception.Message)"
    }
    return @($Exception.Data['RaymanInstallPathBindingLedger'])
}

function Assert-InstallSelfTestRetainedEntries {
    param(
        [Parameter(Mandatory = $true)]$Ledger,
        [Parameter(Mandatory = $true)][string]$Label,
        [int]$MinimumCount = 1
    )

    $retained = @(Get-RetainedInstallLedgerEntry -Ledger $Ledger)
    if ($retained.Count -lt $MinimumCount) {
        throw "$Label did not retain the expected identity-bound evidence: count=$($retained.Count)"
    }
    foreach ($entry in $retained) {
        $path = if ([string]::IsNullOrWhiteSpace([string]$entry.ObservedPath)) {
            $entry.Path
        } else {
            $entry.ObservedPath
        }
        if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
            throw "$Label retained ledger path is missing: role=$($entry.Role) state=$($entry.State) path=$path"
        }
        if ($entry.VerificationStatus -notin @('verified', 'created_handle_verified')) {
            throw "$Label retained ledger path is not terminally verified: role=$($entry.Role) state=$($entry.State) verification=$($entry.VerificationStatus)"
        }
    }
    return @($retained)
}

function Assert-InstallSelfTestBackupReportedRetained {
    param(
        [Parameter(Mandatory = $true)]$InstallRecord,
        [Parameter(Mandatory = $true)][string]$Label,
        [string[]]$ExpectedStates = @(
            'backup_link_raw_success',
            'retained_after_restore',
            'rollback_failed_retained',
            'committed_retained_backup',
            'committed_cleanup_failed_retained'
        )
    )

    $entries = @(Get-RetainedInstallLedgerEntry -Ledger $InstallRecord.PathBindingLedger | Where-Object {
        $_.Role -eq 'rollback_backup' -and
        $_.Leaf -eq $InstallRecord.BackupLeaf -and
        $_.State -in $ExpectedStates
    })
    if ($entries.Count -ne 1) {
        throw "$Label did not classify the rollback backup as retained: count=$($entries.Count)"
    }
    $path = if ([string]::IsNullOrWhiteSpace([string]$entries[0].ObservedPath)) {
        $entries[0].Path
    } else {
        $entries[0].ObservedPath
    }
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
        throw "$Label reported a retained rollback backup that is not present: $path"
    }
    return $entries[0]
}

function Invoke-InstallPathSelfTest {
    $managedTemp = Resolve-ManagedDirectory `
        -Path (Join-Path $repoRoot '.RaymanCodingSkill/tmp') `
        -Label 'Managed self-test temp'
    $testRoot = Join-Path $managedTemp ("install-path-selftest-" + [Guid]::NewGuid().ToString('N'))
    $inside = Join-Path $testRoot 'inside'
    $outside = Join-Path $testRoot 'outside'
    $link = Join-Path $inside 'escape'
    $null = Resolve-ManagedDirectory -Path $inside -Label 'Self-test inside'
    $null = Resolve-ManagedDirectory -Path $outside -Label 'Self-test outside'
    $context = [pscustomobject]@{
        TestRoot = $testRoot
        Inside = $inside
        Outside = $outside
        EscapeLink = $link
    }

    try {
        Invoke-InstallNamedSelfTest -Name 'manifest_resource_plan' -Context $context -Body {
            param($ctx)
            $resourcePlan = @(Get-CodexSkillResourcePlan -DestinationRoot $ctx.Inside)
            $resourceDestinations = @($resourcePlan.DestinationRelative | Sort-Object)
            if ($resourcePlan.Count -ne 3 -or
                $resourceDestinations -contains 'CLAUDE.md' -or
                $resourceDestinations -notcontains 'SKILL.md' -or
                $resourceDestinations -notcontains 'references/workflow-contract.md') {
                throw 'the manifest did not produce the exact Codex resource plan'
            }
            $selected = Get-CanonicalSkillResource -ResourcePlan $resourcePlan
            $expected = @($resourcePlan | Where-Object DestinationRelative -eq 'SKILL.md')[0]
            foreach ($property in @('Source', 'Destination')) {
                if (-not [string]::Equals(
                        [string]$selected.$property,
                        [string]$expected.$property,
                        [StringComparison]::Ordinal
                    )) {
                    throw "canonical SKILL.md $property selection drifted"
                }
            }
        }

        Invoke-InstallNamedSelfTest -Name 'source_hash_drift_rejected' -Context $context -Body {
            param($ctx)
            $probe = Join-Path $ctx.TestRoot 'hash-probe.bin'
            Write-InstallSelfTestFile -Path $probe -Content 'verified'
            $expectedHash = (Get-FileHash -LiteralPath $probe -Algorithm SHA256).Hash.ToLowerInvariant()
            Write-InstallSelfTestFile -Path $probe -Content 'drifted'
            $rejected = $false
            try {
                Assert-ExpectedFileHash -Path $probe -ExpectedHash $expectedHash -Label 'Self-test source'
            } catch {
                $rejected = $_.Exception.Message -match 'hash drifted'
            }
            if (-not $rejected) {
                throw 'verified source hash drift was not rejected'
            }
        }

        Invoke-InstallNamedSelfTest `
            -Name 'windows_file_disposition_info_u1' `
            -SkipReason $(if ($IsWindows) { $null } else { 'Windows runtime ABI test' }) `
            -Body {
                param($unused)
                Initialize-HandleCasNative
                $size = [RaymanInstallerV2.WindowsFileLease]::FileDispositionInfoSize
                if ($size -ne 1) {
                    throw "FILE_DISPOSITION_INFO must marshal to one byte; observed=$size"
                }
            }

        Invoke-InstallNamedSelfTest `
            -Name 'linux_statx_zeroed_mask_contract' `
            -SkipReason $(if ($IsLinux) { $null } else { 'Linux runtime ABI test' }) `
            -Body {
                param($unused)
                Initialize-LinuxHandleCasNative
                $size = [RaymanInstallerV2.LinuxDirectoryLease]::StatxBufferSizeForSelfTest
                if ($size -ne 256) {
                    throw "statx ABI buffer must be 256 bytes; observed=$size"
                }
                $mask = [RaymanInstallerV2.LinuxDirectoryLease]::RequiredStatxMaskForSelfTest
                [RaymanInstallerV2.LinuxDirectoryLease]::ValidateStatxMaskForSelfTest($mask)
                [RaymanInstallerV2.LinuxDirectoryLease]::ValidateStatxMaskForSelfTest([uint32]($mask -bor [uint32]0x80000000))
                $missingMode = [uint32]($mask -band [uint32]0xfffffffd)
                $rejected = $false
                try {
                    [RaymanInstallerV2.LinuxDirectoryLease]::ValidateStatxMaskForSelfTest($missingMode)
                } catch {
                    $rejected = $_.Exception.Message -match 'incomplete mask'
                }
                if (-not $rejected) {
                    throw 'statx accepted a result missing a mandatory mask bit'
                }
            }
        Invoke-InstallNamedSelfTest -Name 'exclusive_stage_collision' -Context $context -Body {
            param($ctx)
            $case = New-InstallSelfTestCase -TestRoot $ctx.TestRoot -Name 'stage-collision' -SourceContent 'new-stage-collision'
            $stagePath = "$($case.Destination).install-$($case.Nonce)"
            Write-InstallSelfTestFile -Path $stagePath -Content 'concurrent-stage-owner'
            $failure = $null
            try {
                $null = Install-FileWithRollback `
                    -Source $case.Source `
                    -Destination $case.Destination `
                    -Nonce $case.Nonce `
                    -ExpectedHash $case.ExpectedHash
            } catch {
                $failure = $_
            }
            if ($null -eq $failure) {
                throw 'exclusive stage collision was accepted'
            }
            $ledger = @(Get-InstallSelfTestFailureLedger -Exception $failure.Exception)
            $collision = @($ledger | Where-Object {
                $_.Role -eq 'prepared_stage' -and $_.State -eq 'exclusive_create_rejected'
            })
            if ($collision.Count -ne 1 -or (Test-Path -LiteralPath $case.Destination)) {
                throw 'exclusive stage collision did not fail before publication'
            }
            Assert-InstallSelfTestContent -Path $stagePath -Expected 'concurrent-stage-owner' -Label 'Concurrent stage owner'
        }

        Invoke-InstallNamedSelfTest -Name 'terminal_parent_binding_rejected' -Context $context -Body {
            param($ctx)
            $caseRoot = Join-Path $ctx.TestRoot 'parent-binding-root'
            $heldRoot = "$caseRoot-held"
            $null = Resolve-ManagedDirectory -Path $caseRoot -Label 'Parent-binding self-test root'
            $source = Join-Path $ctx.TestRoot 'parent-binding-source.bin'
            $destination = Join-Path $caseRoot 'rayman.bin'
            Write-InstallSelfTestFile -Path $source -Content 'parent-binding-source'
            $expectedHash = (Get-FileHash -LiteralPath $source -Algorithm SHA256).Hash.ToLowerInvariant()
            $hooks = @{
                after_parent_lease = {
                    param($record)
                    [IO.Directory]::Move($record.DestinationDirectory, "$($record.DestinationDirectory)-held")
                    [IO.Directory]::CreateDirectory($record.DestinationDirectory) | Out-Null
                }
            }
            $failure = $null
            try {
                $null = Install-FileWithRollback `
                    -Source $source `
                    -Destination $destination `
                    -Nonce "parent-binding-$([Guid]::NewGuid().ToString('N'))" `
                    -ExpectedHash $expectedHash `
                    -TestHooks $hooks
            } catch {
                $failure = $_
            }
            if ($null -eq $failure -or (Test-Path -LiteralPath $destination)) {
                throw 'held terminal parent rebinding was not rejected before mutation'
            }
            if ($IsWindows) {
                if (-not (Test-Path -LiteralPath $caseRoot -PathType Container) -or
                    (Test-Path -LiteralPath $heldRoot)) {
                    throw 'Windows terminal parent lease did not block directory rebinding'
                }
            } elseif ($failure.Exception.Message -notmatch 'binding|path' -or
                -not (Test-Path -LiteralPath $caseRoot -PathType Container) -or
                -not (Test-Path -LiteralPath $heldRoot -PathType Container)) {
                throw 'Linux terminal parent binding drift was not detected after directory rebinding'
            }
        }

        Invoke-InstallNamedSelfTest -Name 'backup_raw_failure' -Context $context -Body {
            param($ctx)
            $case = New-InstallSelfTestCase `
                -TestRoot $ctx.TestRoot `
                -Name 'backup-raw-failure' `
                -SourceContent 'new-backup-failure' `
                -WithOriginal `
                -OriginalContent 'old-backup-failure'
            $hooks = @{
                before_backup_rename_raw = {
                    param($record)
                    throw 'self-test injected backup raw failure'
                }
            }
            $failure = $null
            try {
                $null = Install-FileWithRollback `
                    -Source $case.Source `
                    -Destination $case.Destination `
                    -Nonce $case.Nonce `
                    -ExpectedHash $case.ExpectedHash `
                    -TestHooks $hooks
            } catch {
                $failure = $_
            }
            if ($null -eq $failure -or $failure.Exception.Message -notmatch 'injected backup raw failure') {
                throw 'backup raw failure was not propagated'
            }
            Assert-InstallSelfTestContent -Path $case.Destination -Expected 'old-backup-failure' -Label 'Original destination after backup failure'
            if (Test-Path -LiteralPath "$($case.Destination).backup-$($case.Nonce)") {
                throw 'backup raw failure published an unverified backup'
            }
            $ledger = @(Get-InstallSelfTestFailureLedger -Exception $failure.Exception)
            if ($IsWindows) {
                if (Test-Path -LiteralPath "$($case.Destination).install-$($case.Nonce)") {
                    throw 'Windows failed stage was not removed by identity-bound disposition'
                }
            } else {
                $null = Assert-InstallSelfTestRetainedEntries -Ledger $ledger -Label 'Linux backup-failure recovery' -MinimumCount 2
            }
        }

        Invoke-InstallNamedSelfTest `
            -Name 'linux_preflight_link_raw_failure_retained' `
            -Context $context `
            -SkipReason $(if ($IsLinux) { $null } else { 'Linux raw-success retained classification' }) `
            -Body {
                param($ctx)
                $case = New-InstallSelfTestCase `
                    -TestRoot $ctx.TestRoot `
                    -Name 'linux-preflight-link-failure' `
                    -SourceContent 'linux-preflight-link-source'
                $hooks = @{
                    after_linux_preflight_link_raw = {
                        param($record)
                        throw 'self-test injected Linux preflight-link post-raw failure'
                    }
                }
                $failure = $null
                try {
                    $null = Install-FileWithRollback `
                        -Source $case.Source `
                        -Destination $case.Destination `
                        -Nonce $case.Nonce `
                        -ExpectedHash $case.ExpectedHash `
                        -TestHooks $hooks
                } catch {
                    $failure = $_
                }
                if ($null -eq $failure -or $failure.Exception.Message -notmatch 'preflight-link post-raw failure') {
                    throw 'Linux preflight-link raw-success failure was not propagated'
                }
                $ledger = @(Get-InstallSelfTestFailureLedger -Exception $failure.Exception)
                $null = Assert-InstallSelfTestRetainedEntries `
                    -Ledger $ledger `
                    -Label 'Linux preflight-link raw failure' `
                    -MinimumCount 2
                if (@($ledger | Where-Object {
                    "$($_.Role)|$($_.State)" -eq 'preflight_retained_evidence|hardlink_preflight_raw_success'
                }).Count -ne 1 -or (Test-Path -LiteralPath $case.Destination)) {
                    throw 'Linux raw-created preflight evidence was not classified and preserved'
                }
            }

        Invoke-InstallNamedSelfTest `
            -Name 'linux_backup_link_raw_failure_retained' `
            -Context $context `
            -SkipReason $(if ($IsLinux) { $null } else { 'Linux raw-success retained classification' }) `
            -Body {
                param($ctx)
                $case = New-InstallSelfTestCase `
                    -TestRoot $ctx.TestRoot `
                    -Name 'linux-backup-link-failure' `
                    -SourceContent 'linux-backup-link-source' `
                    -WithOriginal `
                    -OriginalContent 'linux-backup-link-old'
                $hooks = @{
                    after_backup_link_raw = {
                        param($record)
                        throw 'self-test injected Linux backup-link post-raw failure'
                    }
                }
                $failure = $null
                try {
                    $null = Install-FileWithRollback `
                        -Source $case.Source `
                        -Destination $case.Destination `
                        -Nonce $case.Nonce `
                        -ExpectedHash $case.ExpectedHash `
                        -TestHooks $hooks
                } catch {
                    $failure = $_
                }
                if ($null -eq $failure -or $failure.Exception.Message -notmatch 'backup-link post-raw failure') {
                    throw 'Linux backup-link raw-success failure was not propagated'
                }
                $ledger = @(Get-InstallSelfTestFailureLedger -Exception $failure.Exception)
                $null = Assert-InstallSelfTestRetainedEntries `
                    -Ledger $ledger `
                    -Label 'Linux backup-link raw failure' `
                    -MinimumCount 3
                Assert-InstallSelfTestContent -Path $case.Destination -Expected 'linux-backup-link-old' -Label 'Linux original after backup-link hook failure'
                $backupEntries = @($ledger | Where-Object {
                    "$($_.Role)|$($_.State)" -eq 'rollback_backup|backup_link_raw_success'
                })
                if ($backupEntries.Count -ne 1) {
                    throw 'Linux raw-created rollback backup was not classified as retained'
                }
                Assert-InstallSelfTestContent -Path $backupEntries[0].ObservedPath -Expected 'linux-backup-link-old' -Label 'Linux raw-created rollback backup'
            }

        Invoke-InstallNamedSelfTest `
            -Name 'linux_original_isolation_raw_failure_retained' `
            -Context $context `
            -SkipReason $(if ($IsLinux) { $null } else { 'Linux raw-success retained classification' }) `
            -Body {
                param($ctx)
                $case = New-InstallSelfTestCase `
                    -TestRoot $ctx.TestRoot `
                    -Name 'linux-original-isolation-failure' `
                    -SourceContent 'linux-original-isolation-source' `
                    -WithOriginal `
                    -OriginalContent 'linux-original-isolation-old'
                $hooks = @{
                    after_backup_rename_raw = {
                        param($record)
                        throw 'self-test injected Linux original-isolation post-raw failure'
                    }
                }
                $failure = $null
                try {
                    $null = Install-FileWithRollback `
                        -Source $case.Source `
                        -Destination $case.Destination `
                        -Nonce $case.Nonce `
                        -ExpectedHash $case.ExpectedHash `
                        -TestHooks $hooks
                } catch {
                    $failure = $_
                }
                if ($null -eq $failure -or $failure.Exception.Message -notmatch 'original-isolation post-raw failure') {
                    throw 'Linux original-isolation raw-success failure was not propagated'
                }
                $ledger = @(Get-InstallSelfTestFailureLedger -Exception $failure.Exception)
                $null = Assert-InstallSelfTestRetainedEntries `
                    -Ledger $ledger `
                    -Label 'Linux original-isolation raw failure' `
                    -MinimumCount 4
                if (@($ledger | Where-Object {
                    "$($_.Role)|$($_.State)" -eq 'retained_original_destination|isolation_raw_success'
                }).Count -ne 1) {
                    throw 'Linux raw-isolated original was not classified as retained after hook failure'
                }
                Assert-InstallSelfTestContent -Path $case.Destination -Expected 'linux-original-isolation-old' -Label 'Linux restored original after isolation hook failure'
            }

        Invoke-InstallNamedSelfTest -Name 'publication_no_replace_race' -Context $context -Body {
            param($ctx)
            $case = New-InstallSelfTestCase `
                -TestRoot $ctx.TestRoot `
                -Name 'publication-race' `
                -SourceContent 'new-publication-race' `
                -WithOriginal `
                -OriginalContent 'old-publication-race'
            $hooks = @{
                before_publication_rename_raw = {
                    param($record)
                    Write-InstallSelfTestFile -Path $record.Destination -Content 'concurrent-publication-winner'
                }
            }
            $failure = $null
            try {
                $null = Install-FileWithRollback `
                    -Source $case.Source `
                    -Destination $case.Destination `
                    -Nonce $case.Nonce `
                    -ExpectedHash $case.ExpectedHash `
                    -TestHooks $hooks
            } catch {
                $failure = $_
            }
            if ($null -eq $failure) {
                throw 'publication no-replace race was accepted'
            }
            Assert-InstallSelfTestContent -Path $case.Destination -Expected 'concurrent-publication-winner' -Label 'Concurrent publication winner'
            $backup = "$($case.Destination).backup-$($case.Nonce)"
            Assert-InstallSelfTestContent -Path $backup -Expected 'old-publication-race' -Label 'Rollback backup after publication race'
            $ledger = @(Get-InstallSelfTestFailureLedger -Exception $failure.Exception)
            if ($IsLinux) {
                $null = Assert-InstallSelfTestRetainedEntries -Ledger $ledger -Label 'Linux publication-race recovery' -MinimumCount 3
            }
        }

        Invoke-InstallNamedSelfTest -Name 'first_install_rollback' -Context $context -Body {
            param($ctx)
            $case = New-InstallSelfTestCase -TestRoot $ctx.TestRoot -Name 'first-rollback' -SourceContent 'new-first-install'
            $record = $null
            try {
                $record = Install-FileWithRollback `
                    -Source $case.Source `
                    -Destination $case.Destination `
                    -Nonce $case.Nonce `
                    -ExpectedHash $case.ExpectedHash
                Restore-InstalledFile -InstallRecord $record
                if (Test-Path -LiteralPath $case.Destination) {
                    throw 'first-install rollback left a public destination'
                }
                if ($IsLinux) {
                    $null = Assert-InstallSelfTestRetainedEntries -Ledger $record.PathBindingLedger -Label 'Linux first-install rollback' -MinimumCount 3
                }
            } finally {
                Close-InstallSelfTestRecord -InstallRecord $record
            }
        }

        Invoke-InstallNamedSelfTest -Name 'upgrade_rollback' -Context $context -Body {
            param($ctx)
            $case = New-InstallSelfTestCase `
                -TestRoot $ctx.TestRoot `
                -Name 'upgrade-rollback' `
                -SourceContent 'new-upgrade-install' `
                -WithOriginal `
                -OriginalContent 'old-upgrade-install'
            $record = $null
            try {
                $record = Install-FileWithRollback `
                    -Source $case.Source `
                    -Destination $case.Destination `
                    -Nonce $case.Nonce `
                    -ExpectedHash $case.ExpectedHash
                Restore-InstalledFile -InstallRecord $record
                Assert-InstallSelfTestContent -Path $case.Destination -Expected 'old-upgrade-install' -Label 'Restored upgrade destination'
                if ($IsWindows -and (Test-Path -LiteralPath $record.Backup)) {
                    throw 'Windows upgrade rollback left the moved backup leaf'
                }
                if ($IsLinux) {
                    $null = Assert-InstallSelfTestRetainedEntries -Ledger $record.PathBindingLedger -Label 'Linux upgrade rollback' -MinimumCount 5
                }
            } finally {
                Close-InstallSelfTestRecord -InstallRecord $record
            }
        }

        Invoke-InstallNamedSelfTest -Name 'same_bytes_different_identity_replacement' -Context $context -Body {
            param($ctx)
            $case = New-InstallSelfTestCase `
                -TestRoot $ctx.TestRoot `
                -Name 'same-bytes-identity' `
                -SourceContent 'identical-publication' `
                -WithOriginal `
                -OriginalContent 'same-bytes-old'
            $displaced = "$($case.Destination).owned-displaced"
            $record = $null
            $failure = $null
            try {
                $record = Install-FileWithRollback `
                    -Source $case.Source `
                    -Destination $case.Destination `
                    -Nonce $case.Nonce `
                    -ExpectedHash $case.ExpectedHash
                [IO.File]::Move($case.Destination, $displaced)
                Write-InstallSelfTestFile -Path $case.Destination -Content $case.SourceContent
                try {
                    Restore-InstalledFile -InstallRecord $record
                } catch {
                    $failure = $_
                }
                if ($null -eq $failure -or $failure.Exception.Message -notmatch 'identity|binding mismatch') {
                    throw 'same-byte replacement with a different platform identity was accepted'
                }
                Assert-InstallSelfTestContent -Path $case.Destination -Expected $case.SourceContent -Label 'Same-byte concurrent replacement'
                Assert-InstallSelfTestContent -Path $displaced -Expected $case.SourceContent -Label 'Displaced owned publication'
                Assert-InstallSelfTestContent -Path $record.Backup -Expected 'same-bytes-old' -Label 'Preserved same-byte rollback backup'
                $null = Assert-InstallSelfTestBackupReportedRetained `
                    -InstallRecord $record `
                    -Label 'Same-byte replacement rollback' `
                    -ExpectedStates @('rollback_failed_retained')
            } finally {
                Close-InstallSelfTestRecord -InstallRecord $record
            }
        }

        Invoke-InstallNamedSelfTest -Name 'rollback_race_after_publication_isolation' -Context $context -Body {
            param($ctx)
            $case = New-InstallSelfTestCase `
                -TestRoot $ctx.TestRoot `
                -Name 'rollback-publication-race' `
                -SourceContent 'rollback-race-publication' `
                -WithOriginal `
                -OriginalContent 'rollback-race-old'
            $record = $null
            $failure = $null
            try {
                $record = Install-FileWithRollback `
                    -Source $case.Source `
                    -Destination $case.Destination `
                    -Nonce $case.Nonce `
                    -ExpectedHash $case.ExpectedHash
                $hooks = @{
                    after_rollback_publication_raw = {
                        param($installRecord)
                        Write-InstallSelfTestFile -Path $installRecord.Destination -Content 'rollback-race-winner'
                    }
                }
                try {
                    Restore-InstalledFile -InstallRecord $record -TestHooks $hooks
                } catch {
                    $failure = $_
                }
                if ($null -eq $failure -or $failure.Exception.Message -notmatch 'rollback|restor') {
                    throw 'rollback race did not fail closed'
                }
                Assert-InstallSelfTestContent -Path $case.Destination -Expected 'rollback-race-winner' -Label 'Rollback race winner'
                Assert-InstallSelfTestContent -Path $record.Backup -Expected 'rollback-race-old' -Label 'Rollback race backup'
                $null = Assert-InstallSelfTestBackupReportedRetained `
                    -InstallRecord $record `
                    -Label 'Rollback publication race' `
                    -ExpectedStates @('rollback_failed_retained')
                if ($IsLinux) {
                    $null = Assert-InstallSelfTestRetainedEntries -Ledger $record.PathBindingLedger -Label 'Linux rollback race' -MinimumCount 4
                }
            } finally {
                Close-InstallSelfTestRecord -InstallRecord $record
            }
        }

        Invoke-InstallNamedSelfTest -Name 'terminal_ledger_reconciliation' -Context $context -Body {
            param($ctx)
            $case = New-InstallSelfTestCase -TestRoot $ctx.TestRoot -Name 'ledger-reconciliation' -SourceContent 'ledger-publication'
            $record = $null
            try {
                $record = Install-FileWithRollback `
                    -Source $case.Source `
                    -Destination $case.Destination `
                    -Nonce $case.Nonce `
                    -ExpectedHash $case.ExpectedHash
                $initialErrors = @(Reconcile-InstallPathBindingLedger `
                    -Ledger $record.PathBindingLedger `
                    -ParentLease $record.ParentLease)
                if ($initialErrors.Count -ne 0 -or
                    @($record.PathBindingLedger | Where-Object {
                        $_.Active -and $_.VerificationStatus -in @(
                            'not_verified',
                            'missing',
                            'identity_or_metadata_mismatch',
                            'reopen_failed'
                        )
                    }).Count -ne 0) {
                    throw "successful transaction ledger was not terminally verified: $($initialErrors -join ' | ')"
                }

                $probeLeaf = "terminal-ledger-probe-$([Guid]::NewGuid().ToString('N')).bin"
                $probePath = Join-Path $record.DestinationDirectory $probeLeaf
                $probeEntry = Add-InstallLedgerEntry `
                    -Ledger $record.PathBindingLedger `
                    -ParentLease $record.ParentLease `
                    -Leaf $probeLeaf `
                    -Role 'selftest_expected_vacancy' `
                    -State 'probe' `
                    -Reason 'self-test terminal reconciliation race' `
                    -ExpectedPresence Absent `
                    -Active $true
                Write-InstallSelfTestFile -Path $probePath -Content 'terminal-ledger-race'
                $raceErrors = @(Reconcile-InstallPathBindingLedger `
                    -Ledger $record.PathBindingLedger `
                    -ParentLease $record.ParentLease)
                if ($raceErrors.Count -eq 0 -or
                    $probeEntry.VerificationStatus -ne 'unexpected_object_preserved') {
                    throw 'terminal ledger reconciliation did not report an occupied expected-vacant leaf'
                }
                [IO.File]::Delete($probePath)
                $cleanupWarning = Remove-CommittedBackup -InstallRecord $record
                if ($IsWindows -and -not [string]::IsNullOrWhiteSpace($cleanupWarning)) {
                    throw "Windows terminal ledger cleanup remained incomplete: $cleanupWarning"
                }
                if ($IsLinux -and $cleanupWarning -notmatch 'retained-evidence policy') {
                    throw 'Linux terminal ledger cleanup did not report retained evidence'
                }
            } finally {
                Close-InstallSelfTestRecord -InstallRecord $record
            }
        }

        Invoke-InstallNamedSelfTest `
            -Name 'windows_committed_backup_replacement_race' `
            -Context $context `
            -SkipReason $(if ($IsWindows) { $null } else { 'Windows identity-bound disposition race' }) `
            -Body {
                param($ctx)
                $case = New-InstallSelfTestCase `
                    -TestRoot $ctx.TestRoot `
                    -Name 'committed-backup-race' `
                    -SourceContent 'committed-race-publication' `
                    -WithOriginal `
                    -OriginalContent 'committed-race-old'
                $record = $null
                try {
                    $record = Install-FileWithRollback `
                        -Source $case.Source `
                        -Destination $case.Destination `
                        -Nonce $case.Nonce `
                        -ExpectedHash $case.ExpectedHash
                    $hooks = @{
                        after_committed_backup_raw = {
                            param($installRecord)
                            Write-InstallSelfTestFile -Path $installRecord.Backup -Content 'committed-race-old'
                        }
                    }
                    $warning = Remove-CommittedBackup -InstallRecord $record -TestHooks $hooks
                    if ($warning -notmatch 'expected-vacant|occupied and was preserved') {
                        throw "committed-backup replacement race was not reported: $warning"
                    }
                    Assert-InstallSelfTestContent -Path $record.Backup -Expected 'committed-race-old' -Label 'Concurrent committed-backup replacement'
                    Assert-InstallSelfTestContent -Path $case.Destination -Expected 'committed-race-publication' -Label 'Committed publication after backup race'
                } finally {
                    Close-InstallSelfTestRecord -InstallRecord $record
                }
            }

        Invoke-InstallNamedSelfTest `
            -Name 'linux_preflight_retained_evidence_no_auto_unlink' `
            -Context $context `
            -SkipReason $(if ($IsLinux) { $null } else { 'Linux renameat2/hardlink runtime policy' }) `
            -Body {
                param($ctx)
                $case = New-InstallSelfTestCase `
                    -TestRoot $ctx.TestRoot `
                    -Name 'linux-retained-policy' `
                    -SourceContent 'linux-retained-publication' `
                    -WithOriginal `
                    -OriginalContent 'linux-retained-old'
                $record = $null
                try {
                    $record = Install-FileWithRollback `
                        -Source $case.Source `
                        -Destination $case.Destination `
                        -Nonce $case.Nonce `
                        -ExpectedHash $case.ExpectedHash
                    if (-not $record.LinuxPreflightComplete) {
                        throw 'Linux renameat2/hardlink preflight did not complete before publication'
                    }
                    foreach ($requiredState in @(
                            'preflight_retained_evidence|retained_verified',
                            'prepared_stage|retained_prepared_stage',
                            'retained_original_destination|retained_verified'
                        )) {
                        if (@($record.PathBindingLedger | Where-Object {
                            "$($_.Role)|$($_.State)" -eq $requiredState
                        }).Count -ne 1) {
                            throw "Linux preflight ledger is missing $requiredState"
                        }
                    }
                    $warning = Remove-CommittedBackup -InstallRecord $record
                    $retained = @(Assert-InstallSelfTestRetainedEntries `
                        -Ledger $record.PathBindingLedger `
                        -Label 'Linux committed retained-evidence policy' `
                        -MinimumCount 4)
                    if ($warning -notmatch "kept $($retained.Count) identity-bound path") {
                        throw "Linux retained-evidence count was not reported precisely: $warning"
                    }
                    if (@($retained | Where-Object {
                        "$($_.Role)|$($_.State)" -eq 'rollback_backup|committed_retained_backup'
                    }).Count -ne 1) {
                        throw 'Linux committed rollback backup was not retained in the terminal ledger'
                    }
                    Assert-InstallSelfTestContent -Path $case.Destination -Expected 'linux-retained-publication' -Label 'Linux committed publication'
                } finally {
                    Close-InstallSelfTestRecord -InstallRecord $record
                }
            }
        Invoke-InstallNamedSelfTest `
            -Name 'windows_attribute_metadata_drift' `
            -Context $context `
            -SkipReason $(if ($IsWindows) { $null } else { 'Windows metadata binding' }) `
            -Body {
                param($ctx)
                $case = New-InstallSelfTestCase `
                    -TestRoot $ctx.TestRoot `
                    -Name 'windows-attribute-drift' `
                    -SourceContent 'windows-attribute-publication' `
                    -WithOriginal `
                    -OriginalContent 'windows-attribute-old'
                $record = $null
                $originalAttributes = $null
                try {
                    $record = Install-FileWithRollback `
                        -Source $case.Source `
                        -Destination $case.Destination `
                        -Nonce $case.Nonce `
                        -ExpectedHash $case.ExpectedHash
                    $originalAttributes = [IO.File]::GetAttributes($record.Backup)
                    [IO.File]::SetAttributes($record.Backup, $originalAttributes -bor [IO.FileAttributes]::ReadOnly)
                    $warning = Remove-CommittedBackup -InstallRecord $record
                    if ([string]::IsNullOrWhiteSpace($warning) -or
                        $warning -notmatch 'attributes|metadata|binding mismatch') {
                        throw "Windows attribute-only drift was not reported: $warning"
                    }
                    $null = Assert-InstallSelfTestBackupReportedRetained `
                        -InstallRecord $record `
                        -Label 'Windows attribute-drift cleanup' `
                        -ExpectedStates @('committed_cleanup_failed_retained')
                    Assert-InstallSelfTestContent -Path $record.Backup -Expected 'windows-attribute-old' -Label 'Attribute-drift backup'
                } finally {
                    if ($null -ne $record -and
                        $null -ne $originalAttributes -and
                        (Test-Path -LiteralPath $record.Backup -PathType Leaf)) {
                        [IO.File]::SetAttributes($record.Backup, $originalAttributes)
                    }
                    Close-InstallSelfTestRecord -InstallRecord $record
                }
            }

        Invoke-InstallNamedSelfTest `
            -Name 'windows_security_descriptor_drift' `
            -Context $context `
            -SkipReason $(if ($IsWindows) { $null } else { 'Windows owner/group/DACL binding' }) `
            -Body {
                param($ctx)
                $case = New-InstallSelfTestCase `
                    -TestRoot $ctx.TestRoot `
                    -Name 'windows-dacl-drift' `
                    -SourceContent 'windows-dacl-publication' `
                    -WithOriginal `
                    -OriginalContent 'windows-dacl-old'
                $record = $null
                $originalAcl = $null
                try {
                    $record = Install-FileWithRollback `
                        -Source $case.Source `
                        -Destination $case.Destination `
                        -Nonce $case.Nonce `
                        -ExpectedHash $case.ExpectedHash
                    $originalAcl = Get-Acl -LiteralPath $record.Backup
                    $changedAcl = Get-Acl -LiteralPath $record.Backup
                    $sid = [Security.Principal.WindowsIdentity]::GetCurrent().User
                    $rule = [Security.AccessControl.FileSystemAccessRule]::new(
                        $sid,
                        [Security.AccessControl.FileSystemRights]::ReadAttributes,
                        [Security.AccessControl.AccessControlType]::Allow
                    )
                    $null = $changedAcl.AddAccessRule($rule)
                    Set-Acl -LiteralPath $record.Backup -AclObject $changedAcl
                    $warning = Remove-CommittedBackup -InstallRecord $record
                    if ([string]::IsNullOrWhiteSpace($warning) -or
                        $warning -notmatch 'DACL|metadata|binding mismatch') {
                        throw "Windows security-descriptor drift was not reported: $warning"
                    }
                    $null = Assert-InstallSelfTestBackupReportedRetained `
                        -InstallRecord $record `
                        -Label 'Windows DACL-drift cleanup' `
                        -ExpectedStates @('committed_cleanup_failed_retained')
                    Assert-InstallSelfTestContent -Path $record.Backup -Expected 'windows-dacl-old' -Label 'DACL-drift backup'
                } finally {
                    if ($null -ne $record -and
                        $null -ne $originalAcl -and
                        (Test-Path -LiteralPath $record.Backup -PathType Leaf)) {
                        Set-Acl -LiteralPath $record.Backup -AclObject $originalAcl
                    }
                    Close-InstallSelfTestRecord -InstallRecord $record
                }
            }

        Invoke-InstallNamedSelfTest `
            -Name 'linux_mode_metadata_drift' `
            -Context $context `
            -SkipReason $(if ($IsLinux) { $null } else { 'Linux mode binding' }) `
            -Body {
                param($ctx)
                $case = New-InstallSelfTestCase `
                    -TestRoot $ctx.TestRoot `
                    -Name 'linux-mode-drift' `
                    -SourceContent 'linux-mode-publication' `
                    -WithOriginal `
                    -OriginalContent 'linux-mode-old'
                $record = $null
                $originalMode = $null
                try {
                    $record = Install-FileWithRollback `
                        -Source $case.Source `
                        -Destination $case.Destination `
                        -Nonce $case.Nonce `
                        -ExpectedHash $case.ExpectedHash
                    $originalMode = [IO.File]::GetUnixFileMode($record.Backup)
                    [IO.File]::SetUnixFileMode(
                        $record.Backup,
                        $originalMode -bxor [IO.UnixFileMode]::OtherRead
                    )
                    $warning = Remove-CommittedBackup -InstallRecord $record
                    if ([string]::IsNullOrWhiteSpace($warning) -or
                        $warning -notmatch 'mode|metadata|binding mismatch') {
                        throw "Linux mode drift was not reported: $warning"
                    }
                    $null = Assert-InstallSelfTestBackupReportedRetained `
                        -InstallRecord $record `
                        -Label 'Linux mode-drift cleanup' `
                        -ExpectedStates @('committed_cleanup_failed_retained')
                    Assert-InstallSelfTestContent -Path $record.Backup -Expected 'linux-mode-old' -Label 'Linux mode-drift backup'
                } finally {
                    if ($null -ne $record -and
                        $null -ne $originalMode -and
                        (Test-Path -LiteralPath $record.Backup -PathType Leaf)) {
                        [IO.File]::SetUnixFileMode($record.Backup, $originalMode)
                    }
                    Close-InstallSelfTestRecord -InstallRecord $record
                }
            }

        $setfaclCommand = if ($IsLinux) {
            Get-Command 'setfacl' -CommandType Application -ErrorAction SilentlyContinue
        } else {
            $null
        }
        if ($IsLinux -and $null -eq $setfaclCommand -and
            (-not [string]::IsNullOrWhiteSpace($env:CI) -or
                -not [string]::IsNullOrWhiteSpace($env:GITHUB_ACTIONS))) {
            throw 'CI Linux runtime proof requires setfacl; refusing to capability-skip the POSIX ACL test.'
        }
        Invoke-InstallNamedSelfTest `
            -Name 'linux_posix_acl_xattr_drift' `
            -Context $context `
            -SkipReason $(
                if (-not $IsLinux) {
                    'Linux POSIX ACL binding'
                } elseif ($null -eq $setfaclCommand) {
                    'setfacl is unavailable; runtime ACL proof was not executed'
                } else {
                    $null
                }
            ) `
            -Body {
                param($ctx)
                $setfacl = Get-Command 'setfacl' -CommandType Application -ErrorAction Stop
                $case = New-InstallSelfTestCase `
                    -TestRoot $ctx.TestRoot `
                    -Name 'linux-acl-drift' `
                    -SourceContent 'linux-acl-publication' `
                    -WithOriginal `
                    -OriginalContent 'linux-acl-old'
                & $setfacl.Source '-n' '-m' 'u:65534:r--,m::rwx' '--' $case.Destination
                if ($LASTEXITCODE -ne 0) {
                    throw "failed to establish POSIX ACL baseline: exit=$LASTEXITCODE"
                }
                $record = $null
                try {
                    $record = Install-FileWithRollback `
                        -Source $case.Source `
                        -Destination $case.Destination `
                        -Nonce $case.Nonce `
                        -ExpectedHash $case.ExpectedHash
                    $modeBefore = [IO.File]::GetUnixFileMode($record.Backup)
                    & $setfacl.Source '-n' '-m' 'u:65534:rw-,m::rwx' '--' $record.Backup
                    if ($LASTEXITCODE -ne 0) {
                        throw "failed to mutate POSIX ACL: exit=$LASTEXITCODE"
                    }
                    if ([IO.File]::GetUnixFileMode($record.Backup) -ne $modeBefore) {
                        throw 'POSIX ACL mutation changed Unix mode and did not isolate xattr drift'
                    }
                    $warning = Remove-CommittedBackup -InstallRecord $record
                    if ([string]::IsNullOrWhiteSpace($warning) -or
                        $warning -notmatch 'xattr|metadata|binding mismatch') {
                        throw "Linux POSIX ACL xattr drift was not reported: $warning"
                    }
                    $null = Assert-InstallSelfTestBackupReportedRetained `
                        -InstallRecord $record `
                        -Label 'Linux ACL-drift cleanup' `
                        -ExpectedStates @('committed_cleanup_failed_retained')
                    Assert-InstallSelfTestContent -Path $record.Backup -Expected 'linux-acl-old' -Label 'Linux ACL-drift backup'
                } finally {
                    if ($null -ne $record -and (Test-Path -LiteralPath $record.Backup -PathType Leaf)) {
                        & $setfacl.Source '-b' '--' $record.Backup
                        if ($LASTEXITCODE -ne 0) {
                            throw "failed to clear POSIX ACL probe: exit=$LASTEXITCODE"
                        }
                    }
                    Close-InstallSelfTestRecord -InstallRecord $record
                }
            }
        Invoke-InstallNamedSelfTest -Name 'doctor_whole_tree_retained_isolation' -Context $context -Body {
            param($ctx)
            $doctorParent = Join-Path $ctx.TestRoot 'doctor-normal-parent'
            $null = Resolve-ManagedDirectory -Path $doctorParent -Label 'Doctor normal self-test parent'
            $record = New-TemporaryDoctorWorkspace -TemporaryParent $doctorParent
            $originalRoot = $record.Root
            $sibling = Join-Path $doctorParent 'sibling.sentinel'
            Write-InstallSelfTestFile -Path $sibling -Content 'preserve-sibling'
            Write-InstallSelfTestFile -Path (Join-Path $record.Root 'owned.txt') -Content 'owned-doctor-tree'
            $warning = Remove-TemporaryDoctorWorkspace -Record $record
            if ($warning -notmatch 'retained for review' -or
                $record.State -ne 'retained_leases_released' -or
                @($record.TerminalErrors).Count -ne 0 -or
                [string]::IsNullOrWhiteSpace([string]$record.RetainedRoot) -or
                (Test-Path -LiteralPath $originalRoot) -or
                -not (Test-Path -LiteralPath $record.RetainedRoot -PathType Container)) {
                throw "doctor root was not terminally retained by whole-tree no-replace isolation: $warning"
            }
            if (-not [string]::Equals(
                    [string]$record.MarkerPath,
                    [string](Join-Path $record.RetainedRoot $record.MarkerLeaf),
                    [StringComparison]::OrdinalIgnoreCase
                )) {
                throw 'doctor marker path was not rebound to the retained root'
            }
            $parentLease = New-InstallDirectoryLease -Path $record.TempRoot
            $retainedLease = $null
            $markerLease = $null
            try {
                $retainedLease = Open-InstallDirectoryLeafLease `
                    -ParentLease $parentLease `
                    -Leaf (Split-Path -Leaf $record.RetainedRoot)
                if ($null -eq $retainedLease) {
                    throw 'retained doctor root could not be reopened through its held-parent model'
                }
                Assert-InstallDirectorySnapshot `
                    -Actual $retainedLease.CaptureIdentity() `
                    -Expected $record.RootIdentity `
                    -Label 'Self-test retained doctor root'
                $markerLease = Open-InstallLeafLease `
                    -ParentLease $retainedLease `
                    -Leaf $record.MarkerLeaf
                if ($null -eq $markerLease) {
                    throw 'retained doctor owner marker could not be reopened relative to the retained root'
                }
                Assert-InstallFileSnapshot `
                    -Actual $markerLease.CaptureSnapshot() `
                    -Expected $record.MarkerIdentity `
                    -ExpectedHash $record.MarkerHash `
                    -Label 'Self-test retained doctor marker'
            } finally {
                if ($null -ne $markerLease) { $markerLease.Dispose() }
                if ($null -ne $retainedLease) { $retainedLease.Dispose() }
                $parentLease.Dispose()
            }
            Assert-InstallSelfTestContent -Path (Join-Path $record.RetainedRoot 'owned.txt') -Expected 'owned-doctor-tree' -Label 'Retained doctor owned leaf'
            Assert-InstallSelfTestContent -Path $sibling -Expected 'preserve-sibling' -Label 'Doctor sibling'
            $retainedRoot = $record.RetainedRoot
            $second = Remove-TemporaryDoctorWorkspace -Record $record
            if ($second -notmatch 'remains retained.*no retry' -or $second -match 'already retained' -or
                -not [string]::Equals([string]$record.RetainedRoot, [string]$retainedRoot, [StringComparison]::Ordinal)) {
                throw 'second doctor cleanup call retried an already retained workspace'
            }
        }

        Invoke-InstallNamedSelfTest -Name 'doctor_reparse_descendant_not_traversed' -Context $context -Body {
            param($ctx)
            $doctorParent = Join-Path $ctx.TestRoot 'doctor-reparse-parent'
            $null = Resolve-ManagedDirectory -Path $doctorParent -Label 'Doctor reparse self-test parent'
            $record = New-TemporaryDoctorWorkspace -TemporaryParent $doctorParent
            $outsideSentinel = Join-Path $ctx.Outside 'doctor-reparse-outside.sentinel'
            Write-InstallSelfTestFile -Path $outsideSentinel -Content 'outside-preserved'
            $descendantLeaf = 'reparse-descendant'
            $descendant = Join-Path $record.Root $descendantLeaf
            $linkType = if ($IsWindows) { 'Junction' } else { 'SymbolicLink' }
            New-Item -ItemType $linkType -Path $descendant -Target $ctx.Outside | Out-Null
            $warning = Remove-TemporaryDoctorWorkspace -Record $record
            $retainedDescendant = Join-Path $record.RetainedRoot $descendantLeaf
            if ($warning -notmatch 'terminally verified' -or
                $record.State -ne 'retained_leases_released' -or
                (Test-Path -LiteralPath $record.Root) -or
                -not (Test-Path -LiteralPath $record.RetainedRoot -PathType Container) -or
                -not (Test-Path -LiteralPath $retainedDescendant) -or
                @($record.TerminalErrors).Count -ne 0) {
                throw "doctor did not isolate the reparse-containing tree without traversal: $warning"
            }
            Assert-InstallSelfTestContent -Path $outsideSentinel -Expected 'outside-preserved' -Label 'Doctor reparse outside sentinel'
            $second = Remove-TemporaryDoctorWorkspace -Record $record
            if ($second -notmatch 'remains retained.*no retry' -or $second -match 'already retained') {
                throw 'doctor reparse case retried a released retained lease'
            }
            Remove-Item -LiteralPath $retainedDescendant -Force
        }

        Invoke-InstallNamedSelfTest -Name 'doctor_marker_drift_fail_closed' -Context $context -Body {
            param($ctx)
            $doctorParent = Join-Path $ctx.TestRoot 'doctor-marker-drift-parent'
            $null = Resolve-ManagedDirectory -Path $doctorParent -Label 'Doctor marker-drift self-test parent'
            $record = New-TemporaryDoctorWorkspace -TemporaryParent $doctorParent
            Write-InstallSelfTestFile -Path $record.MarkerPath -Content 'tampered-marker'
            $warning = Remove-TemporaryDoctorWorkspace -Record $record
            if ($record.State -ne 'failed_leases_released' -or
                -not [string]::IsNullOrWhiteSpace([string]$record.RetainedRoot) -or
                -not (Test-Path -LiteralPath $record.Root -PathType Container) -or
                @($record.TerminalErrors).Count -eq 0 -or
                $warning -notmatch 'Terminal verification reported') {
                throw "doctor marker drift did not fail closed with the original root preserved: $warning"
            }
            $second = Remove-TemporaryDoctorWorkspace -Record $record
            if ($second -notmatch 'prior failed isolation' -or $second -match 'already retained') {
                throw 'doctor marker-drift retry misreported a failed released lease as retained'
            }
        }

        Invoke-InstallNamedSelfTest -Name 'doctor_post_rename_failure_not_retried' -Context $context -Body {
            param($ctx)
            $doctorParent = Join-Path $ctx.TestRoot 'doctor-post-rename-failure-parent'
            $null = Resolve-ManagedDirectory -Path $doctorParent -Label 'Doctor post-rename self-test parent'
            $record = New-TemporaryDoctorWorkspace -TemporaryParent $doctorParent
            $hooks = @{
                after_doctor_isolate_raw = {
                    param($doctorRecord)
                    throw 'self-test injected post-rename doctor verification failure'
                }
            }
            $warning = Remove-TemporaryDoctorWorkspace -Record $record -TestHooks $hooks
            if ($record.State -ne 'failed_leases_released' -or
                [string]::IsNullOrWhiteSpace([string]$record.RetainedRoot) -or
                (Test-Path -LiteralPath $record.Root) -or
                -not (Test-Path -LiteralPath $record.RetainedRoot -PathType Container) -or
                @($record.TerminalErrors).Count -eq 0 -or
                $warning -notmatch 'post-rename doctor verification failure') {
                throw "post-rename doctor failure lost or misreported the retained root: $warning"
            }
            $second = Remove-TemporaryDoctorWorkspace -Record $record
            if ($second -notmatch 'prior failed isolation' -or $second -match 'already retained') {
                throw 'doctor retried or misreported a post-rename verification failure'
            }
        }
        Invoke-InstallNamedSelfTest -Name 'path_projection_ordering' -Context $context -Body {
            param($ctx)
            $destination = Join-Path $ctx.TestRoot 'future-bin'
            $oldUserEntry = Join-Path $ctx.TestRoot 'old-user-bin'
            $proposed = Get-ProposedUserPath `
                -ExistingUserPath "$oldUserEntry$([IO.Path]::PathSeparator)$destination" `
                -CliDirectory $destination
            $entries = @($proposed -split [IO.Path]::PathSeparator)
            if ($entries.Count -ne 2 -or
                (Get-PathComparisonKey $entries[0]) -ne (Get-PathComparisonKey $destination) -or
                (Get-PathComparisonKey $entries[1]) -ne (Get-PathComparisonKey $oldUserEntry)) {
                throw 'destination was not moved to the front of the proposed user PATH'
            }
            $machineEntry = Join-Path $ctx.TestRoot 'machine-bin'
            $projected = Get-ProjectedPersistentPath -MachinePath $machineEntry -UserPath $proposed
            $projectedEntries = @($projected -split [IO.Path]::PathSeparator)
            if ($projectedEntries.Count -ne 3 -or
                (Get-PathComparisonKey $projectedEntries[0]) -ne (Get-PathComparisonKey $machineEntry) -or
                (Get-PathComparisonKey $projectedEntries[1]) -ne (Get-PathComparisonKey $destination)) {
                throw 'projected persistent PATH did not preserve Machine + proposed User ordering'
            }
        }

        Invoke-InstallNamedSelfTest -Name 'hook_install_status_exact_match' -Context $context -Body {
            param($ctx)
            $installReport = [pscustomobject]@{
                hooks_path = (Join-Path $ctx.TestRoot 'hooks.json')
                installed = $true
                changed = $true
                command = 'rayman codex-hook stop'
            }
            $statusReport = [pscustomobject]@{
                hooks_path = $installReport.hooks_path
                installed = $true
                changed = $false
                command = $installReport.command
            }
            Assert-HookInstallationReports -InstallReport $installReport -StatusReport $statusReport
            foreach ($badStatus in @(
                    [pscustomobject]@{
                        hooks_path = ($installReport.hooks_path + '.different')
                        installed = $true
                        changed = $false
                        command = 'different stop command'
                    },
                    [pscustomobject]@{
                        hooks_path = $installReport.hooks_path
                        installed = $false
                        changed = $false
                        command = $installReport.command
                    }
                )) {
                $rejected = $false
                try {
                    Assert-HookInstallationReports -InstallReport $installReport -StatusReport $badStatus
                } catch {
                    $rejected = $_.Exception.Message -match 'does not exactly match'
                }
                if (-not $rejected) {
                    throw 'mismatched or noncanonical Hook status was accepted'
                }
            }
        }

        Invoke-InstallNamedSelfTest `
            -Name 'windows_user_environment_cas_logic' `
            -SkipReason $(if ($IsWindows) { $null } else { 'Windows environment CAS contract' }) `
            -Body {
                param($unused)
                $probeName = 'RaymanInstallSelfTestPathProbe_' + [Guid]::NewGuid().ToString('N')
                $store = @{}
                $absent = [pscustomobject]@{
                    Name = $probeName
                    Exists = $false
                    Value = $null
                    Kind = [Microsoft.Win32.RegistryValueKind]::ExpandString
                }
                $original = [pscustomobject]@{
                    Name = $probeName
                    Exists = $true
                    Value = '%RAYMAN_SELFTEST_ROOT%\original'
                    Kind = [Microsoft.Win32.RegistryValueKind]::ExpandString
                }
                $published = [pscustomobject]@{
                    Name = $probeName
                    Exists = $true
                    Value = 'C:\literal\published'
                    Kind = [Microsoft.Win32.RegistryValueKind]::String
                }
                $checkedPublished = [pscustomobject]@{
                    Name = $probeName
                    Exists = $true
                    Value = '%RAYMAN_SELFTEST_ROOT%\checked-published'
                    Kind = [Microsoft.Win32.RegistryValueKind]::ExpandString
                }
                $concurrent = [pscustomobject]@{
                    Name = $probeName
                    Exists = $true
                    Value = 'C:\literal\concurrent'
                    Kind = [Microsoft.Win32.RegistryValueKind]::String
                }
                try {
                    if ((Get-PersistentUserEnvironmentRecord -Name $probeName -TestStore $store).Exists) {
                        throw 'scratch environment test record unexpectedly exists'
                    }

                    $publicationCommitted = $false
                    $publication = Invoke-PersistentUserEnvironmentRecordCas `
                        -Expected $absent `
                        -Desired $checkedPublished `
                        -MutationCommitted ([ref]$publicationCommitted) `
                        -TestStore $store
                    if (-not $publicationCommitted -or
                        -not $publication.Changed -or
                        $publication.AlreadyDesired -or
                        -not (Test-PersistentUserEnvironmentRecord `
                            -Expected $checkedPublished `
                            -Actual (Get-PersistentUserEnvironmentRecord -Name $probeName -TestStore $store))) {
                        throw 'environment compare-exchange did not publish an absent value'
                    }

                    $idempotentCommitted = $true
                    $idempotent = Invoke-PersistentUserEnvironmentRecordCas `
                        -Expected $absent `
                        -Desired $checkedPublished `
                        -MutationCommitted ([ref]$idempotentCommitted) `
                        -TestStore $store
                    if ($idempotentCommitted -or $idempotent.Changed -or -not $idempotent.AlreadyDesired) {
                        throw 'environment compare-exchange did not treat an already-desired value as an unowned no-op'
                    }
                    Restore-PersistentUserEnvironmentRecordCas `
                        -Original $absent `
                        -Published $checkedPublished `
                        -TestStore $store

                    Set-PersistentUserEnvironmentRecord -Record $original -TestStore $store
                    $beforeCommit = {
                        Set-PersistentUserEnvironmentRecord -Record $concurrent -TestStore $store
                    }
                    $raceCommitted = $false
                    $raceRejected = $false
                    try {
                        $null = Invoke-PersistentUserEnvironmentRecordCas `
                            -Expected $original `
                            -Desired $checkedPublished `
                            -MutationCommitted ([ref]$raceCommitted) `
                            -TestStore $store `
                            -BeforeCommitTestHook $beforeCommit
                    } catch {
                        $raceRejected = $_.Exception.Message -match 'changed concurrently'
                    }
                    if (-not $raceRejected -or $raceCommitted -or
                        -not (Test-PersistentUserEnvironmentRecord `
                            -Expected $concurrent `
                            -Actual (Get-PersistentUserEnvironmentRecord -Name $probeName -TestStore $store))) {
                        throw 'environment compare-exchange overwrote a pre-commit concurrent value'
                    }

                    Set-PersistentUserEnvironmentRecord -Record $original -TestStore $store
                    $afterCommit = {
                        Set-PersistentUserEnvironmentRecord -Record $concurrent -TestStore $store
                    }
                    $postCommitMutation = $false
                    $postCommitRejected = $false
                    try {
                        $null = Invoke-PersistentUserEnvironmentRecordCas `
                            -Expected $original `
                            -Desired $checkedPublished `
                            -MutationCommitted ([ref]$postCommitMutation) `
                            -TestStore $store `
                            -AfterCommitBeforeVerifyTestHook $afterCommit
                    } catch {
                        $postCommitRejected = $_.Exception.Message -match 'changed after transactional compare-exchange commit'
                    }
                    if (-not $postCommitRejected -or -not $postCommitMutation -or
                        -not (Test-PersistentUserEnvironmentRecord `
                            -Expected $concurrent `
                            -Actual (Get-PersistentUserEnvironmentRecord -Name $probeName -TestStore $store))) {
                        throw 'environment verification overwrote or lost a post-commit concurrent winner'
                    }

                    Set-PersistentUserEnvironmentRecord -Record $original -TestStore $store
                    $updateCommitted = $false
                    $null = Invoke-PersistentUserEnvironmentRecordCas `
                        -Expected $original `
                        -Desired $checkedPublished `
                        -MutationCommitted ([ref]$updateCommitted) `
                        -TestStore $store
                    if (-not $updateCommitted -or
                        -not (Test-PersistentUserEnvironmentRecord `
                            -Expected $checkedPublished `
                            -Actual (Get-PersistentUserEnvironmentRecord -Name $probeName -TestStore $store))) {
                        throw 'environment compare-exchange did not publish over the expected old value'
                    }
                    Restore-PersistentUserEnvironmentRecordCas `
                        -Original $original `
                        -Published $checkedPublished `
                        -TestStore $store

                    Set-PersistentUserEnvironmentRecord -Record $original -TestStore $store
                    Restore-PersistentUserEnvironmentRecordCas `
                        -Original $original `
                        -Published $published `
                        -TestStore $store
                    if (-not (Test-PersistentUserEnvironmentRecord `
                            -Expected $original `
                            -Actual (Get-PersistentUserEnvironmentRecord -Name $probeName -TestStore $store))) {
                        throw 'environment CAS changed an already-original value'
                    }
                    Set-PersistentUserEnvironmentRecord -Record $published -TestStore $store
                    Restore-PersistentUserEnvironmentRecordCas `
                        -Original $original `
                        -Published $published `
                        -TestStore $store
                    if (-not (Test-PersistentUserEnvironmentRecord `
                            -Expected $original `
                            -Actual (Get-PersistentUserEnvironmentRecord -Name $probeName -TestStore $store))) {
                        throw 'environment CAS did not restore the original value and kind'
                    }
                    Set-PersistentUserEnvironmentRecord -Record $concurrent -TestStore $store
                    $rejected = $false
                    try {
                        Restore-PersistentUserEnvironmentRecordCas `
                            -Original $original `
                            -Published $published `
                            -TestStore $store
                    } catch {
                        $rejected = $_.Exception.Message -match 'changed concurrently'
                    }
                    if (-not $rejected -or
                        -not (Test-PersistentUserEnvironmentRecord `
                            -Expected $concurrent `
                            -Actual (Get-PersistentUserEnvironmentRecord -Name $probeName -TestStore $store))) {
                        throw 'environment CAS overwrote a concurrent value'
                    }
                    Set-PersistentUserEnvironmentRecord -Record $published -TestStore $store
                    Restore-PersistentUserEnvironmentRecordCas `
                        -Original $absent `
                        -Published $published `
                        -TestStore $store
                    if ((Get-PersistentUserEnvironmentRecord -Name $probeName -TestStore $store).Exists) {
                        throw 'environment CAS did not restore an absent value'
                    }
                } finally {
                    Set-PersistentUserEnvironmentRecord -Record $absent -TestStore $store
                }
            }

        $registryProbeContext = $null
        $registrySkipReason = if ($IsWindows) { $null } else { 'Windows HKCU registry round-trip' }
        if ($IsWindows) {
            $registryProbeContext = [pscustomobject]@{
                Name = 'RaymanInstallSelfTestRegistryProbe_' + [Guid]::NewGuid().ToString('N')
            }
            try {
                $writeProbeKey = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment', $true)
                if ($null -eq $writeProbeKey) {
                    throw 'HKCU\Environment is unavailable for a write-capability probe'
                }
                $writeProbeKey.Dispose()
                if ((Get-PersistentUserEnvironmentRecord -Name $registryProbeContext.Name).Exists) {
                    throw "unexpected scratch registry collision: HKCU\Environment\$($registryProbeContext.Name)"
                }
            } catch {
                $baseException = $_.Exception.GetBaseException()
                if ($baseException -is [UnauthorizedAccessException] -or
                    $_.Exception.Message -match 'registry access is not allowed|Requested registry access|Access.*denied') {
                    $registrySkipReason = "host denied HKCU\Environment access: $($baseException.Message)"
                } else {
                    throw
                }
            }
            if (-not [string]::IsNullOrWhiteSpace($registrySkipReason) -and
                (-not [string]::IsNullOrWhiteSpace($env:CI) -or
                    -not [string]::IsNullOrWhiteSpace($env:GITHUB_ACTIONS))) {
                throw "CI Windows runtime proof requires the HKCU environment round-trip: $registrySkipReason"
            }
        }
        Invoke-InstallNamedSelfTest `
            -Name 'windows_user_environment_registry_roundtrip' `
            -Context $registryProbeContext `
            -SkipReason $registrySkipReason `
            -Body {
                param($probe)
                $absent = [pscustomobject]@{
                    Name = $probe.Name
                    Exists = $false
                    Value = $null
                    Kind = [Microsoft.Win32.RegistryValueKind]::ExpandString
                }
                $original = [pscustomobject]@{
                    Name = $probe.Name
                    Exists = $true
                    Value = '%RAYMAN_SELFTEST_ROOT%\registry-roundtrip'
                    Kind = [Microsoft.Win32.RegistryValueKind]::ExpandString
                }
                $published = [pscustomobject]@{
                    Name = $probe.Name
                    Exists = $true
                    Value = '%RAYMAN_SELFTEST_ROOT%\registry-published'
                    Kind = [Microsoft.Win32.RegistryValueKind]::ExpandString
                }
                $concurrent = [pscustomobject]@{
                    Name = $probe.Name
                    Exists = $true
                    Value = 'C:\literal\registry-concurrent'
                    Kind = [Microsoft.Win32.RegistryValueKind]::String
                }
                try {
                    $absentPublicationCommitted = $false
                    $absentPublication = Invoke-PersistentUserEnvironmentRecordCas `
                        -Expected $absent `
                        -Desired $published `
                        -MutationCommitted ([ref]$absentPublicationCommitted)
                    if (-not $absentPublicationCommitted -or
                        -not $absentPublication.Changed -or
                        $absentPublication.AlreadyDesired -or
                        -not (Test-PersistentUserEnvironmentRecord `
                            -Expected $published `
                            -Actual (Get-PersistentUserEnvironmentRecord -Name $probe.Name))) {
                        throw 'HKCU transactional compare-exchange did not publish an absent native value'
                    }
                    $nativeIdempotentCommitted = $true
                    $nativeIdempotent = Invoke-PersistentUserEnvironmentRecordCas `
                        -Expected $absent `
                        -Desired $published `
                        -MutationCommitted ([ref]$nativeIdempotentCommitted)
                    if ($nativeIdempotentCommitted -or
                        $nativeIdempotent.Changed -or
                        -not $nativeIdempotent.AlreadyDesired) {
                        throw 'HKCU transactional compare-exchange did not preserve native already-desired ownership'
                    }
                    Restore-PersistentUserEnvironmentRecordCas `
                        -Original $absent `
                        -Published $published
                    if ((Get-PersistentUserEnvironmentRecord -Name $probe.Name).Exists) {
                        throw 'HKCU transactional compare-exchange did not delete the native scratch value'
                    }

                    Set-PersistentUserEnvironmentRecord -Record $original
                    $actual = Get-PersistentUserEnvironmentRecord -Name $probe.Name
                    if (-not (Test-PersistentUserEnvironmentRecord -Expected $original -Actual $actual)) {
                        throw 'HKCU environment round-trip changed the raw value or registry kind'
                    }

                    $publicationCommitted = $false
                    $null = Invoke-PersistentUserEnvironmentRecordCas `
                        -Expected $original `
                        -Desired $published `
                        -MutationCommitted ([ref]$publicationCommitted)
                    if (-not $publicationCommitted -or
                        -not (Test-PersistentUserEnvironmentRecord `
                            -Expected $published `
                            -Actual (Get-PersistentUserEnvironmentRecord -Name $probe.Name))) {
                        throw 'HKCU transactional compare-exchange did not commit the expected value and kind'
                    }
                    Restore-PersistentUserEnvironmentRecordCas `
                        -Original $original `
                        -Published $published

                    $ordinaryConcurrentWrite = {
                        Set-PersistentUserEnvironmentRecord -Record $concurrent
                    }
                    $raceCommitted = $false
                    $raceRejected = $false
                    try {
                        $null = Invoke-PersistentUserEnvironmentRecordCas `
                            -Expected $original `
                            -Desired $published `
                            -MutationCommitted ([ref]$raceCommitted) `
                            -BeforeCommitTestHook $ordinaryConcurrentWrite
                    } catch {
                        $raceRejected = $_.Exception.Message -match 'transactional compare-exchange did not commit|transaction.*commit|changed concurrently'
                    }
                    if (-not $raceRejected -or $raceCommitted -or
                        -not (Test-PersistentUserEnvironmentRecord `
                            -Expected $concurrent `
                            -Actual (Get-PersistentUserEnvironmentRecord -Name $probe.Name))) {
                        throw 'HKCU transaction overwrote an ordinary concurrent registry writer'
                    }

                    Set-PersistentUserEnvironmentRecord -Record $original
                    $ordinaryPostCommitWrite = {
                        Set-PersistentUserEnvironmentRecord -Record $concurrent
                    }
                    $postCommitMutation = $false
                    $postCommitRejected = $false
                    try {
                        $null = Invoke-PersistentUserEnvironmentRecordCas `
                            -Expected $original `
                            -Desired $published `
                            -MutationCommitted ([ref]$postCommitMutation) `
                            -AfterCommitBeforeVerifyTestHook $ordinaryPostCommitWrite
                    } catch {
                        $postCommitRejected = $_.Exception.Message -match 'changed after transactional compare-exchange commit'
                    }
                    if (-not $postCommitRejected -or -not $postCommitMutation -or
                        -not (Test-PersistentUserEnvironmentRecord `
                            -Expected $concurrent `
                            -Actual (Get-PersistentUserEnvironmentRecord -Name $probe.Name))) {
                        throw 'HKCU verification overwrote or lost a post-commit concurrent winner'
                    }
                } finally {
                    Set-PersistentUserEnvironmentRecord -Record $absent
                }
            }

        Invoke-InstallNamedSelfTest -Name 'reparse_ancestor_rejected' -Context $context -Body {
            param($ctx)
            $linkType = if ($IsWindows) { 'Junction' } else { 'SymbolicLink' }
            New-Item -ItemType $linkType -Path $ctx.EscapeLink -Target $ctx.Outside | Out-Null
            $rejected = $false
            try {
                $null = Resolve-ManagedDirectory -Path (Join-Path $ctx.EscapeLink 'child') -Label 'Escaping self-test path'
            } catch {
                $rejected = $_.Exception.Message -match 'reparse point|canonical path escaped'
            }
            if (-not $rejected) {
                throw 'a symlink/junction ancestor was not rejected'
            }
        }
    } finally {
        if (Test-Path -LiteralPath $link) {
            Remove-Item -LiteralPath $link -Force
        }
        if (Test-Path -LiteralPath $testRoot -PathType Container) {
            $testItem = Get-Item -LiteralPath $testRoot -Force
            $prefix = $managedTemp.TrimEnd([IO.Path]::DirectorySeparatorChar) + [IO.Path]::DirectorySeparatorChar
            if (-not $testItem.PSIsContainer -or
                $testItem.Attributes -band [IO.FileAttributes]::ReparsePoint -or
                -not $testItem.FullName.StartsWith(
                    $prefix,
                    $(if ($IsWindows) { [StringComparison]::OrdinalIgnoreCase } else { [StringComparison]::Ordinal })
                )) {
                throw "Refusing unsafe install path self-test cleanup: $testRoot"
            }
            Remove-Item -LiteralPath $testRoot -Recurse -Force
        }
    }
    Write-Host 'Install self-test passed: native ABI, relative handle CAS, rollback/cleanup races, metadata binding, retained-evidence reporting, doctor isolation, manifest, PATH compare-exchange logic, Hook reports, and reparse rejection.'
}


if ($SelfTest) {
    Invoke-InstallPathSelfTest
    return
}

if (-not $Yes) {
    $declared = @('the managed rayman executable') + (
        @(Get-CodexSkillResourcePlan -DestinationRoot ([IO.Path]::GetFullPath($repoRoot))) |
            ForEach-Object { "Codex skill resource '$($_.DestinationRelative)'" }
    )
    $declared += 'the current repository activation via workspace install-bind'
    if ($AddToUserPath) {
        $declared += 'HKCU\Environment\Path'
    }
    if (-not $SkipCodexStopHook) {
        $declared += 'a post-core-commit merge into Codex hooks.json'
    }
    throw "Installation will change $($declared -join ', '). Re-run with -Yes after reviewing the destination paths and requested integration flags."
}

# Fail before any installation write when this shell would invoke an alias or
# function instead of the installed executable.
$existingCommands = @(Get-Command rayman -All -ErrorAction SilentlyContinue)
if ($existingCommands.Count -gt 0 -and $existingCommands[0].CommandType -ne 'Application') {
    throw "PowerShell resolves rayman to $($existingCommands[0].CommandType) '$($existingCommands[0].Name)'. Remove that alias/function before installation so the executable can be verified."
}
$cargoApplication = Resolve-RequiredApplication -Name 'cargo' -Label 'Cargo executable'
$cargoApplicationHash = (Get-FileHash -LiteralPath $cargoApplication -Algorithm SHA256).Hash.ToLowerInvariant()

$resolvedBinDirectory = Resolve-ManagedDirectory -Path $BinDirectory -Label 'CLI directory'
$resolvedSkillDirectory = Resolve-ManagedDirectory -Path $SkillDirectory -Label 'Skill directory'
$skillResources = @(Get-CodexSkillResourcePlan -DestinationRoot $resolvedSkillDirectory)
$destinationCli = Join-Path $resolvedBinDirectory $artifactName
$canonicalSkillResource = Get-CanonicalSkillResource -ResourcePlan $skillResources
$canonicalSkill = $canonicalSkillResource.Source
$destinationSkill = $canonicalSkillResource.Destination
Assert-ReplaceableFile -Path $destinationCli -Label 'CLI'
foreach ($resource in $skillResources) {
    Assert-ReplaceableFile -Path $resource.Destination -Label "Codex skill resource '$($resource.DestinationRelative)'"
}

$doctorWorkspaceRecord = $null
$originalPath = $env:PATH
$hookInstalled = $false
$hookFailure = $null
$retainedEvidenceReport = @()

try {
    $doctorWorkspaceRecord = New-TemporaryDoctorWorkspace
    Push-Location $repoRoot
    try {
        Invoke-NativeChecked -FilePath $cargoApplication -Arguments @('build', '--locked', '--release', '-p', 'rayman')
        $releaseRoot = if ($env:CARGO_TARGET_DIR) {
            [IO.Path]::GetFullPath([IO.Path]::Combine($repoRoot, $env:CARGO_TARGET_DIR))
        } else {
            Join-Path $repoRoot 'target'
        }
        $artifact = (Resolve-Path -LiteralPath (Join-Path $releaseRoot "release/$artifactName")).ProviderPath

        $artifactHashBeforeVerification = (Get-FileHash -LiteralPath $artifact -Algorithm SHA256).Hash.ToLowerInvariant()
        $resourceHashes = @{}
        foreach ($resource in $skillResources) {
            $resourceHashes[$resource.DestinationRelative] = (Get-FileHash -LiteralPath $resource.Source -Algorithm SHA256).Hash.ToLowerInvariant()
        }

        # Never rebind the source checkout for a speculative install.
        Invoke-NativeCheckedInDirectory -Directory $doctorWorkspaceRecord.Root -FilePath $artifact -Arguments @(
            'workspace', 'activate', '--skill-file', $canonicalSkill, '--yes'
        )
        $env:PATH = "$(Split-Path -Parent $artifact)$([IO.Path]::PathSeparator)$originalPath"
        $preVerification = @{
            CliPath = $artifact
            ReferenceCliPath = $artifact
            SkillPath = $canonicalSkill
            WorkspaceSkillPath = $canonicalSkill
            SkillResourceMode = 'Source'
            DoctorWorkspace = $doctorWorkspaceRecord.Root
            RequireSourceFresh = $true
        }
        & (Join-Path $repoRoot 'scripts/verify-release-contract.ps1') @preVerification
        Assert-ExpectedFileHash -Path $artifact -ExpectedHash $artifactHashBeforeVerification -Label 'Source-fresh verified artifact'
        foreach ($resource in $skillResources) {
            Assert-ExpectedFileHash -Path $resource.Source -ExpectedHash $resourceHashes[$resource.DestinationRelative] -Label "Verified canonical resource '$($resource.SourceRelative)'"
        }
        $verifiedArtifactHash = $artifactHashBeforeVerification

        if ($AddToUserPath) {
            Assert-PersistentUserEnvironmentCasCapability
        }

        $nonce = [Guid]::NewGuid().ToString('N')
        $installed = @()
        $oldUserPathRecord = $null
        $proposedUserPathRecord = $null
        $pathMutationCommitted = $false
        $machinePathSnapshot = $null
        $machinePathSnapshotCaptured = $false
        $coreCommitted = $false
        try {
            $installed += Install-FileWithRollback -Source $artifact -Destination $destinationCli -Nonce $nonce -ExpectedHash $verifiedArtifactHash
            foreach ($resource in $skillResources) {
                $installed += Install-FileWithRollback -Source $resource.Source -Destination $resource.Destination -Nonce $nonce -ExpectedHash $resourceHashes[$resource.DestinationRelative]
            }

            Assert-ExpectedFileHash -Path $artifact -ExpectedHash $verifiedArtifactHash -Label 'Verified artifact before post-install check'
            Assert-ExpectedFileHash -Path $destinationCli -ExpectedHash $verifiedArtifactHash -Label 'Installed CLI'
            foreach ($resource in $skillResources) {
                $expectedHash = $resourceHashes[$resource.DestinationRelative]
                Assert-ExpectedFileHash -Path $resource.Source -ExpectedHash $expectedHash -Label "Verified resource before post-install check '$($resource.SourceRelative)'"
                Assert-ExpectedFileHash -Path $resource.Destination -ExpectedHash $expectedHash -Label "Installed resource '$($resource.DestinationRelative)'"
            }

            if ($AddToUserPath) {
                $oldUserPathRecord = Get-PersistentUserEnvironmentRecord -Name 'Path'
                $proposedUserPath = Get-ProposedUserPath -ExistingUserPath $oldUserPathRecord.Value -CliDirectory $resolvedBinDirectory
                $proposedUserPathRecord = [pscustomobject]@{
                    Name = 'Path'
                    Exists = $true
                    Value = $proposedUserPath
                    Kind = $oldUserPathRecord.Kind
                }
                $null = Invoke-PersistentUserEnvironmentRecordCas `
                    -Expected $oldUserPathRecord `
                    -Desired $proposedUserPathRecord `
                    -MutationCommitted ([ref]$pathMutationCommitted) `
                    -Broadcast
                if (-not (Test-PersistentUserEnvironmentRecord -Expected $proposedUserPathRecord -Actual (Get-PersistentUserEnvironmentRecord -Name 'Path'))) {
                    throw 'The persisted Windows user PATH differs from the proposed value or registry value kind.'
                }
                $machinePathSnapshot = [Environment]::GetEnvironmentVariable('Path', 'Machine')
                $machinePathSnapshotCaptured = $true
                $projectedPersistentPath = Get-ProjectedPersistentPath `
                    -MachinePath $machinePathSnapshot `
                    -UserPath $proposedUserPath
                $env:PATH = $projectedPersistentPath
            } else {
                $env:PATH = $originalPath
                $existingRayman = @(Get-Command 'rayman' -All -ErrorAction SilentlyContinue)
                if ($existingRayman.Count -eq 0) {
                    $script:processPathOnly = $true
                    $env:PATH = $resolvedBinDirectory + [IO.Path]::PathSeparator + $originalPath
                }
            }

            Invoke-NativeCheckedInDirectory -Directory $doctorWorkspaceRecord.Root -FilePath $destinationCli -Arguments @(
                'workspace', 'activate', '--skill-file', $destinationSkill, '--yes'
            )
            $postVerification = @{
                CliPath = $destinationCli
                ReferenceCliPath = $artifact
                SkillPath = $destinationSkill
                WorkspaceSkillPath = $destinationSkill
                DoctorWorkspace = $doctorWorkspaceRecord.Root
                RequirePath = $true
            }
            & (Join-Path $repoRoot 'scripts/verify-release-contract.ps1') @postVerification

            Assert-ExpectedFileHash -Path $artifact -ExpectedHash $verifiedArtifactHash -Label 'Verified artifact after post-install check'
            Assert-ExpectedFileHash -Path $destinationCli -ExpectedHash $verifiedArtifactHash -Label 'Installed CLI after post-install check'
            foreach ($resource in $skillResources) {
                $expectedHash = $resourceHashes[$resource.DestinationRelative]
                Assert-ExpectedFileHash -Path $resource.Source -ExpectedHash $expectedHash -Label "Verified resource after post-install check '$($resource.SourceRelative)'"
                Assert-ExpectedFileHash -Path $resource.Destination -ExpectedHash $expectedHash -Label "Installed resource after post-install check '$($resource.DestinationRelative)'"
            }
            if ((Get-FileHash -LiteralPath $cargoApplication -Algorithm SHA256).Hash.ToLowerInvariant() -ne $cargoApplicationHash) {
                throw 'Cargo executable identity changed during installation.'
            }
            if ($AddToUserPath) {
                if (-not (Test-PersistentUserEnvironmentRecord -Expected $proposedUserPathRecord -Actual (Get-PersistentUserEnvironmentRecord -Name 'Path'))) {
                    throw 'The persisted Windows user PATH changed during post-install verification.'
                }
                if (-not $machinePathSnapshotCaptured) {
                    throw 'The Windows machine PATH verification snapshot was not captured.'
                }
                $currentMachinePath = [Environment]::GetEnvironmentVariable('Path', 'Machine')
                $machinePathChanged = if ($null -eq $machinePathSnapshot) {
                    $null -ne $currentMachinePath
                } elseif ($null -eq $currentMachinePath) {
                    $true
                } else {
                    -not [string]::Equals(
                        [string]$machinePathSnapshot,
                        [string]$currentMachinePath,
                        [StringComparison]::Ordinal
                    )
                }
                if ($machinePathChanged) {
                    throw 'The Windows machine PATH changed during post-install verification.'
                }
            }

            # Final operation in the rollback domain. The CLI owns binding rollback.
            Invoke-NativeChecked -FilePath $destinationCli -Arguments @(
                'workspace', 'install-bind', '--skill-file', $canonicalSkill, '--yes'
            )
            $coreCommitted = $true
        } catch {
            $installFailure = $_.Exception.Message
            if ($coreCommitted) {
                throw "Core installation committed; refusing rollback after install-bind: $installFailure"
            }
            $recoveryErrors = @()
            if ($pathMutationCommitted) {
                try {
                    Restore-PersistentUserEnvironmentRecordCas -Original $oldUserPathRecord -Published $proposedUserPathRecord -Broadcast
                } catch {
                    $recoveryErrors += "unable to CAS-restore Windows user PATH: $($_.Exception.Message)"
                }
            }
            $recoveryErrors += @(Invoke-InstallRollback -InstallRecords $installed)
            $rollbackRetained = @($installed | ForEach-Object {
                @(Get-RetainedInstallLedgerEntry -Ledger $_.PathBindingLedger)
            })
            if ($rollbackRetained.Count -gt 0) {
                $recoveryErrors += @($rollbackRetained | ForEach-Object {
                    "Retained path-binding evidence: $($_.ObservedPath) role=$($_.Role) state=$($_.State) identity=$($_.ObservedPlatformIdentity) hash=$($_.ObservedContentHash) ($($_.Reason))"
                })
            }
            if ($recoveryErrors.Count -gt 0) {
                $nl = [Environment]::NewLine
                throw ("Installation failed: {0}{1}Rollback was incomplete; retained backup/current/PATH state requires review:{1}{2}" -f $installFailure, $nl, ($recoveryErrors -join $nl))
            }
            throw $installFailure
        }

        # Core is committed: cleanup warns only; Hook is an independent domain.
        foreach ($record in $installed) {
            $cleanupWarning = Remove-CommittedBackup -InstallRecord $record
            if (-not [string]::IsNullOrWhiteSpace($cleanupWarning)) {
                Write-Warning $cleanupWarning -WarningAction Continue
            }
        }
        $retainedEvidenceReport = @($installed | ForEach-Object {
            @(Get-RetainedInstallLedgerEntry -Ledger $_.PathBindingLedger)
        })
        if (-not $SkipCodexStopHook) {
            try {
                $hookInstallReport = Invoke-NativeJsonChecked -FilePath $destinationCli -Arguments @(
                    '--format', 'json', 'codex-hook', 'install', '--yes'
                )
                $hookStatusReport = Invoke-NativeJsonChecked -FilePath $destinationCli -Arguments @(
                    '--format', 'json', 'codex-hook', 'status'
                )
                Assert-HookInstallationReports -InstallReport $hookInstallReport -StatusReport $hookStatusReport
                $hookInstalled = $true
            } catch {
                $hookFailure = $_.Exception.Message
            }
        }
    } finally {
        Pop-Location
    }
} finally {
    $env:PATH = $originalPath
    if ($null -ne $doctorWorkspaceRecord) {
        $doctorCleanupWarning = Remove-TemporaryDoctorWorkspace -Record $doctorWorkspaceRecord
        if (-not [string]::IsNullOrWhiteSpace($doctorCleanupWarning)) {
            Write-Warning $doctorCleanupWarning -WarningAction Continue
        }
    }
}

Write-Host 'RaymanCodingSkill core installation verified and committed.'
Write-Host "  CLI: $destinationCli"
foreach ($resource in $skillResources) {
    Write-Host "  Codex resource ($($resource.DestinationRelative)): $($resource.Destination)"
}
if ($retainedEvidenceReport.Count -gt 0) {
    Write-Warning "  Identity-bound CAS retained $($retainedEvidenceReport.Count) auditable path(s); retained evidence is not auto-deleted." -WarningAction Continue
    Write-Warning '  Retained paths may hard-link the live installed inode: do not edit them; remove only after reviewing the complete ledger under explicit cleanup authority.' -WarningAction Continue
    foreach ($evidence in $retainedEvidenceReport) {
        Write-Warning "    retained: path=$($evidence.ObservedPath) leaf=$($evidence.Leaf) role=$($evidence.Role) state=$($evidence.State) verification=$($evidence.VerificationStatus) identity=$($evidence.ObservedPlatformIdentity) hash=$($evidence.ObservedContentHash) windows_attributes=$($evidence.ObservedWindowsAttributes) windows_security=$($evidence.ObservedWindowsSecurityDescriptorHash) linux_mode=$($evidence.ObservedLinuxMode) linux_uid=$($evidence.ObservedLinuxUid) linux_gid=$($evidence.ObservedLinuxGid) linux_size=$($evidence.ObservedLinuxSize) linux_xattr=$($evidence.ObservedLinuxXattrMetadataHash) reason=$($evidence.Reason)" -WarningAction Continue
    }
}
if ($SkipCodexStopHook) {
    Write-Host '  Codex Stop guard: skipped by explicit request'
} elseif ($hookInstalled) {
    Write-Host '  Codex Stop guard: installed; exact install/status reports matched'
} else {
    Write-Warning '  Codex Stop guard: core stayed committed, but Hook setup failed.' -WarningAction Continue
}
if ($AddToUserPath) {
    Write-Host "  Persistent user PATH: verified with '$resolvedBinDirectory' first in the user segment"
} elseif ($script:processPathOnly) {
    Write-Host "  Persistent PATH: unchanged. Nothing resolved 'rayman' in this shell, so the destination was made resolvable for verification only."
    Write-Host "  ACTION: open a new terminal, or prepend '$resolvedBinDirectory' to this process PATH."
} else {
    Write-Host '  Persistent PATH: unchanged; current effective PATH identity was verified'
}
Write-Host '  Existing workspace activations: not scanned or automatically rebound'
Write-Host '  ACTION in each existing workspace: run rayman workspace status'
Write-Host '  If its complete enabled raymancodingskill binding reports only identity drift,'
Write-Host '  run rayman workspace rebind --yes there; rebind preserves skill_file.'

if (-not [string]::IsNullOrWhiteSpace($hookFailure)) {
    $quotedCli = $destinationCli.Replace("'", "''")
    $nl = [Environment]::NewLine
    throw ("Rayman core installation is committed, but Codex Stop guard setup failed: {0}{1}Retry: & '{2}' --format json codex-hook install --yes{1}Verify: & '{2}' --format json codex-hook status" -f $hookFailure, $nl, $quotedCli)
}
