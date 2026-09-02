[CmdletBinding(DefaultParameterSetName = 'Invoke')]
param(
    [Parameter(Mandatory = $true, ParameterSetName = 'Invoke')]
    [ValidateSet('identity_probe', 'git_local_commit_v1')]
    [string]$Operation,

    [Parameter(ParameterSetName = 'Invoke')]
    [ValidateLength(1, 200)]
    [string]$CommitMessage,

    [Parameter(ParameterSetName = 'Invoke')]
    [switch]$Yes,

    [Parameter(ParameterSetName = 'Invoke')]
    [ValidateRange(1, 300)]
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

$script:SchemaVersion = 3
$script:MaxRequestBytes = 256KB
$script:MaxClockSkewSeconds = 30
$script:MaxLifetimeSeconds = 120
$script:HeartbeatMaximumAgeSeconds = 15
$script:ReceiptName = 'install-receipt.json'
$script:HeartbeatName = 'heartbeat.json'
$script:RequestSuffix = '.request.json'
$script:ResultSuffix = '.result.json'
$script:ResultWriteProbeName = '.codex-sandbox-result-write-probe'
$script:GitCapabilityId = 'git_local_commit_v1'
$script:GitCapabilityManifestName = 'git-local-commit-v1.json'
$script:GitCapabilityReadyName = 'git-local-commit-v1.ready.json'
$script:GitTransactionDirectoryName = 'git-local-commit-transactions'
$script:GitSelfTestFaultPhase = $null
$script:GitSelfTestBeforeAddAction = $null

function Invoke-BrokerPortableStaticSelfTest {
    $parseErrors = $null
    [void][Management.Automation.Language.Parser]::ParseFile(
        $PSCommandPath, [ref]$null, [ref]$parseErrors
    )
    $source = [IO.File]::ReadAllText($PSCommandPath)
    foreach ($required in @(
        'function Invoke-BrokerClient',
        'function Assert-InstalledTaskContract',
        'function Invoke-GitLocalCommit',
        'Arbitrary commands are never accepted.'
    )) {
        if (-not $source.Contains($required, [StringComparison]::Ordinal)) {
            throw "Non-Windows static broker self-test lost required contract: $required"
        }
    }
    if (@($parseErrors).Count -ne 0) {
        throw 'Non-Windows static broker self-test found parse errors.'
    }
}

if (-not $IsWindows) {
    if (-not $SelfTest) {
        throw 'Codex PowerShell identity broker is supported only on Windows.'
    }
    Invoke-BrokerPortableStaticSelfTest
    Write-Host 'codex-powershell-broker.ps1 self-test passed.'
    return
}

if (-not ('Rayman.CodexBrokerNative' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.ComponentModel;
using System.IO;
using System.Runtime.InteropServices;
using System.Text;
using System.Text.Json;
using System.Threading.Tasks;
using Microsoft.Win32.SafeHandles;

namespace Rayman {
    public sealed class FixedProcessResult {
        public int ExitCode { get; set; }
        public string StandardOutput { get; set; }
        public string StandardError { get; set; }
    }

    public sealed class HeldRegularFile : IDisposable {
        public byte[] Bytes { get; private set; }
        FileStream stream;
        List<SafeFileHandle> ancestors;
        public HeldRegularFile(byte[] bytes, FileStream heldStream,
            List<SafeFileHandle> heldAncestors) {
            Bytes = bytes;
            stream = heldStream;
            ancestors = heldAncestors;
        }
        public void Dispose() {
            if (stream != null) { stream.Dispose(); stream = null; }
            if (ancestors != null) {
                for (int i = ancestors.Count - 1; i >= 0; i--)
                    ancestors[i].Dispose();
                ancestors = null;
            }
        }
    }

    public static class CodexBrokerNative {
        const uint HANDLE_FLAG_INHERIT = 0x00000001;
        const uint STARTF_USESTDHANDLES = 0x00000100;
        const uint CREATE_SUSPENDED = 0x00000004;
        const uint CREATE_NO_WINDOW = 0x08000000;
        const uint CREATE_UNICODE_ENVIRONMENT = 0x00000400;
        const uint WAIT_OBJECT_0 = 0;
        const uint WAIT_TIMEOUT = 258;
        const uint INFINITE = 0xffffffff;
        const uint JOB_OBJECT_LIMIT_ACTIVE_PROCESS = 0x00000008;
        const uint JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE = 0x00002000;
        const int JobObjectExtendedLimitInformation = 9;

        [StructLayout(LayoutKind.Sequential)]
        struct SECURITY_ATTRIBUTES {
            public int nLength;
            public IntPtr lpSecurityDescriptor;
            [MarshalAs(UnmanagedType.Bool)] public bool bInheritHandle;
        }

        [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
        struct STARTUPINFO {
            public int cb;
            public string lpReserved;
            public string lpDesktop;
            public string lpTitle;
            public uint dwX;
            public uint dwY;
            public uint dwXSize;
            public uint dwYSize;
            public uint dwXCountChars;
            public uint dwYCountChars;
            public uint dwFillAttribute;
            public uint dwFlags;
            public short wShowWindow;
            public short cbReserved2;
            public IntPtr lpReserved2;
            public IntPtr hStdInput;
            public IntPtr hStdOutput;
            public IntPtr hStdError;
        }

        [StructLayout(LayoutKind.Sequential)]
        struct PROCESS_INFORMATION {
            public IntPtr hProcess;
            public IntPtr hThread;
            public uint dwProcessId;
            public uint dwThreadId;
        }

        [StructLayout(LayoutKind.Sequential)]
        struct JOBOBJECT_BASIC_LIMIT_INFORMATION {
            public long PerProcessUserTimeLimit;
            public long PerJobUserTimeLimit;
            public uint LimitFlags;
            public UIntPtr MinimumWorkingSetSize;
            public UIntPtr MaximumWorkingSetSize;
            public uint ActiveProcessLimit;
            public UIntPtr Affinity;
            public uint PriorityClass;
            public uint SchedulingClass;
        }

        [StructLayout(LayoutKind.Sequential)]
        struct IO_COUNTERS {
            public ulong ReadOperationCount;
            public ulong WriteOperationCount;
            public ulong OtherOperationCount;
            public ulong ReadTransferCount;
            public ulong WriteTransferCount;
            public ulong OtherTransferCount;
        }

        [StructLayout(LayoutKind.Sequential)]
        struct JOBOBJECT_EXTENDED_LIMIT_INFORMATION {
            public JOBOBJECT_BASIC_LIMIT_INFORMATION BasicLimitInformation;
            public IO_COUNTERS IoInfo;
            public UIntPtr ProcessMemoryLimit;
            public UIntPtr JobMemoryLimit;
            public UIntPtr PeakProcessMemoryUsed;
            public UIntPtr PeakJobMemoryUsed;
        }

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
        public static extern SafeFileHandle CreateFileW(
            string path,
            uint desiredAccess,
            uint shareMode,
            IntPtr securityAttributes,
            uint creationDisposition,
            uint flagsAndAttributes,
            IntPtr templateFile);

        [DllImport("kernel32.dll", SetLastError = true)]
        static extern bool GetFileInformationByHandleEx(
            SafeFileHandle hFile, int fileInformationClass,
            out FILE_ID_INFO fileInformation, uint dwBufferSize);

        [DllImport("kernel32.dll", SetLastError = true)]
        static extern bool GetFileInformationByHandleEx(
            SafeFileHandle hFile, int fileInformationClass,
            out FILE_ATTRIBUTE_TAG_INFO fileInformation, uint dwBufferSize);

        [DllImport("kernel32.dll", SetLastError = true)]
        static extern bool CreatePipe(
            out IntPtr readPipe, out IntPtr writePipe,
            ref SECURITY_ATTRIBUTES attributes, int size);

        [DllImport("kernel32.dll", SetLastError = true)]
        static extern bool SetHandleInformation(IntPtr handle, uint mask, uint flags);

        [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
        static extern bool CreateProcessW(
            string applicationName, StringBuilder commandLine,
            IntPtr processAttributes, IntPtr threadAttributes,
            bool inheritHandles, uint creationFlags, IntPtr environment,
            string currentDirectory, ref STARTUPINFO startupInfo,
            out PROCESS_INFORMATION processInformation);

        [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
        static extern IntPtr CreateJobObjectW(IntPtr attributes, string name);

        [DllImport("kernel32.dll", SetLastError = true)]
        static extern bool SetInformationJobObject(
            IntPtr job, int infoClass, IntPtr info, uint length);

        [DllImport("kernel32.dll", SetLastError = true)]
        static extern bool AssignProcessToJobObject(IntPtr job, IntPtr process);

        [DllImport("kernel32.dll", SetLastError = true)]
        static extern uint ResumeThread(IntPtr thread);

        [DllImport("kernel32.dll", SetLastError = true)]
        static extern uint WaitForSingleObject(IntPtr handle, uint milliseconds);

        [DllImport("kernel32.dll", SetLastError = true)]
        static extern bool GetExitCodeProcess(IntPtr process, out uint exitCode);

        [DllImport("kernel32.dll", SetLastError = true)]
        static extern bool TerminateJobObject(IntPtr job, uint exitCode);

        [DllImport("kernel32.dll", SetLastError = true)]
        static extern bool TerminateProcess(IntPtr process, uint exitCode);

        [DllImport("kernel32.dll", SetLastError = true)]
        static extern bool CloseHandle(IntPtr handle);

        static void ThrowLast(string message) {
            throw new Win32Exception(Marshal.GetLastWin32Error(), message);
        }

        static string QuoteArgument(string value) {
            if (value.IndexOf('\0') >= 0) throw new ArgumentException("argv contains NUL");
            if (value.Length > 0 && value.IndexOfAny(new[] { ' ', '\t', '\n', '\v', '"' }) < 0)
                return value;
            var result = new StringBuilder("\"");
            int slashes = 0;
            foreach (char ch in value) {
                if (ch == '\\') { slashes++; continue; }
                if (ch == '"') {
                    result.Append('\\', slashes * 2 + 1).Append('"');
                    slashes = 0;
                    continue;
                }
                result.Append('\\', slashes).Append(ch);
                slashes = 0;
            }
            result.Append('\\', slashes * 2).Append('"');
            return result.ToString();
        }

        static byte[] ReadBounded(Stream stream, int maximumBytes) {
            using (stream) {
                using (var output = new MemoryStream()) {
                    var buffer = new byte[8192];
                    while (true) {
                        int read = stream.Read(buffer, 0, buffer.Length);
                        if (read == 0) break;
                        if (output.Length + read > maximumBytes)
                            throw new InvalidDataException("fixed process output exceeded its bound");
                        output.Write(buffer, 0, read);
                    }
                    return output.ToArray();
                }
            }
        }

        static IntPtr BuildEnvironment(IDictionary<string,string> environment) {
            var keys = new List<string>(environment.Keys);
            keys.Sort(StringComparer.OrdinalIgnoreCase);
            var text = new StringBuilder();
            string previous = null;
            foreach (string key in keys) {
                if (String.IsNullOrEmpty(key) || key.IndexOf('=') >= 0 || key.IndexOf('\0') >= 0)
                    throw new ArgumentException("invalid environment key");
                if (previous != null && StringComparer.OrdinalIgnoreCase.Equals(previous, key))
                    throw new ArgumentException("duplicate environment key");
                string value = environment[key] ?? String.Empty;
                if (value.IndexOf('\0') >= 0) throw new ArgumentException("invalid environment value");
                text.Append(key).Append('=').Append(value).Append('\0');
                previous = key;
            }
            text.Append('\0');
            return Marshal.StringToHGlobalUni(text.ToString());
        }

        static string GetStrongFileIdentity(SafeFileHandle handle) {
            FILE_ID_INFO info;
            if (!GetFileInformationByHandleEx(handle, 18, out info,
                (uint)Marshal.SizeOf<FILE_ID_INFO>()))
                ThrowLast("cannot read strong file identity");
            return info.VolumeSerialNumber.ToString("x16") + ":" +
                BitConverter.ToString(info.FileId.Identifier).Replace("-", "").ToLowerInvariant();
        }

        static SafeFileHandle OpenNonReparseHeld(string path, bool directory,
            uint desiredAccess, uint shareMode) {
            const uint OPEN_EXISTING = 3;
            const uint FILE_FLAG_BACKUP_SEMANTICS = 0x02000000;
            const uint FILE_FLAG_OPEN_REPARSE_POINT = 0x00200000;
            const uint FILE_ATTRIBUTE_DIRECTORY = 0x00000010;
            const uint FILE_ATTRIBUTE_REPARSE_POINT = 0x00000400;
            var handle = CreateFileW(path, desiredAccess, shareMode, IntPtr.Zero,
                OPEN_EXISTING, FILE_FLAG_OPEN_REPARSE_POINT |
                (directory ? FILE_FLAG_BACKUP_SEMANTICS : 0), IntPtr.Zero);
            if (handle.IsInvalid) {
                handle.Dispose();
                ThrowLast("cannot open held non-reparse path");
            }
            try {
                FILE_ATTRIBUTE_TAG_INFO info;
                if (!GetFileInformationByHandleEx(handle, 9, out info,
                    (uint)Marshal.SizeOf<FILE_ATTRIBUTE_TAG_INFO>()))
                    ThrowLast("cannot read held path attributes");
                bool isDirectory = (info.FileAttributes & FILE_ATTRIBUTE_DIRECTORY) != 0;
                if ((info.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT) != 0 ||
                    isDirectory != directory)
                    throw new InvalidDataException(
                        "held path is reparse-backed or has the wrong type: " + path);
                return handle;
            } catch {
                handle.Dispose();
                throw;
            }
        }

        public static string GetStrongFileIdentity(string path, bool directory) {
            const uint GENERIC_READ = 0x80000000;
            const uint FILE_SHARE_READ = 0x00000001;
            const uint FILE_SHARE_WRITE = 0x00000002;
            const uint FILE_SHARE_DELETE = 0x00000004;
            const uint OPEN_EXISTING = 3;
            const uint FILE_FLAG_BACKUP_SEMANTICS = 0x02000000;
            const uint FILE_FLAG_OPEN_REPARSE_POINT = 0x00200000;
            using (var handle = CreateFileW(path, GENERIC_READ,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                IntPtr.Zero, OPEN_EXISTING,
                FILE_FLAG_OPEN_REPARSE_POINT | (directory ? FILE_FLAG_BACKUP_SEMANTICS : 0),
                IntPtr.Zero)) {
                if (handle.IsInvalid) ThrowLast("cannot open strong-identity target");
                return GetStrongFileIdentity(handle);
            }
        }

        public static HeldRegularFile ReadBoundedRegularFileTreeHeld(
            string repositoryRoot, string expectedRootIdentity,
            string absolutePath, long maximumBytes) {
            const uint GENERIC_READ = 0x80000000;
            const uint FILE_READ_ATTRIBUTES = 0x00000080;
            const uint FILE_SHARE_READ = 0x00000001;
            const uint FILE_SHARE_WRITE = 0x00000002;
            if (maximumBytes < 0 || maximumBytes > Int32.MaxValue)
                throw new ArgumentOutOfRangeException("maximumBytes");
            string root = Path.GetFullPath(repositoryRoot)
                .TrimEnd(Path.DirectorySeparatorChar, Path.AltDirectorySeparatorChar);
            string target = Path.GetFullPath(absolutePath);
            if (!target.StartsWith(root + Path.DirectorySeparatorChar,
                StringComparison.OrdinalIgnoreCase))
                throw new InvalidDataException("held file escaped repository root");
            string relative = target.Substring(root.Length + 1)
                .Replace(Path.AltDirectorySeparatorChar, Path.DirectorySeparatorChar);
            string[] segments = relative.Split(new[] { Path.DirectorySeparatorChar },
                StringSplitOptions.RemoveEmptyEntries);
            if (segments.Length == 0)
                throw new InvalidDataException("held file path has no relative leaf");

            var ancestors = new List<SafeFileHandle>();
            SafeFileHandle leaf = null;
            FileStream stream = null;
            try {
                var rootHandle = OpenNonReparseHeld(root, true,
                    FILE_READ_ATTRIBUTES, FILE_SHARE_READ | FILE_SHARE_WRITE);
                ancestors.Add(rootHandle);
                if (!StringComparer.Ordinal.Equals(
                    GetStrongFileIdentity(rootHandle), expectedRootIdentity))
                    throw new InvalidDataException(
                        "held repository root identity drifted");
                string current = root;
                for (int i = 0; i < segments.Length - 1; i++) {
                    current = Path.Combine(current, segments[i]);
                    ancestors.Add(OpenNonReparseHeld(current, true,
                        FILE_READ_ATTRIBUTES, FILE_SHARE_READ | FILE_SHARE_WRITE));
                }
                leaf = OpenNonReparseHeld(target, false, GENERIC_READ,
                    FILE_SHARE_READ);
                stream = new FileStream(leaf, FileAccess.Read);
                leaf = null;
                if (stream.Length > maximumBytes)
                    throw new InvalidDataException(
                        "held regular file exceeded its byte bound");
                var bytes = new byte[(int)stream.Length];
                int offset = 0;
                while (offset < bytes.Length) {
                    int read = stream.Read(bytes, offset, bytes.Length - offset);
                    if (read <= 0)
                        throw new EndOfStreamException(
                            "held regular file shortened while reading");
                    offset += read;
                }
                return new HeldRegularFile(bytes, stream, ancestors);
            } catch {
                if (stream != null) stream.Dispose();
                if (leaf != null) leaf.Dispose();
                for (int i = ancestors.Count - 1; i >= 0; i--)
                    ancestors[i].Dispose();
                throw;
            }
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
                        throw new InvalidDataException("duplicate or case-colliding JSON property at " + path);
                    ValidateElement(property.Value, path + "." + property.Name, depth + 1);
                }
            } else if (element.ValueKind == JsonValueKind.Array) {
                int index = 0;
                foreach (var item in element.EnumerateArray())
                    ValidateElement(item, path + "[" + (index++) + "]", depth + 1);
            }
        }

        public static FixedProcessResult RunSingleProcess(
            string application, string[] arguments, string currentDirectory,
            IDictionary<string,string> environment, byte[] standardInput,
            int timeoutMilliseconds, int maximumOutputBytes) {
            if (timeoutMilliseconds < 1 || maximumOutputBytes < 1)
                throw new ArgumentOutOfRangeException();
            var security = new SECURITY_ATTRIBUTES {
                nLength = Marshal.SizeOf<SECURITY_ATTRIBUTES>(),
                bInheritHandle = true
            };
            IntPtr stdoutRead = IntPtr.Zero, stdoutWrite = IntPtr.Zero;
            IntPtr stderrRead = IntPtr.Zero, stderrWrite = IntPtr.Zero;
            IntPtr stdinRead = IntPtr.Zero, stdinWrite = IntPtr.Zero;
            IntPtr job = IntPtr.Zero, environmentBlock = IntPtr.Zero;
            var process = new PROCESS_INFORMATION();
            bool assignedToJob = false;
            try {
                if (!CreatePipe(out stdoutRead, out stdoutWrite, ref security, 0) ||
                    !CreatePipe(out stderrRead, out stderrWrite, ref security, 0) ||
                    !CreatePipe(out stdinRead, out stdinWrite, ref security, 0))
                    ThrowLast("cannot create fixed process pipes");
                if (!SetHandleInformation(stdoutRead, HANDLE_FLAG_INHERIT, 0) ||
                    !SetHandleInformation(stderrRead, HANDLE_FLAG_INHERIT, 0) ||
                    !SetHandleInformation(stdinWrite, HANDLE_FLAG_INHERIT, 0))
                    ThrowLast("cannot protect parent pipe handles");

                var command = new StringBuilder(QuoteArgument(application));
                foreach (string argument in arguments)
                    command.Append(' ').Append(QuoteArgument(argument));
                environmentBlock = BuildEnvironment(environment);
                var startup = new STARTUPINFO {
                    cb = Marshal.SizeOf<STARTUPINFO>(),
                    dwFlags = STARTF_USESTDHANDLES,
                    hStdInput = stdinRead,
                    hStdOutput = stdoutWrite,
                    hStdError = stderrWrite
                };
                if (!CreateProcessW(application, command, IntPtr.Zero, IntPtr.Zero, true,
                    CREATE_SUSPENDED | CREATE_NO_WINDOW | CREATE_UNICODE_ENVIRONMENT,
                    environmentBlock, currentDirectory, ref startup, out process))
                    ThrowLast("cannot create fixed Git process suspended");

                job = CreateJobObjectW(IntPtr.Zero, null);
                if (job == IntPtr.Zero) ThrowLast("cannot create fixed Git job");
                var limits = new JOBOBJECT_EXTENDED_LIMIT_INFORMATION();
                limits.BasicLimitInformation.LimitFlags =
                    JOB_OBJECT_LIMIT_ACTIVE_PROCESS | JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                limits.BasicLimitInformation.ActiveProcessLimit = 1;
                IntPtr limitsPointer = Marshal.AllocHGlobal(
                    Marshal.SizeOf<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>());
                try {
                    Marshal.StructureToPtr(limits, limitsPointer, false);
                    if (!SetInformationJobObject(job, JobObjectExtendedLimitInformation,
                        limitsPointer,
                        (uint)Marshal.SizeOf<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>()))
                        ThrowLast("cannot set fixed Git job limits");
                } finally { Marshal.FreeHGlobal(limitsPointer); }
                if (!AssignProcessToJobObject(job, process.hProcess))
                    ThrowLast("cannot assign Git before first instruction");
                assignedToJob = true;

                CloseHandle(stdoutWrite); stdoutWrite = IntPtr.Zero;
                CloseHandle(stderrWrite); stderrWrite = IntPtr.Zero;
                CloseHandle(stdinRead); stdinRead = IntPtr.Zero;
                if (ResumeThread(process.hThread) == 0xffffffff)
                    ThrowLast("cannot resume fixed Git process");

                var stdoutHandle = new SafeFileHandle(stdoutRead, true); stdoutRead = IntPtr.Zero;
                var stderrHandle = new SafeFileHandle(stderrRead, true); stderrRead = IntPtr.Zero;
                var stdinHandle = new SafeFileHandle(stdinWrite, true); stdinWrite = IntPtr.Zero;
                var stdoutTask = Task.Run(() => ReadBounded(
                    new FileStream(stdoutHandle, FileAccess.Read), maximumOutputBytes));
                var stderrTask = Task.Run(() => ReadBounded(
                    new FileStream(stderrHandle, FileAccess.Read), maximumOutputBytes));
                using (var input = new FileStream(stdinHandle, FileAccess.Write)) {
                    if (standardInput != null && standardInput.Length > 0)
                        input.Write(standardInput, 0, standardInput.Length);
                }

                uint wait = WaitForSingleObject(process.hProcess, (uint)timeoutMilliseconds);
                if (wait == WAIT_TIMEOUT) {
                    TerminateJobObject(job, 1460);
                    WaitForSingleObject(process.hProcess, INFINITE);
                    throw new TimeoutException("fixed Git process timed out");
                }
                if (wait != WAIT_OBJECT_0) ThrowLast("cannot wait for fixed Git process");
                uint exitCode;
                if (!GetExitCodeProcess(process.hProcess, out exitCode))
                    ThrowLast("cannot read fixed Git exit code");
                Task.WaitAll(stdoutTask, stderrTask);
                var utf8 = new UTF8Encoding(false, true);
                return new FixedProcessResult {
                    ExitCode = unchecked((int)exitCode),
                    StandardOutput = utf8.GetString(stdoutTask.Result),
                    StandardError = utf8.GetString(stderrTask.Result)
                };
            } finally {
                if (environmentBlock != IntPtr.Zero) Marshal.FreeHGlobal(environmentBlock);
                if (process.hProcess != IntPtr.Zero) {
                    uint finalExitCode;
                    bool stillActive = !GetExitCodeProcess(
                        process.hProcess, out finalExitCode) || finalExitCode == 259;
                    if (stillActive) {
                        bool terminated = assignedToJob && job != IntPtr.Zero
                            ? TerminateJobObject(job, 1)
                            : TerminateProcess(process.hProcess, 1);
                        if (terminated)
                            WaitForSingleObject(process.hProcess, INFINITE);
                    }
                }
                if (process.hThread != IntPtr.Zero) CloseHandle(process.hThread);
                if (process.hProcess != IntPtr.Zero) CloseHandle(process.hProcess);
                if (job != IntPtr.Zero) CloseHandle(job);
                foreach (IntPtr handle in new[] { stdoutRead, stdoutWrite, stderrRead,
                    stderrWrite, stdinRead, stdinWrite })
                    if (handle != IntPtr.Zero) CloseHandle(handle);
            }
        }
    }
}
'@
}

function Get-BytesSha256 {
    param([Parameter(Mandatory = $true)][AllowEmptyCollection()][byte[]]$Bytes)

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
        [switch]$PassThruHash,
        [Security.AccessControl.FileSecurity]$Security
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
        [Rayman.CodexBrokerNative]::ValidateUniqueJsonProperties($Bytes)
    } catch {
        throw "$Label has duplicate, case-colliding, or structurally invalid JSON properties: $($_.Exception.Message)"
    }
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

function Get-OrdinalSortedStrings {
    param(
        [Parameter(Mandatory = $true)]
        [AllowEmptyCollection()]
        [string[]]$Values
    )

    $sorted = [string[]]@($Values)
    [Array]::Sort($sorted, [StringComparer]::Ordinal)
    return $sorted
}

function Assert-ExactProperties {
    param(
        [Parameter(Mandatory = $true)]$Document,
        [Parameter(Mandatory = $true)][AllowEmptyCollection()][string[]]$Expected,
        [Parameter(Mandatory = $true)][string]$Label
    )

    if ($Document -isnot [pscustomobject]) { throw "$Label must be a JSON object." }
    $actual = @(Get-OrdinalSortedStrings -Values @(
        $Document.PSObject.Properties | ForEach-Object { $_.Name }
    ))
    $wanted = @(Get-OrdinalSortedStrings -Values $Expected)
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
        @($Paths.Transactions, $readOnly, 'Git transaction root'),
        @($Paths.Hooks, $readOnly, 'Protected empty hooks root'),
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
        @($Paths.GitCapabilityManifest, 'Git capability manifest'),
        @([string]$Receipt.worker_path, 'Installed worker'),
        @($Paths.WorkerLock, 'Worker lock'),
        @($Paths.GitTransactionLock, 'Git transaction lock')
    )) {
        [void](Assert-NoReparseAncestors -Path $entry[0] `
            -Label $entry[1] -RequireLeaf)
        Assert-ExactSecurity -Path $entry[0] -Expected $file `
            -ExpectedOwnerSid ([string]$Receipt.user_sid) -Label $entry[1]
    }
    if (Test-Path -LiteralPath $Paths.GitCapabilityReady -PathType Leaf) {
        [void](Assert-NoReparseAncestors -Path $Paths.GitCapabilityReady `
            -Label 'Git capability ready marker' -RequireLeaf)
        Assert-ExactSecurity -Path $Paths.GitCapabilityReady -Expected $file `
            -ExpectedOwnerSid ([string]$Receipt.user_sid) `
            -Label 'Git capability ready marker'
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
        Transactions = Join-Path $installFull $script:GitTransactionDirectoryName
        Hooks = Join-Path $installFull 'empty-hooks'
        Receipt = Join-Path $installFull $script:ReceiptName
        GitCapabilityManifest = Join-Path $installFull $script:GitCapabilityManifestName
        GitCapabilityReady = Join-Path $installFull $script:GitCapabilityReadyName
        Heartbeat = Join-Path (Join-Path $installFull 'results') $script:HeartbeatName
        WorkerLock = Join-Path $installFull 'worker.lock'
        GitTransactionLock = Join-Path `
            (Join-Path $installFull $script:GitTransactionDirectoryName) `
            'transaction.lock'
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
        'git_capability_manifest_path', 'git_capability_manifest_sha256',
        'powershell_path', 'powershell_sha256', 'request_root', 'result_root',
        'sandbox_group', 'sandbox_group_sid', 'schema_version', 'task_name',
        'user_account', 'user_sid', 'worker_path', 'worker_sha256'
    )
    if ($receipt.schema_version -ne $script:SchemaVersion -or
        $receipt.capabilities -isnot [array] -or
        @($receipt.capabilities).Count -ne 2 -or
        [string]$receipt.capabilities[0] -cne 'identity_probe' -or
        [string]$receipt.capabilities[1] -cne $script:GitCapabilityId) {
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
    $capabilityPath = Get-NormalizedAbsolutePath `
        -Path ([string]$receipt.git_capability_manifest_path) `
        -Label 'Git capability manifest'
    if ($capabilityPath -cne $Paths.GitCapabilityManifest -or
        [string]$receipt.git_capability_manifest_sha256 -notmatch '^[0-9a-f]{64}$') {
        throw 'Broker install receipt is bound to a different Git capability manifest.'
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
    [void](Assert-NoReparseAncestors -Path $capabilityPath `
        -Label 'Git capability manifest' -RequireLeaf)
    if ((Get-FileSha256 -Path $capabilityPath) -cne
        [string]$receipt.git_capability_manifest_sha256) {
        throw 'Git capability manifest hash does not match its protected receipt.'
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

function Get-StrongPathIdentity {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][bool]$Directory
    )

    return [Rayman.CodexBrokerNative]::GetStrongFileIdentity($Path, $Directory)
}

function Assert-GitCommitMessage {
    param([Parameter(Mandatory = $true)][string]$Message)

    if ($Message.Length -lt 1 -or $Message.Length -gt 200 -or
        $Message.Contains([char]0) -or $Message.Contains("`r") -or
        $Message.Contains("`n") -or $Message.Trim() -cne $Message) {
        throw 'git_local_commit_v1 commit message must be one trimmed UTF-8 line of 1-200 characters.'
    }
    return $Message.Normalize([Text.NormalizationForm]::FormC)
}

function Assert-SafeGitRelativePath {
    param([Parameter(Mandatory = $true)][string]$Path)

    if ([string]::IsNullOrWhiteSpace($Path) -or $Path.Contains([char]0) -or
        $Path.Contains('\') -or $Path.StartsWith('/') -or $Path.Contains(':') -or
        $Path.Normalize([Text.NormalizationForm]::FormC) -cne $Path) {
        throw "git_local_commit_v1 path is not canonical repository-relative UTF-8: $Path"
    }
    $segments = @($Path.Split('/'))
    $reserved = '^(?i:con|prn|aux|nul|com[1-9]|lpt[1-9])(?:\..*)?$'
    foreach ($segment in $segments) {
        if ([string]::IsNullOrEmpty($segment) -or $segment -ceq '.' -or
            $segment -ceq '..' -or $segment -ieq '.git' -or
            $segment.EndsWith(' ') -or $segment.EndsWith('.') -or
            $segment -match $reserved) {
            throw "git_local_commit_v1 path contains a forbidden Windows/Git segment: $Path"
        }
    }
    return $Path
}

function Get-GitBlobOid {
    param([Parameter(Mandatory = $true)][byte[]]$Bytes)

    $hash = [Security.Cryptography.IncrementalHash]::CreateHash(
        [Security.Cryptography.HashAlgorithmName]::SHA1
    )
    try {
        $header = [Text.Encoding]::ASCII.GetBytes("blob $($Bytes.Length)`0")
        $hash.AppendData($header)
        $hash.AppendData($Bytes)
        return [Convert]::ToHexString($hash.GetHashAndReset()).ToLowerInvariant()
    } finally { $hash.Dispose() }
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
            throw 'git_local_commit_v1 rejects malformed, commented, or continuation local Git config.'
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

function Read-GitCapabilityManifest {
    param(
        [Parameter(Mandatory = $true)]$Paths,
        [Parameter(Mandatory = $true)]$Receipt
    )

    $manifest = Read-StrictJsonDocument `
        -Path $Paths.GitCapabilityManifest -MaximumBytes 64KB `
        -Label 'git_local_commit_v1 capability manifest'
    Assert-ExactProperties -Document $manifest `
        -Label 'git_local_commit_v1 capability manifest' -Expected @(
        'allow_pre_staged', 'allow_untracked', 'allowed_ref', 'author_email',
        'author_name', 'authorization_mode', 'capability_id', 'created_at_utc',
        'git_config_sha256', 'git_dir', 'git_dir_identity',
        'git_executable_identity', 'git_executable_path', 'git_executable_sha256',
        'git_signer_subject', 'git_signer_thumbprint',
        'gitattributes_sha256', 'hooks_root', 'info_attributes_sha256',
        'info_exclude_sha256', 'install_id', 'object_format', 'repository_id',
        'repository_root', 'repository_root_identity', 'schema_version',
        'tracked_only', 'transactions_root', 'local_commit_only', 'push_allowed',
        'confirmation_required'
    )
    if ([int]$manifest.schema_version -ne 1 -or
        [string]$manifest.capability_id -cne $script:GitCapabilityId -or
        [string]$manifest.install_id -cne [string]$Receipt.install_id -or
        [string]$manifest.authorization_mode -cne 'persistent_install_grant' -or
        [string]$manifest.allowed_ref -cne 'refs/heads/main' -or
        [string]$manifest.object_format -cne 'sha1' -or
        [bool]$manifest.tracked_only -ne $true -or
        [bool]$manifest.allow_untracked -ne $false -or
        [bool]$manifest.allow_pre_staged -ne $false -or
        [bool]$manifest.local_commit_only -ne $true -or
        [bool]$manifest.push_allowed -ne $false -or
        [bool]$manifest.confirmation_required -ne $false -or
        $null -ne $manifest.info_attributes_sha256) {
        throw 'git_local_commit_v1 capability manifest has an unsupported authority contract.'
    }
    $repositoryId = [Guid]::Empty
    if (-not [Guid]::TryParseExact(
        [string]$manifest.repository_id, 'N', [ref]$repositoryId
    )) { throw 'git_local_commit_v1 repository_id is not canonical.' }
    foreach ($property in @('git_config_sha256', 'git_executable_sha256',
        'gitattributes_sha256', 'info_exclude_sha256')) {
        if ([string]$manifest.$property -notmatch '^[0-9a-f]{64}$') {
            throw "git_local_commit_v1 manifest hash is invalid: $property"
        }
    }
    foreach ($identityProperty in @('repository_root_identity', 'git_dir_identity',
        'git_executable_identity')) {
        if ([string]$manifest.$identityProperty -notmatch '^[0-9a-f]{16}:[0-9a-f]{32}$') {
            throw "git_local_commit_v1 strong identity is invalid: $identityProperty"
        }
    }
    $root = Get-NormalizedAbsolutePath `
        -Path ([string]$manifest.repository_root) -Label 'Registered repository root'
    $gitDir = Get-NormalizedAbsolutePath `
        -Path ([string]$manifest.git_dir) -Label 'Registered Git directory'
    $gitExecutable = Get-NormalizedAbsolutePath `
        -Path ([string]$manifest.git_executable_path) -Label 'Registered Git executable'
    $hooks = Get-NormalizedAbsolutePath `
        -Path ([string]$manifest.hooks_root) -Label 'Protected hooks root'
    $transactions = Get-NormalizedAbsolutePath `
        -Path ([string]$manifest.transactions_root) -Label 'Git transaction root'
    if ($gitDir -cne (Join-Path $root '.git') -or $hooks -cne $Paths.Hooks -or
        $transactions -cne $Paths.Transactions -or
        -not $gitExecutable.EndsWith('\mingw64\bin\git.exe',
            [StringComparison]::OrdinalIgnoreCase)) {
        throw 'git_local_commit_v1 manifest path closure is invalid.'
    }
    foreach ($entry in @(
        @($root, $true, [string]$manifest.repository_root_identity, 'Repository root'),
        @($gitDir, $true, [string]$manifest.git_dir_identity, 'Git directory'),
        @($gitExecutable, $false, [string]$manifest.git_executable_identity, 'Git executable')
    )) {
        [void](Assert-NoReparseAncestors -Path $entry[0] -Label $entry[3] -RequireLeaf)
        $item = Get-Item -LiteralPath $entry[0] -Force -ErrorAction Stop
        if ([bool]$item.PSIsContainer -ne [bool]$entry[1] -or
            ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0 -or
            (Get-StrongPathIdentity -Path $entry[0] -Directory ([bool]$entry[1])) -cne
                [string]$entry[2]) {
            throw "git_local_commit_v1 strong path identity drifted: $($entry[3])"
        }
    }
    if ((Get-FileSha256 -Path $gitExecutable) -cne
            [string]$manifest.git_executable_sha256) {
        throw 'git_local_commit_v1 Git executable hash drifted.'
    }
    $signature = Get-AuthenticodeSignature -LiteralPath $gitExecutable
    if ([string]$signature.Status -cne 'Valid' -or
        $null -eq $signature.SignerCertificate -or
        [string]$signature.SignerCertificate.Subject -cne
            [string]$manifest.git_signer_subject -or
        [string]$signature.SignerCertificate.Thumbprint -cne
            [string]$manifest.git_signer_thumbprint) {
        throw 'git_local_commit_v1 Git Authenticode signer drifted.'
    }
    $configPath = Join-Path $gitDir 'config'
    $attributesPath = Join-Path $root '.gitattributes'
    $infoAttributes = Join-Path (Join-Path $gitDir 'info') 'attributes'
    $infoExclude = Join-Path (Join-Path $gitDir 'info') 'exclude'
    foreach ($entry in @(
        @($configPath, [string]$manifest.git_config_sha256, 'Git config'),
        @($attributesPath, [string]$manifest.gitattributes_sha256, '.gitattributes'),
        @($infoExclude, [string]$manifest.info_exclude_sha256, 'info/exclude')
    )) {
        [void](Assert-NoReparseAncestors -Path $entry[0] -Label $entry[2] -RequireLeaf)
        if ((Get-FileSha256 -Path $entry[0]) -cne $entry[1]) {
            throw "git_local_commit_v1 protected repository input drifted: $($entry[2])"
        }
    }
    if ($null -ne (Get-Item -LiteralPath $infoAttributes -Force `
            -ErrorAction SilentlyContinue)) {
        throw 'git_local_commit_v1 requires info/attributes to remain absent.'
    }
    Assert-GitLocalConfigSafe -Path $configPath
    [void](Assert-NoReparseAncestors -Path $hooks -Label 'Protected hooks root' -RequireLeaf)
    if (@(Get-ChildItem -LiteralPath $hooks -Force -ErrorAction Stop).Count -ne 0) {
        throw 'git_local_commit_v1 protected hooks root is not empty.'
    }
    if ([string]$manifest.author_name -cne 'rayman' -or
        [string]$manifest.author_email -cne '32691594@qq.com') {
        throw 'git_local_commit_v1 protected author identity is invalid.'
    }
    return $manifest
}

function Read-GitCapabilityReady {
    param(
        [Parameter(Mandatory = $true)]$Paths,
        [Parameter(Mandatory = $true)]$Receipt
    )

    if (-not (Test-Path -LiteralPath $Paths.GitCapabilityReady -PathType Leaf)) {
        throw 'git_local_commit_v1 is not committed by the installer.'
    }
    [void](Assert-NoReparseAncestors -Path $Paths.GitCapabilityReady `
        -Label 'Git capability ready marker' -RequireLeaf)
    $file = New-ExpectedFileSecurity `
        -UserSid ([string]$Receipt.user_sid) `
        -SandboxSid ([string]$Receipt.sandbox_group_sid)
    Assert-ExactSecurity -Path $Paths.GitCapabilityReady -Expected $file `
        -ExpectedOwnerSid ([string]$Receipt.user_sid) `
        -Label 'Git capability ready marker'
    $ready = Read-StrictJsonDocument `
        -Path $Paths.GitCapabilityReady -MaximumBytes 16KB `
        -Label 'git_local_commit_v1 ready marker'
    Assert-ExactProperties -Document $ready `
        -Label 'git_local_commit_v1 ready marker' -Expected @(
        'git_capability_manifest_sha256', 'install_id', 'ready_at_utc',
        'schema_version', 'worker_sha256'
    )
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
        ) -or
        $readyAt -gt [DateTimeOffset]::UtcNow.AddSeconds(
            $script:MaxClockSkewSeconds
        )) {
        throw 'git_local_commit_v1 ready marker binding is invalid.'
    }
    return $ready
}

function New-FixedGitEnvironment {
    param(
        [Parameter(Mandatory = $true)]$Manifest,
        [Parameter(Mandatory = $true)]$Paths,
        [AllowNull()][string]$IndexPath
    )

    $environment = [Collections.Generic.Dictionary[string,string]]::new(
        [StringComparer]::OrdinalIgnoreCase
    )
    foreach ($entry in @{
        SystemRoot = [string]$env:SystemRoot
        WINDIR = [string]$env:WINDIR
        TEMP = $Paths.Transactions
        TMP = $Paths.Transactions
        HOME = $Paths.Hooks
        USERPROFILE = $Paths.Hooks
        PATH = ''
        GIT_DIR = [string]$Manifest.git_dir
        GIT_WORK_TREE = [string]$Manifest.repository_root
        GIT_CONFIG_NOSYSTEM = '1'
        GIT_CONFIG_SYSTEM = 'NUL'
        GIT_CONFIG_GLOBAL = 'NUL'
        GIT_CONFIG_COUNT = '0'
        GIT_ATTR_NOSYSTEM = '1'
        GIT_TERMINAL_PROMPT = '0'
        GCM_INTERACTIVE = 'Never'
        GIT_NO_LAZY_FETCH = '1'
        GIT_NO_REPLACE_OBJECTS = '1'
        GIT_OPTIONAL_LOCKS = '0'
        GIT_DISCOVERY_ACROSS_FILESYSTEM = '0'
        GIT_FLUSH = '1'
        LC_ALL = 'C'
        LANG = 'C'
    }.GetEnumerator()) { $environment.Add([string]$entry.Key, [string]$entry.Value) }
    if (-not [string]::IsNullOrEmpty($IndexPath)) {
        $environment.Add('GIT_INDEX_FILE', $IndexPath)
    }
    return $environment
}

function Invoke-FixedGit {
    param(
        [Parameter(Mandatory = $true)]$Manifest,
        [Parameter(Mandatory = $true)]$Paths,
        [Parameter(Mandatory = $true)]
        [ValidateSet('rev-parse-head', 'rev-parse-head-tree', 'ls-tree-head', 'symbolic-ref',
            'allowed-ref-symbolic',
            'write-tree', 'status', 'ls-files', 'add-update', 'hash-commit',
            'hash-filtered-blob',
            'update-ref', 'for-each-remote', 'for-each-all-refs', 'cat-commit',
            'diff-tree')]
        [string]$FixedCommand,
        [AllowNull()][string]$IndexPath,
        [AllowNull()][string]$ObjectId,
        [AllowNull()][string]$RelativePath,
        [AllowNull()][byte[]]$StandardInput,
        [int[]]$AllowedExitCodes = @(0)
    )

    $common = @(
        '-c', ('core.hooksPath=' + [string]$Manifest.hooks_root),
        '-c', 'core.fsmonitor=false',
        '-c', 'commit.gpgSign=false',
        '-c', 'tag.gpgSign=false',
        '-c', 'maintenance.auto=false',
        '-c', 'gc.auto=0',
        '-c', 'core.pager=cat',
        '-c', 'color.ui=false'
    )
    $specific = switch ($FixedCommand) {
        'rev-parse-head' { @('rev-parse', '--verify', 'HEAD') }
        'rev-parse-head-tree' { @('rev-parse', '--verify', 'HEAD^{tree}') }
        'ls-tree-head' { @('ls-tree', '-r', '-z', '--full-tree', 'HEAD') }
        'symbolic-ref' { @('symbolic-ref', '-q', 'HEAD') }
        'allowed-ref-symbolic' {
            @('symbolic-ref', '-q', [string]$Manifest.allowed_ref)
        }
        'write-tree' { @('write-tree') }
        'status' { @('status', '--porcelain=v1', '-z', '--untracked-files=all', '--ignore-submodules=none') }
        'ls-files' { @('ls-files', '--stage', '-z') }
        'add-update' { @('add', '-u', '--', ':/') }
        'hash-commit' { @('hash-object', '-t', 'commit', '-w', '--stdin') }
        'hash-filtered-blob' {
            $filteredPath = Assert-SafeGitRelativePath -Path $RelativePath
            if ($null -eq $StandardInput) {
                throw 'Filtered Git blob hashing requires exact held worktree bytes.'
            }
            @('-C', [string]$Manifest.repository_root, 'hash-object',
                ('--path=' + $filteredPath), '--stdin')
        }
        'update-ref' { @('update-ref', '--no-deref', '--stdin') }
        'for-each-remote' { @('for-each-ref', '--format=%(refname)%00%(objectname)%00', 'refs/remotes/') }
        'for-each-all-refs' { @('for-each-ref', '--format=%(refname) %(objectname)') }
        'cat-commit' {
            if ($ObjectId -notmatch '^[0-9a-f]{40}$') { throw 'Invalid internal commit oid.' }
            @('cat-file', 'commit', $ObjectId)
        }
        'diff-tree' {
            if ($ObjectId -notmatch '^[0-9a-f]{40}$') { throw 'Invalid internal commit oid.' }
            @('diff-tree', '--no-commit-id', '--name-only', '-r', '-z', $ObjectId)
        }
    }
    $environment = New-FixedGitEnvironment `
        -Manifest $Manifest -Paths $Paths -IndexPath $IndexPath
    $result = [Rayman.CodexBrokerNative]::RunSingleProcess(
        [string]$Manifest.git_executable_path,
        [string[]]@($common + $specific),
        [string]$Manifest.hooks_root,
        $environment,
        $StandardInput,
        30000,
        4MB
    )
    if ($AllowedExitCodes -notcontains [int]$result.ExitCode) {
        throw "Fixed Git command $FixedCommand failed exit=$($result.ExitCode): $($result.StandardError.Trim())"
    }
    return $result
}

function Get-FilteredGitBlobOid {
    param(
        [Parameter(Mandatory = $true)]$Manifest,
        [Parameter(Mandatory = $true)]$Paths,
        [Parameter(Mandatory = $true)][string]$RelativePath,
        [Parameter(Mandatory = $true)][byte[]]$Bytes
    )

    $result = Invoke-FixedGit -Manifest $Manifest -Paths $Paths `
        -FixedCommand hash-filtered-blob `
        -IndexPath (Join-Path ([string]$Manifest.git_dir) 'index') `
        -RelativePath $RelativePath `
        -StandardInput $Bytes
    $oid = $result.StandardOutput.Trim()
    if ($oid -notmatch '^[0-9a-f]{40}$') {
        throw "git_local_commit_v1 received an invalid filtered blob OID: $RelativePath"
    }
    return $oid
}

function Get-RemoteRefsSha256 {
    param(
        [Parameter(Mandatory = $true)]$Manifest,
        [Parameter(Mandatory = $true)]$Paths
    )

    $result = Invoke-FixedGit -Manifest $Manifest -Paths $Paths `
        -FixedCommand for-each-remote
    return Get-BytesSha256 -Bytes (
        [Text.UTF8Encoding]::new($false, $true).GetBytes($result.StandardOutput)
    )
}

function Get-OtherRefsSha256 {
    param(
        [Parameter(Mandatory = $true)]$Manifest,
        [Parameter(Mandatory = $true)]$Paths
    )

    $result = Invoke-FixedGit -Manifest $Manifest -Paths $Paths `
        -FixedCommand for-each-all-refs
    $lines = @(Get-OrdinalSortedStrings -Values @(
        $result.StandardOutput.Split(
            @("`r`n", "`n"), [StringSplitOptions]::RemoveEmptyEntries
        ) | Where-Object {
            $ref = ($_ -split ' ', 2)[0]
            $ref -cne [string]$Manifest.allowed_ref
        }
    ))
    return Get-BytesSha256 -Bytes (
        [Text.UTF8Encoding]::new($false, $true).GetBytes($lines -join "`n")
    )
}

function Assert-GitRepositoryShape {
    param([Parameter(Mandatory = $true)]$Manifest)

    $gitDir = [string]$Manifest.git_dir
    foreach ($relative in @(
        'index.lock', 'MERGE_HEAD', 'CHERRY_PICK_HEAD', 'REVERT_HEAD',
        'BISECT_LOG', 'sequencer', 'rebase-apply', 'rebase-merge', 'worktrees',
        'shallow', 'info\grafts', 'info\sparse-checkout',
        'objects\info\alternates', 'refs\replace', 'modules'
    )) {
        if (Test-Path -LiteralPath (Join-Path $gitDir $relative)) {
            throw "git_local_commit_v1 rejects Git operation/alternate/sparse state: $relative"
        }
    }
    foreach ($entry in @(Get-ChildItem -LiteralPath (Join-Path $gitDir 'hooks') `
        -File -Force -ErrorAction Stop)) {
        if (-not $entry.Name.EndsWith('.sample', [StringComparison]::Ordinal)) {
            throw "git_local_commit_v1 rejects an active repository hook: $($entry.Name)"
        }
    }
    $allowedAttributes = @(
        '* text=auto eol=lf', '*.png binary', '*.jpg binary', '*.jpeg binary',
        '*.gif binary', '*.ico binary', '*.pdf binary', '*.exe binary'
    )
    $actualAttributes = @(
        [IO.File]::ReadAllLines(
            (Join-Path ([string]$Manifest.repository_root) '.gitattributes'),
            [Text.UTF8Encoding]::new($false, $true)
        ) | Where-Object { -not [string]::IsNullOrWhiteSpace($_) }
    )
    if (($actualAttributes -join "`n") -cne ($allowedAttributes -join "`n")) {
        throw 'git_local_commit_v1 rejects filter-affecting or unknown .gitattributes rules.'
    }
    $indexPath = Join-Path $gitDir 'index'
    [void](Assert-NoReparseAncestors -Path $indexPath -Label 'Live Git index' -RequireLeaf)
    $index = Get-Item -LiteralPath $indexPath -Force -ErrorAction Stop
    if ($index.PSIsContainer -or
        ($index.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0 -or
        $index.Length -lt 12) {
        throw 'git_local_commit_v1 live index is not a bounded regular file.'
    }
    $indexBytes = [IO.File]::ReadAllBytes($indexPath)
    $indexAscii = [Text.Encoding]::ASCII.GetString($indexBytes)
    if ($indexAscii.Contains('link') -or $indexAscii.Contains('sdir')) {
        throw 'git_local_commit_v1 rejects split or sparse index extensions.'
    }
}

function Read-BoundedRegularFileHeld {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][int64]$MaximumBytes,
        [Parameter(Mandatory = $true)][string]$RepositoryRoot,
        [Parameter(Mandatory = $true)][string]$RepositoryIdentity,
        [switch]$Hold
    )
    $lease = [Rayman.CodexBrokerNative]::ReadBoundedRegularFileTreeHeld(
        $RepositoryRoot, $RepositoryIdentity, $Path, $MaximumBytes
    )
    if ($Hold) {
        return [pscustomobject]@{ Bytes = $lease.Bytes; Lease = $lease }
    }
    try {
        return [pscustomobject]@{ Bytes = $lease.Bytes; Lease = $null }
    } finally { $lease.Dispose() }
}

function Get-GitTrackedChangeSnapshot {
    param(
        [Parameter(Mandatory = $true)]$Manifest,
        [Parameter(Mandatory = $true)]$Paths,
        [switch]$HoldChangedFiles,
        [switch]$AllowNoChanges
    )

    Assert-GitRepositoryShape -Manifest $Manifest
    $branchResult = Invoke-FixedGit -Manifest $Manifest -Paths $Paths `
        -FixedCommand symbolic-ref
    $branch = $branchResult.StandardOutput.Trim()
    if ($branch -cne [string]$Manifest.allowed_ref) {
        throw 'git_local_commit_v1 requires the registered attached branch.'
    }
    $allowedRefSymbolic = Invoke-FixedGit -Manifest $Manifest -Paths $Paths `
        -FixedCommand allowed-ref-symbolic -AllowedExitCodes @(0, 1)
    if ([int]$allowedRefSymbolic.ExitCode -ne 1 -or
        -not [string]::IsNullOrEmpty($allowedRefSymbolic.StandardOutput) -or
        -not [string]::IsNullOrEmpty($allowedRefSymbolic.StandardError)) {
        throw 'git_local_commit_v1 requires the registered branch ref itself to be direct.'
    }
    $head = (Invoke-FixedGit -Manifest $Manifest -Paths $Paths `
        -FixedCommand rev-parse-head).StandardOutput.Trim()
    $headTree = (Invoke-FixedGit -Manifest $Manifest -Paths $Paths `
        -FixedCommand rev-parse-head-tree).StandardOutput.Trim()
    foreach ($oid in @($head, $headTree)) {
        if ($oid -notmatch '^[0-9a-f]{40}$') {
            throw 'git_local_commit_v1 received a non-SHA1 object identity.'
        }
    }
    $lsFilesResult = Invoke-FixedGit -Manifest $Manifest -Paths $Paths `
        -FixedCommand ls-files
    $tracked = @{}
    foreach ($record in @($lsFilesResult.StandardOutput.Split(
        [char]0, [StringSplitOptions]::RemoveEmptyEntries
    ))) {
        if ($record -notmatch '^([0-7]{6}) ([0-9a-f]{40}) ([0-3])\t(.+)$') {
            throw 'git_local_commit_v1 cannot parse the exact tracked index.'
        }
        $mode = [string]$Matches[1]
        $oid = [string]$Matches[2]
        $stage = [string]$Matches[3]
        $path = Assert-SafeGitRelativePath -Path ([string]$Matches[4])
        if ($stage -cne '0' -or ($mode -cne '100644' -and $mode -cne '100755') -or
            $tracked.ContainsKey($path) -or
            ($path.EndsWith('/.gitattributes', [StringComparison]::OrdinalIgnoreCase)) -or
            $path -ieq '.gitmodules') {
            throw "git_local_commit_v1 rejects conflicts, gitlinks, symlinks, nested attributes, or duplicate index paths: $path"
        }
        $tracked[$path] = [pscustomobject]@{ Mode = $mode; Oid = $oid }
    }
    $headEntriesResult = Invoke-FixedGit -Manifest $Manifest -Paths $Paths `
        -FixedCommand ls-tree-head
    $headEntries = @{}
    foreach ($record in @($headEntriesResult.StandardOutput.Split(
        [char]0, [StringSplitOptions]::RemoveEmptyEntries
    ))) {
        if ($record -notmatch '^([0-7]{6}) blob ([0-9a-f]{40})\t(.+)$') {
            throw 'git_local_commit_v1 rejects non-blob or malformed HEAD tree entries.'
        }
        $mode = [string]$Matches[1]
        $oid = [string]$Matches[2]
        $path = Assert-SafeGitRelativePath -Path ([string]$Matches[3])
        if (($mode -cne '100644' -and $mode -cne '100755') -or
            $headEntries.ContainsKey($path) -or $path -ieq '.gitmodules') {
            throw "git_local_commit_v1 rejects unsupported or duplicate HEAD tree entries: $path"
        }
        $headEntries[$path] = [pscustomobject]@{ Mode = $mode; Oid = $oid }
    }
    if ($headEntries.Count -ne $tracked.Count) {
        throw 'git_local_commit_v1 rejects pre-staged index content: index and HEAD path counts differ.'
    }
    foreach ($path in $tracked.Keys) {
        if (-not $headEntries.ContainsKey($path) -or
            [string]$headEntries[$path].Mode -cne [string]$tracked[$path].Mode -or
            [string]$headEntries[$path].Oid -cne [string]$tracked[$path].Oid) {
            throw "git_local_commit_v1 rejects pre-staged index content: index and HEAD entry differ: $path"
        }
    }
    # The complete stage-0 index map is byte-for-byte equivalent to HEAD's
    # recursive tree map, so its tree identity is the already verified HEAD
    # tree. The sandbox-side client never runs write-tree against the live
    # index; only the logged-on-user worker writes its alternate index.
    $indexTree = $headTree
    $statusResult = Invoke-FixedGit -Manifest $Manifest -Paths $Paths `
        -FixedCommand status
    $statusRecords = @($statusResult.StandardOutput.Split(
        [char]0, [StringSplitOptions]::RemoveEmptyEntries
    ))
    if (-not $AllowNoChanges -and $statusRecords.Count -eq 0) {
        throw 'git_local_commit_v1 requires at least one tracked modification or deletion.'
    }
    if ($statusRecords.Count -gt 128) {
        throw 'git_local_commit_v1 change count exceeds 128 tracked paths.'
    }
    $changes = [Collections.Generic.List[object]]::new()
    $handles = [Collections.Generic.List[IDisposable]]::new()
    $caseFolded = [Collections.Generic.HashSet[string]]::new(
        [StringComparer]::OrdinalIgnoreCase
    )
    $totalBytes = [int64]0
    try {
        foreach ($record in $statusRecords) {
            if ($record.Length -lt 4 -or $record[0] -cne ' ' -or
                ($record[1] -cne 'M' -and $record[1] -cne 'D') -or
                $record[2] -cne ' ') {
                throw "git_local_commit_v1 rejects untracked, staged, renamed, conflicted, type, or mode status: $record"
            }
            $path = Assert-SafeGitRelativePath -Path $record.Substring(3)
            if (-not $tracked.ContainsKey($path) -or -not $caseFolded.Add($path)) {
                throw "git_local_commit_v1 status path is missing or case-colliding in the index: $path"
            }
            $entry = $tracked[$path]
            if ($record[1] -ceq 'D') {
                $absolute = Join-Path ([string]$Manifest.repository_root) `
                    ($path.Replace('/', [IO.Path]::DirectorySeparatorChar))
                if (Test-Path -LiteralPath $absolute) {
                    throw "git_local_commit_v1 deletion path reappeared: $path"
                }
                $changes.Add([ordered]@{
                    path = $path; kind = 'deleted'
                    index_blob_oid_before = [string]$entry.Oid
                    index_mode_before = [string]$entry.Mode
                    worktree_sha256_after = $null; git_blob_oid_after = $null
                })
                continue
            }
            $absolute = Join-Path ([string]$Manifest.repository_root) `
                ($path.Replace('/', [IO.Path]::DirectorySeparatorChar))
            [void](Assert-ChildPath -Child $absolute `
                -Parent ([string]$Manifest.repository_root) -Label 'Tracked change')
            $held = Read-BoundedRegularFileHeld -Path $absolute -MaximumBytes 32MB `
                -RepositoryRoot ([string]$Manifest.repository_root) `
                -RepositoryIdentity ([string]$Manifest.repository_root_identity) `
                -Hold:$HoldChangedFiles
            $totalBytes += $held.Bytes.Length
            if ($totalBytes -gt 128MB) {
                if ($null -ne $held.Lease) { $held.Lease.Dispose() }
                throw 'git_local_commit_v1 total changed bytes exceed 128 MiB.'
            }
            if ($null -ne $held.Lease) { $handles.Add($held.Lease) }
            $changes.Add([ordered]@{
                path = $path; kind = 'modified'
                index_blob_oid_before = [string]$entry.Oid
                index_mode_before = [string]$entry.Mode
                worktree_sha256_after = Get-BytesSha256 -Bytes $held.Bytes
                git_blob_oid_after = Get-FilteredGitBlobOid `
                    -Manifest $Manifest -Paths $Paths -RelativePath $path `
                    -Bytes $held.Bytes
            })
        }
        $sorted = @($changes | ForEach-Object { $_ })
        $previousStatusPath = $null
        foreach ($change in $sorted) {
            $statusPath = [string]$change.path
            if ($null -ne $previousStatusPath -and
                [StringComparer]::Ordinal.Compare(
                    [string]$previousStatusPath, $statusPath
                ) -ge 0) {
                throw 'git_local_commit_v1 requires Git status paths in canonical ordinal order.'
            }
            $previousStatusPath = $statusPath
        }
        $indexPath = Join-Path ([string]$Manifest.git_dir) 'index'
        return [pscustomobject]@{
            Head = $head
            HeadTree = $headTree
            IndexTree = $indexTree
            IndexSha256 = Get-FileSha256 -Path $indexPath
            RemoteRefsSha256 = Get-RemoteRefsSha256 -Manifest $Manifest -Paths $Paths
            OtherRefsSha256 = Get-OtherRefsSha256 -Manifest $Manifest -Paths $Paths
            Changes = $sorted
            IndexEntries = @(
                Get-OrdinalSortedStrings -Values @($tracked.Keys) | ForEach-Object {
                    [ordered]@{
                        path = [string]$_
                        mode = [string]$tracked[$_].Mode
                        oid = [string]$tracked[$_].Oid
                    }
                }
            )
            ChangedFiles = $handles
            StatusSha256 = Get-BytesSha256 -Bytes (
                [Text.UTF8Encoding]::new($false, $true).GetBytes(
                    $statusResult.StandardOutput
                )
            )
        }
    } catch {
        foreach ($handle in $handles) { $handle.Dispose() }
        throw
    }
}

function Close-GitSnapshotHandles {
    param([AllowNull()]$Snapshot)
    if ($null -eq $Snapshot) { return }
    foreach ($handle in @($Snapshot.ChangedFiles)) {
        if ($null -ne $handle) { $handle.Dispose() }
    }
}

function Assert-GitChangeDocument {
    param(
        [Parameter(Mandatory = $true)]$Change,
        [Parameter(Mandatory = $true)][int]$Index,
        [Parameter(Mandatory = $true)][AllowEmptyCollection()]
        [Collections.Generic.HashSet[string]]$Seen
    )

    Assert-ExactProperties -Document $Change `
        -Label "git_local_commit_v1 changes[$Index]" -Expected @(
        'git_blob_oid_after', 'index_blob_oid_before', 'index_mode_before',
        'kind', 'path', 'worktree_sha256_after'
    )
    $path = Assert-SafeGitRelativePath -Path ([string]$Change.path)
    if (-not $Seen.Add($path)) {
        throw "git_local_commit_v1 duplicate or case-colliding request path: $path"
    }
    if ([string]$Change.index_blob_oid_before -notmatch '^[0-9a-f]{40}$' -or
        ([string]$Change.index_mode_before -cne '100644' -and
         [string]$Change.index_mode_before -cne '100755')) {
        throw "git_local_commit_v1 request has an invalid prior index entry: $path"
    }
    if ([string]$Change.kind -ceq 'modified') {
        if ([string]$Change.worktree_sha256_after -notmatch '^[0-9a-f]{64}$' -or
            [string]$Change.git_blob_oid_after -notmatch '^[0-9a-f]{40}$') {
            throw "git_local_commit_v1 modified request hashes are invalid: $path"
        }
    } elseif ([string]$Change.kind -ceq 'deleted') {
        if ($null -ne $Change.worktree_sha256_after -or
            $null -ne $Change.git_blob_oid_after) {
            throw "git_local_commit_v1 deletion request must carry null after-hashes: $path"
        }
    } else {
        throw "git_local_commit_v1 request kind is unsupported: $path"
    }
    return $path
}

function Assert-GitCommitPayload {
    param(
        [Parameter(Mandatory = $true)]$Payload,
        [Parameter(Mandatory = $true)]$Receipt
    )

    Assert-ExactProperties -Document $Payload `
        -Label 'git_local_commit_v1 payload' -Expected @(
        'capability_manifest_sha256', 'changes', 'commit_message_utf8',
        'expected_head_oid'
    )
    if ([string]$Payload.capability_manifest_sha256 -cne
            [string]$Receipt.git_capability_manifest_sha256 -or
        [string]$Payload.expected_head_oid -notmatch '^[0-9a-f]{40}$') {
        throw 'git_local_commit_v1 request is not bound to the installed manifest and expected HEAD.'
    }
    $normalizedMessage = Assert-GitCommitMessage `
        -Message ([string]$Payload.commit_message_utf8)
    if ($normalizedMessage -cne [string]$Payload.commit_message_utf8) {
        throw 'git_local_commit_v1 commit message is not NFC-normalized.'
    }
    if ($Payload.changes -isnot [array] -or
        @($Payload.changes).Count -lt 1 -or @($Payload.changes).Count -gt 128) {
        throw 'git_local_commit_v1 changes must be an array of 1-128 exact tracked changes.'
    }
    $seen = [Collections.Generic.HashSet[string]]::new(
        [StringComparer]::OrdinalIgnoreCase
    )
    $previous = $null
    for ($index = 0; $index -lt @($Payload.changes).Count; $index++) {
        $path = Assert-GitChangeDocument `
            -Change $Payload.changes[$index] -Index $index -Seen $seen
        if ($null -ne $previous -and
            [StringComparer]::Ordinal.Compare($previous, $path) -ge 0) {
            throw 'git_local_commit_v1 request paths must be strictly ordinal-sorted.'
        }
        $previous = $path
    }
}

function Assert-GitSnapshotMatchesPayload {
    param(
        [Parameter(Mandatory = $true)]$Snapshot,
        [Parameter(Mandatory = $true)]$Payload
    )

    if ([string]$Snapshot.Head -cne [string]$Payload.expected_head_oid -or
        @($Snapshot.Changes).Count -ne @($Payload.changes).Count) {
        throw 'git_local_commit_v1 expected HEAD or change count drifted before execution.'
    }
    for ($index = 0; $index -lt @($Snapshot.Changes).Count; $index++) {
        $actual = $Snapshot.Changes[$index]
        $expected = $Payload.changes[$index]
        foreach ($property in @(
            'path', 'kind', 'index_blob_oid_before', 'index_mode_before',
            'worktree_sha256_after', 'git_blob_oid_after'
        )) {
            $actualValue = $actual.$property
            $expectedValue = $expected.$property
            if ($null -eq $actualValue -xor $null -eq $expectedValue) {
                throw "git_local_commit_v1 change snapshot drifted: $($actual.path).$property"
            }
            if ($null -ne $actualValue -and
                [string]$actualValue -cne [string]$expectedValue) {
                throw "git_local_commit_v1 change snapshot drifted: $($actual.path).$property"
            }
        }
    }
}

function Assert-AlternateIndexMatchesPayload {
    param(
        [Parameter(Mandatory = $true)]$Manifest,
        [Parameter(Mandatory = $true)]$Paths,
        [Parameter(Mandatory = $true)][string]$IndexPath,
        [Parameter(Mandatory = $true)]$Payload,
        [Parameter(Mandatory = $true)]$BaselineSnapshot
    )

    $result = Invoke-FixedGit -Manifest $Manifest -Paths $Paths `
        -FixedCommand ls-files -IndexPath $IndexPath
    $entries = @{}
    foreach ($record in @($result.StandardOutput.Split(
        [char]0, [StringSplitOptions]::RemoveEmptyEntries
    ))) {
        if ($record -notmatch '^([0-7]{6}) ([0-9a-f]{40}) ([0-3])\t(.+)$') {
            throw 'git_local_commit_v1 cannot parse the alternate index.'
        }
        $path = [string]$Matches[4]
        if ([string]$Matches[3] -cne '0' -or $entries.ContainsKey($path)) {
            throw 'git_local_commit_v1 alternate index contains conflicts or duplicates.'
        }
        $entries[$path] = [pscustomobject]@{
            Mode = [string]$Matches[1]
            Oid = [string]$Matches[2]
        }
    }
    foreach ($change in @($Payload.changes)) {
        $path = [string]$change.path
        if ([string]$change.kind -ceq 'deleted') {
            if ($entries.ContainsKey($path)) {
                throw "git_local_commit_v1 alternate index retained a requested deletion: $path"
            }
        } elseif (-not $entries.ContainsKey($path) -or
            [string]$entries[$path].Mode -cne [string]$change.index_mode_before -or
            [string]$entries[$path].Oid -cne [string]$change.git_blob_oid_after) {
            throw "git_local_commit_v1 alternate index blob/mode differs from the request: $path"
        }
    }
    $expectedEntries = @{}
    foreach ($entry in @($BaselineSnapshot.IndexEntries)) {
        $expectedEntries[[string]$entry.path] = [pscustomobject]@{
            Mode = [string]$entry.mode
            Oid = [string]$entry.oid
        }
    }
    foreach ($change in @($Payload.changes)) {
        $path = [string]$change.path
        if ([string]$change.kind -ceq 'deleted') {
            [void]$expectedEntries.Remove($path)
        } else {
            $expectedEntries[$path] = [pscustomobject]@{
                Mode = [string]$change.index_mode_before
                Oid = [string]$change.git_blob_oid_after
            }
        }
    }
    if ($entries.Count -ne $expectedEntries.Count) {
        throw 'git_local_commit_v1 alternate index contains an extra or missing tracked path.'
    }
    foreach ($path in @($expectedEntries.Keys)) {
        if (-not $entries.ContainsKey($path) -or
            [string]$entries[$path].Mode -cne [string]$expectedEntries[$path].Mode -or
            [string]$entries[$path].Oid -cne [string]$expectedEntries[$path].Oid) {
            throw "git_local_commit_v1 alternate index changed an unrequested entry: $path"
        }
    }
}

function New-GitCommitPayload {
    param(
        [Parameter(Mandatory = $true)]$Paths,
        [Parameter(Mandatory = $true)]$Receipt,
        [Parameter(Mandatory = $true)][string]$Message
    )

    $manifest = Read-GitCapabilityManifest -Paths $Paths -Receipt $Receipt
    [void](Read-GitCapabilityReady -Paths $Paths -Receipt $Receipt)
    $snapshot = Get-GitTrackedChangeSnapshot -Manifest $manifest -Paths $Paths
    try {
        return [ordered]@{
            capability_manifest_sha256 =
                [string]$Receipt.git_capability_manifest_sha256
            expected_head_oid = [string]$snapshot.Head
            changes = @($snapshot.Changes)
            commit_message_utf8 = Assert-GitCommitMessage -Message $Message
        }
    } finally { Close-GitSnapshotHandles -Snapshot $snapshot }
}

function Write-BytesWriteThrough {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][byte[]]$Bytes,
        [switch]$CreateNew
    )

    $mode = if ($CreateNew) { [IO.FileMode]::CreateNew } else { [IO.FileMode]::Create }
    $stream = [IO.FileStream]::new(
        $Path, $mode, [IO.FileAccess]::Write, [IO.FileShare]::None,
        4096, [IO.FileOptions]::WriteThrough
    )
    try {
        $stream.Write($Bytes, 0, $Bytes.Length)
        $stream.Flush($true)
    } finally { $stream.Dispose() }
}

function Get-IndexSecurityFingerprint {
    param([Parameter(Mandatory = $true)][string]$Path)

    $acl = Get-Acl -LiteralPath $Path -ErrorAction Stop
    $owner = [string]([Security.Principal.NTAccount]::new(
        [string]$acl.Owner
    ).Translate([Security.Principal.SecurityIdentifier]).Value)
    return [pscustomobject]@{
        Owner = $owner
        Access = $acl.GetSecurityDescriptorSddlForm(
            [Security.AccessControl.AccessControlSections]::Access
        )
    }
}

function Get-GitChangesDigest {
    param([Parameter(Mandatory = $true)]$Changes)
    $bytes = [Text.UTF8Encoding]::new($false, $true).GetBytes(
        (@($Changes) | ConvertTo-Json -Depth 8 -Compress)
    )
    return Get-BytesSha256 -Bytes $bytes
}

function New-GitCommitObjectBytes {
    param(
        [Parameter(Mandatory = $true)]$Manifest,
        [Parameter(Mandatory = $true)][string]$TreeOid,
        [Parameter(Mandatory = $true)][string]$ParentOid,
        [Parameter(Mandatory = $true)][string]$Message
    )

    foreach ($oid in @($TreeOid, $ParentOid)) {
        if ($oid -notmatch '^[0-9a-f]{40}$') { throw 'Invalid internal Git object id.' }
    }
    $now = [DateTimeOffset]::Now
    $zone = $now.ToString('zzz', [Globalization.CultureInfo]::InvariantCulture).Replace(':', '')
    $identity = "$( [string]$Manifest.author_name) <$( [string]$Manifest.author_email)> $($now.ToUnixTimeSeconds()) $zone"
    $text = "tree $TreeOid`nparent $ParentOid`nauthor $identity`ncommitter $identity`n`n$Message`n"
    return [Text.UTF8Encoding]::new($false, $true).GetBytes($text)
}

function Write-GitJournal {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)]$Journal,
        [switch]$Create
    )

    $Journal.updated_at_utc = [DateTimeOffset]::UtcNow.ToString('o')
    Write-JsonAtomic -Path $Path -Document $Journal -Replace:(-not $Create)
}

function Invoke-GitSelfTestFault {
    param([Parameter(Mandatory = $true)][string]$Phase)
    if ($SelfTest -and [string]$script:GitSelfTestFaultPhase -ceq $Phase) {
        throw "git_local_commit_v1 injected self-test fault after $Phase"
    }
}

function Read-GitJournal {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)]$Paths,
        [Parameter(Mandatory = $true)]$Manifest
    )

    $journal = Read-StrictJsonDocument -Path $Path -MaximumBytes 512KB `
        -Label 'git_local_commit_v1 transaction journal'
    Assert-ExactProperties -Document $journal `
        -Label 'git_local_commit_v1 transaction journal' -Expected @(
        'alternate_index_path', 'alternate_index_sha256', 'backup_index_path',
        'commit_bytes_base64', 'commit_message_sha256', 'commit_oid',
        'created_at_utc', 'head_before', 'head_tree_before', 'index_before_sha256',
        'index_path', 'install_id', 'output', 'phase', 'remote_refs_before_sha256',
        'other_refs_before_sha256', 'request_id', 'request_sha256', 'schema_version', 'stage_index_path',
        'tree_oid', 'updated_at_utc'
    )
    if ([int]$journal.schema_version -ne 1 -or
        [string]$journal.request_id -notmatch '^[0-9a-f]{32}$' -or
        [string]$journal.request_sha256 -notmatch '^[0-9a-f]{64}$' -or
        [string]$journal.head_before -notmatch '^[0-9a-f]{40}$' -or
        [string]$journal.index_before_sha256 -notmatch '^[0-9a-f]{64}$') {
        throw 'git_local_commit_v1 transaction journal identity is invalid.'
    }
    $requestId = [string]$journal.request_id
    $expectedIndex = Join-Path ([string]$Manifest.git_dir) 'index'
    $expectedAlternate = Join-Path $Paths.Transactions ($requestId + '.index')
    $expectedStage = Join-Path ([string]$Manifest.git_dir) 'index.lock'
    $expectedBackup = Join-Path ([string]$Manifest.git_dir) `
        ('.rayman-git-local-commit-' + $requestId + '.backup')
    if ([string]$journal.index_path -cne $expectedIndex -or
        [string]$journal.alternate_index_path -cne $expectedAlternate -or
        [string]$journal.stage_index_path -cne $expectedStage -or
        [string]$journal.backup_index_path -cne $expectedBackup -or
        [string]$Path -cne (Join-Path $Paths.Transactions ($requestId + '.journal.json')) -or
        [string]$journal.phase -notin @(
            'accepted', 'index_prepared', 'objects_written', 'index_staged', 'ref_updated',
            'index_published', 'verified'
        ) -or
        ($null -ne $journal.alternate_index_sha256 -and
            [string]$journal.alternate_index_sha256 -notmatch '^[0-9a-f]{64}$') -or
        ($null -ne $journal.tree_oid -and
            [string]$journal.tree_oid -notmatch '^[0-9a-f]{40}$') -or
        ($null -ne $journal.commit_oid -and
            [string]$journal.commit_oid -notmatch '^[0-9a-f]{40}$') -or
        [string]$journal.remote_refs_before_sha256 -notmatch '^[0-9a-f]{64}$' -or
        [string]$journal.other_refs_before_sha256 -notmatch '^[0-9a-f]{64}$' -or
        [string]$journal.commit_message_sha256 -notmatch '^[0-9a-f]{64}$') {
        throw 'git_local_commit_v1 transaction journal paths or phase drifted.'
    }
    return $journal
}

function Stage-GitAlternateIndex {
    param(
        [Parameter(Mandatory = $true)]$Manifest,
        [Parameter(Mandatory = $true)]$Journal
    )

    $live = [string]$Journal.index_path
    $alternate = [string]$Journal.alternate_index_path
    $stage = [string]$Journal.stage_index_path
    $backup = [string]$Journal.backup_index_path
    $liveHash = Get-FileSha256 -Path $live
    $alternateHash = Get-FileSha256 -Path $alternate
    if ($liveHash -cne [string]$Journal.index_before_sha256 -or
        $alternateHash -cne [string]$Journal.alternate_index_sha256) {
        throw "git_local_commit_v1 index publication precondition drifted. live=$liveHash expected_live=$($Journal.index_before_sha256) alternate=$alternateHash expected_alternate=$($Journal.alternate_index_sha256)"
    }
    if (Test-Path -LiteralPath $backup) {
        throw "git_local_commit_v1 refuses a pre-existing index backup before staging: $backup"
    }
    if (Test-Path -LiteralPath $stage) {
        throw "git_local_commit_v1 refuses a pre-existing standard index lock: $stage"
    }
    $securityBefore = Get-IndexSecurityFingerprint -Path $live
    Write-BytesWriteThrough -Path $stage `
        -Bytes ([IO.File]::ReadAllBytes($alternate)) -CreateNew
    $stageItem = Get-Item -LiteralPath $stage -Force
    if ($stageItem.PSIsContainer -or
        ($stageItem.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0 -or
        (Get-FileSha256 -Path $stage) -cne $alternateHash) {
        throw 'git_local_commit_v1 standard index lock is not the journaled alternate index.'
    }
    $stageSecurity = Get-IndexSecurityFingerprint -Path $stage
    if ($stageSecurity.Owner -cne $securityBefore.Owner -or
        $stageSecurity.Access -cne $securityBefore.Access) {
        throw 'git_local_commit_v1 standard index lock did not inherit the live index security.'
    }
}

function Publish-GitAlternateIndex {
    param(
        [Parameter(Mandatory = $true)]$Manifest,
        [Parameter(Mandatory = $true)]$Journal
    )

    $live = [string]$Journal.index_path
    $alternate = [string]$Journal.alternate_index_path
    $stage = [string]$Journal.stage_index_path
    $backup = [string]$Journal.backup_index_path
    $liveHash = Get-FileSha256 -Path $live
    $alternateHash = Get-FileSha256 -Path $alternate
    if ($liveHash -cne [string]$Journal.index_before_sha256 -or
        $alternateHash -cne [string]$Journal.alternate_index_sha256) {
        throw "git_local_commit_v1 index publication precondition drifted. live=$liveHash expected_live=$($Journal.index_before_sha256) alternate=$alternateHash expected_alternate=$($Journal.alternate_index_sha256)"
    }
    if (Test-Path -LiteralPath $backup) {
        throw "git_local_commit_v1 refuses a pre-existing index backup before publication: $backup"
    }
    $stageItem = Get-Item -LiteralPath $stage -Force -ErrorAction Stop
    if ($stageItem.PSIsContainer -or
        ($stageItem.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0 -or
        (Get-FileSha256 -Path $stage) -cne $alternateHash) {
        throw 'git_local_commit_v1 standard index lock drifted before publication.'
    }
    $securityBefore = Get-IndexSecurityFingerprint -Path $live
    $stageSecurity = Get-IndexSecurityFingerprint -Path $stage
    if ($stageSecurity.Owner -cne $securityBefore.Owner -or
        $stageSecurity.Access -cne $securityBefore.Access) {
        throw 'git_local_commit_v1 standard index lock security drifted before publication.'
    }
    [IO.File]::Replace($stage, $live, $backup, $true)
    if ((Get-FileSha256 -Path $backup) -cne
        [string]$Journal.index_before_sha256) {
        throw 'git_local_commit_v1 index backup does not prove the publication precondition.'
    }
    if ((Get-FileSha256 -Path $live) -cne
        [string]$Journal.alternate_index_sha256) {
        throw 'git_local_commit_v1 live index hash does not match the alternate index.'
    }
    $securityAfter = Get-IndexSecurityFingerprint -Path $live
    if ($securityAfter.Owner -cne $securityBefore.Owner -or
        $securityAfter.Access -cne $securityBefore.Access) {
        throw 'git_local_commit_v1 live index security drifted during publication.'
    }
}

function Test-GitTransactionOutput {
    param(
        [Parameter(Mandatory = $true)]$Manifest,
        [Parameter(Mandatory = $true)]$Paths,
        [Parameter(Mandatory = $true)]$Journal,
        [Parameter(Mandatory = $true)]$Payload
    )

    $head = (Invoke-FixedGit -Manifest $Manifest -Paths $Paths `
        -FixedCommand rev-parse-head).StandardOutput.Trim()
    if ($head -cne [string]$Journal.commit_oid) {
        throw 'git_local_commit_v1 postcondition HEAD is not the transaction commit.'
    }
    $backup = [string]$Journal.backup_index_path
    if ([string]$Journal.phase -cne 'verified' -and
        (-not (Test-Path -LiteralPath $backup -PathType Leaf) -or
         (Get-FileSha256 -Path $backup) -cne
            [string]$Journal.index_before_sha256)) {
        throw 'git_local_commit_v1 index backup cannot prove the replaced live index.'
    }
    if ((Test-Path -LiteralPath $backup -PathType Leaf) -and
        (Get-FileSha256 -Path $backup) -cne
            [string]$Journal.index_before_sha256) {
        throw 'git_local_commit_v1 retained index backup drifted.'
    }
    $tree = (Invoke-FixedGit -Manifest $Manifest -Paths $Paths `
        -FixedCommand write-tree).StandardOutput.Trim()
    if ($tree -cne [string]$Journal.tree_oid) {
        throw 'git_local_commit_v1 live index tree does not equal the committed tree.'
    }
    $commitBytes = [Convert]::FromBase64String([string]$Journal.commit_bytes_base64)
    $commitText = (Invoke-FixedGit -Manifest $Manifest -Paths $Paths `
        -FixedCommand cat-commit -ObjectId $head).StandardOutput
    if ((Get-BytesSha256 -Bytes (
            [Text.UTF8Encoding]::new($false, $true).GetBytes($commitText)
        )) -cne (Get-BytesSha256 -Bytes $commitBytes)) {
        throw 'git_local_commit_v1 committed object bytes do not match the protected journal.'
    }
    $changedPathOutput = (Invoke-FixedGit -Manifest $Manifest -Paths $Paths `
        -FixedCommand diff-tree -ObjectId $head).StandardOutput
    $actualPaths = @(Get-OrdinalSortedStrings -Values @($changedPathOutput.Split(
        [char]0, [StringSplitOptions]::RemoveEmptyEntries
    )))
    $expectedPaths = @(Get-OrdinalSortedStrings -Values @(
        $Payload.changes | ForEach-Object { [string]$_.path }
    ))
    if (($actualPaths -join "`n") -cne ($expectedPaths -join "`n")) {
        throw 'git_local_commit_v1 commit path set differs from the request snapshot.'
    }
    $post = Get-GitTrackedChangeSnapshot -Manifest $Manifest -Paths $Paths `
        -AllowNoChanges
    try {
        if (@($post.Changes).Count -ne 0 -or
            $post.RemoteRefsSha256 -cne
                [string]$Journal.remote_refs_before_sha256 -or
            $post.OtherRefsSha256 -cne
                [string]$Journal.other_refs_before_sha256) {
            throw 'git_local_commit_v1 postcondition is dirty or remote refs changed.'
        }
        return [ordered]@{
            transaction_id = [string]$Journal.request_id
            repository_id = [string]$Manifest.repository_id
            ref = [string]$Manifest.allowed_ref
            parent = [string]$Journal.head_before
            head_before = [string]$Journal.head_before
            commit_oid = [string]$Journal.commit_oid
            tree_oid = [string]$Journal.tree_oid
            head_after = $head
            changed_paths = $expectedPaths
            changed_paths_digest = Get-GitChangesDigest -Changes $Payload.changes
            changed_path_count = $expectedPaths.Count
            commit_message_sha256 = [string]$Journal.commit_message_sha256
            index_before_sha256 = [string]$Journal.index_before_sha256
            index_after_sha256 = [string]$post.IndexSha256
            mutation_phase = 'verified'
            recovery_state = 'none'
            git_sha256 = [string]$Manifest.git_executable_sha256
            remote_refs_sha256 = [string]$post.RemoteRefsSha256
            other_refs_sha256 = [string]$post.OtherRefsSha256
            hooks_disabled = $true
            child_process_policy = 'single_process'
            network_operation = $false
            post_clean = $true
        }
    } finally { Close-GitSnapshotHandles -Snapshot $post }
}

function Remove-GitTransactionScratch {
    param([Parameter(Mandatory = $true)]$Journal)
    foreach ($path in @(
        [string]$Journal.alternate_index_path,
        [string]$Journal.backup_index_path
    )) {
        if (-not [string]::IsNullOrEmpty($path) -and
            (Test-Path -LiteralPath $path -PathType Leaf)) {
            Remove-Item -LiteralPath $path -Force -ErrorAction SilentlyContinue
        }
    }
    $stage = [string]$Journal.stage_index_path
    if ([string]$Journal.phase -in @(
            'accepted', 'index_prepared', 'objects_written', 'index_staged'
        ) -and (Test-Path -LiteralPath $stage -PathType Leaf)) {
        if ([string]$Journal.alternate_index_sha256 -notmatch '^[0-9a-f]{64}$' -or
            (Get-FileSha256 -Path $stage) -cne
                [string]$Journal.alternate_index_sha256) {
            throw 'git_local_commit_v1 refuses to remove a non-journaled standard index lock.'
        }
        Remove-Item -LiteralPath $stage -Force -ErrorAction Stop
    }
}

function Invoke-GitLocalCommitOperation {
    param(
        [Parameter(Mandatory = $true)]$Request,
        [Parameter(Mandatory = $true)]$Receipt,
        [Parameter(Mandatory = $true)]$Paths,
        [Parameter(Mandatory = $true)][string]$RequestSha256
    )

    $manifest = Read-GitCapabilityManifest -Paths $Paths -Receipt $Receipt
    [void](Read-GitCapabilityReady -Paths $Paths -Receipt $Receipt)
    $requestId = [string]$Request.request_id
    $journalPath = Join-Path $Paths.Transactions ($requestId + '.journal.json')
    $transactionLock = [IO.FileStream]::new(
        $Paths.GitTransactionLock, [IO.FileMode]::Open,
        [IO.FileAccess]::ReadWrite, [IO.FileShare]::None
    )
    $snapshot = $null
    try {
        if (Test-Path -LiteralPath $journalPath -PathType Leaf) {
            $existing = Read-GitJournal -Path $journalPath `
                -Paths $Paths -Manifest $manifest
            if ([string]$existing.install_id -cne [string]$Receipt.install_id -or
                [string]$existing.request_id -cne $requestId -or
                [string]$existing.request_sha256 -cne $RequestSha256) {
                throw 'git_local_commit_v1 replay journal binding mismatch.'
            }
            if ([string]$existing.phase -ceq 'verified' -and $null -ne $existing.output) {
                $verifiedOutput = Test-GitTransactionOutput `
                    -Manifest $manifest -Paths $Paths -Journal $existing `
                    -Payload $Request.payload
                $existing.output = $verifiedOutput
                Write-GitJournal -Path $journalPath -Journal $existing
                return $verifiedOutput
            }
            $currentHead = (Invoke-FixedGit -Manifest $manifest -Paths $Paths `
                -FixedCommand rev-parse-head).StandardOutput.Trim()
            if ($null -ne $existing.commit_oid -and
                $currentHead -ceq [string]$existing.commit_oid) {
                if ((Get-FileSha256 -Path ([string]$existing.index_path)) -ceq
                    [string]$existing.index_before_sha256) {
                    Publish-GitAlternateIndex -Manifest $manifest -Journal $existing
                    $existing.phase = 'index_published'
                    Write-GitJournal -Path $journalPath -Journal $existing
                }
                $output = Test-GitTransactionOutput -Manifest $manifest -Paths $Paths `
                    -Journal $existing -Payload $Request.payload
                $existing.phase = 'verified'
                $existing.output = $output
                Write-GitJournal -Path $journalPath -Journal $existing
                Remove-GitTransactionScratch -Journal $existing
                return $output
            }
            if ($currentHead -ceq [string]$existing.head_before -and
                (Get-FileSha256 -Path ([string]$existing.index_path)) -ceq
                    [string]$existing.index_before_sha256) {
                Remove-GitTransactionScratch -Journal $existing
                Remove-Item -LiteralPath $journalPath -Force
            } else {
                throw 'git_local_commit_v1 transaction is indeterminate; preserved journal requires recovery.'
            }
        }

        $snapshot = Get-GitTrackedChangeSnapshot -Manifest $manifest -Paths $Paths `
            -HoldChangedFiles
        Assert-GitSnapshotMatchesPayload -Snapshot $snapshot -Payload $Request.payload
        $indexPath = Join-Path ([string]$manifest.git_dir) 'index'
        $alternate = Join-Path $Paths.Transactions ($requestId + '.index')
        $stage = Join-Path ([string]$manifest.git_dir) 'index.lock'
        $backup = Join-Path ([string]$manifest.git_dir) `
            ('.rayman-git-local-commit-' + $requestId + '.backup')
        foreach ($path in @($alternate, $stage, $backup, $journalPath)) {
            if (Test-Path -LiteralPath $path) {
                throw "git_local_commit_v1 transaction path already exists: $path"
            }
        }
        Write-BytesWriteThrough -Path $alternate `
            -Bytes ([IO.File]::ReadAllBytes($indexPath)) -CreateNew
        $journal = [ordered]@{
            schema_version = 1
            install_id = [string]$Receipt.install_id
            request_id = $requestId
            request_sha256 = $RequestSha256
            phase = 'accepted'
            created_at_utc = [DateTimeOffset]::UtcNow.ToString('o')
            updated_at_utc = [DateTimeOffset]::UtcNow.ToString('o')
            head_before = [string]$snapshot.Head
            head_tree_before = [string]$snapshot.HeadTree
            index_path = $indexPath
            index_before_sha256 = [string]$snapshot.IndexSha256
            alternate_index_path = $alternate
            alternate_index_sha256 = $null
            stage_index_path = $stage
            backup_index_path = $backup
            remote_refs_before_sha256 = [string]$snapshot.RemoteRefsSha256
            other_refs_before_sha256 = [string]$snapshot.OtherRefsSha256
            tree_oid = $null
            commit_oid = $null
            commit_bytes_base64 = $null
            commit_message_sha256 = Get-BytesSha256 -Bytes (
                [Text.UTF8Encoding]::new($false, $true).GetBytes(
                    [string]$Request.payload.commit_message_utf8
                )
            )
            output = $null
        }
        Write-GitJournal -Path $journalPath -Journal $journal -Create
        if ($SelfTest -and $null -ne $script:GitSelfTestBeforeAddAction) {
            & $script:GitSelfTestBeforeAddAction
        }
        Invoke-FixedGit -Manifest $manifest -Paths $Paths `
            -FixedCommand add-update -IndexPath $alternate | Out-Null
        Assert-AlternateIndexMatchesPayload -Manifest $manifest -Paths $Paths `
            -IndexPath $alternate -Payload $Request.payload `
            -BaselineSnapshot $snapshot
        Close-GitSnapshotHandles -Snapshot $snapshot
        $snapshot = $null
        $tree = (Invoke-FixedGit -Manifest $manifest -Paths $Paths `
            -FixedCommand write-tree -IndexPath $alternate).StandardOutput.Trim()
        if ($tree -notmatch '^[0-9a-f]{40}$' -or $tree -ceq $journal.head_tree_before) {
            throw 'git_local_commit_v1 alternate index did not produce a changed tree.'
        }
        $journal.tree_oid = $tree
        $journal.alternate_index_sha256 = Get-FileSha256 -Path $alternate
        $journal.phase = 'index_prepared'
        Write-GitJournal -Path $journalPath -Journal $journal
        Invoke-GitSelfTestFault -Phase 'index_prepared'

        $commitBytes = New-GitCommitObjectBytes -Manifest $manifest -TreeOid $tree `
            -ParentOid ([string]$journal.head_before) `
            -Message ([string]$Request.payload.commit_message_utf8)
        $commitOid = (Invoke-FixedGit -Manifest $manifest -Paths $Paths `
            -FixedCommand hash-commit -StandardInput $commitBytes).StandardOutput.Trim()
        if ($commitOid -notmatch '^[0-9a-f]{40}$') {
            throw 'git_local_commit_v1 did not create a canonical commit object.'
        }
        $journal.commit_oid = $commitOid
        $journal.commit_bytes_base64 = [Convert]::ToBase64String($commitBytes)
        $journal.phase = 'objects_written'
        Write-GitJournal -Path $journalPath -Journal $journal
        Invoke-GitSelfTestFault -Phase 'objects_written'

        Stage-GitAlternateIndex -Manifest $manifest -Journal $journal
        $journal.phase = 'index_staged'
        Write-GitJournal -Path $journalPath -Journal $journal
        Invoke-GitSelfTestFault -Phase 'index_staged'

        $updateProtocol = "start`nupdate $([string]$manifest.allowed_ref) $commitOid $([string]$journal.head_before)`nprepare`ncommit`n"
        Invoke-FixedGit -Manifest $manifest -Paths $Paths -FixedCommand update-ref `
            -StandardInput ([Text.Encoding]::ASCII.GetBytes($updateProtocol)) | Out-Null
        $journal.phase = 'ref_updated'
        Write-GitJournal -Path $journalPath -Journal $journal
        Invoke-GitSelfTestFault -Phase 'ref_updated'

        Publish-GitAlternateIndex -Manifest $manifest -Journal $journal
        $journal.phase = 'index_published'
        Write-GitJournal -Path $journalPath -Journal $journal
        Invoke-GitSelfTestFault -Phase 'index_published'
        $output = Test-GitTransactionOutput -Manifest $manifest -Paths $Paths `
            -Journal $journal -Payload $Request.payload
        $journal.phase = 'verified'
        $journal.output = $output
        Write-GitJournal -Path $journalPath -Journal $journal
        Remove-GitTransactionScratch -Journal $journal
        return $output
    } catch {
        $originalFailure = $_
        Close-GitSnapshotHandles -Snapshot $snapshot
        if (Test-Path -LiteralPath $journalPath -PathType Leaf) {
            $failed = $null
            $currentHead = $null
            try {
                $failed = Read-GitJournal -Path $journalPath `
                    -Paths $Paths -Manifest $manifest
                $currentHead = (Invoke-FixedGit -Manifest $manifest -Paths $Paths `
                    -FixedCommand rev-parse-head).StandardOutput.Trim()
                if ($null -ne $failed.commit_oid -and
                    $currentHead -ceq [string]$failed.commit_oid) {
                    if ((Get-FileSha256 -Path ([string]$failed.index_path)) -ceq
                        [string]$failed.index_before_sha256) {
                        Publish-GitAlternateIndex -Manifest $manifest -Journal $failed
                        $failed.phase = 'index_published'
                        Write-GitJournal -Path $journalPath -Journal $failed
                    }
                    $output = Test-GitTransactionOutput -Manifest $manifest -Paths $Paths `
                        -Journal $failed -Payload $Request.payload
                    $failed.phase = 'verified'
                    $failed.output = $output
                    Write-GitJournal -Path $journalPath -Journal $failed
                    Remove-GitTransactionScratch -Journal $failed
                    return $output
                } elseif ($currentHead -ceq [string]$failed.head_before -and
                    (Get-FileSha256 -Path ([string]$failed.index_path)) -ceq
                        [string]$failed.index_before_sha256) {
                    Remove-GitTransactionScratch -Journal $failed
                    Remove-Item -LiteralPath $journalPath -Force
                } else {
                    throw 'git_local_commit_v1 transaction is indeterminate; preserved journal requires recovery.'
                }
            } catch {
                if ($null -ne $failed -and $null -ne $failed.commit_oid -and
                    $currentHead -ceq [string]$failed.commit_oid) {
                    throw "git_local_commit_v1 transaction is indeterminate after ref update; preserved journal requires recovery: $($_.Exception.Message)"
                }
                if ($_.Exception.Message.Contains('indeterminate')) { throw }
                throw "git_local_commit_v1 transaction recovery is indeterminate; preserved journal requires recovery: $($_.Exception.Message)"
            }
        }
        throw $originalFailure
    } finally {
        $transactionLock.Dispose()
    }
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
    if ([string]$request.operation -ceq 'identity_probe') {
        Assert-ExactProperties `
            -Document $request.payload `
            -Expected @() `
            -Label 'identity_probe payload'
    } elseif ([string]$request.operation -ceq $script:GitCapabilityId) {
        Assert-GitCommitPayload -Payload $request.payload -Receipt $Receipt
    } else {
        throw 'Broker operation is not installed. Arbitrary commands are never accepted.'
    }
    return $request
}

function Invoke-FixedOperation {
    param(
        [Parameter(Mandatory = $true)]$Request,
        [Parameter(Mandatory = $true)]$Receipt,
        [Parameter(Mandatory = $true)]$Paths,
        [Parameter(Mandatory = $true)][string]$RequestSha256
    )

    if ([string]$Request.operation -ceq 'identity_probe') {
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
    if ([string]$Request.operation -ceq $script:GitCapabilityId) {
        return Invoke-GitLocalCommitOperation `
            -Request $Request -Receipt $Receipt -Paths $Paths `
            -RequestSha256 $RequestSha256
    }
    throw 'Broker operation is not installed.'
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

function Open-BrokerWorkerLock {
    param([Parameter(Mandatory = $true)][string]$Path)

    try {
        return [IO.FileStream]::new(
            $Path,
            [IO.FileMode]::Open,
            [IO.FileAccess]::ReadWrite,
            [IO.FileShare]::None
        )
    } catch [IO.IOException] {
        $nativeCode = $_.Exception.HResult -band 0xffff
        $message = "Broker worker lock acquisition failed path=$Path " +
            "win32=$nativeCode"
        throw [IO.IOException]::new($message, $_.Exception)
    }
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
        [AllowNull()][string]$ErrorCode,
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
        error_code = $ErrorCode
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
        $resultPublished = $false
        try {
            $resultPath = Join-Path $Paths.Results ($requestId + $script:ResultSuffix)
            $started = [DateTimeOffset]::UtcNow
            $requestHash = Get-BytesSha256 -Bytes $claim.Bytes
            if (Test-Path -LiteralPath $resultPath -PathType Leaf) {
                $existingResult = Read-StrictJsonDocument `
                    -Path $resultPath -MaximumBytes 256KB `
                    -Label 'Existing broker replay result'
                if ([string]$existingResult.request_id -cne $requestId -or
                    [string]$existingResult.request_sha256 -notmatch '^[0-9a-f]{64}$') {
                    throw 'Existing broker replay result has an invalid protected identity.'
                }
                $resultPublished = $true
                $processed++
                continue
            }
            $operationName = 'invalid'
            $output = $null
            try {
                $untrustedEnvelope = ConvertFrom-StrictJsonBytes `
                    -Bytes $claim.Bytes -Label 'Broker request envelope classification'
                if ([string]$untrustedEnvelope.operation -ceq 'identity_probe' -or
                    [string]$untrustedEnvelope.operation -ceq $script:GitCapabilityId) {
                    $operationName = [string]$untrustedEnvelope.operation
                }
            } catch { }
            try {
                $request = Read-And-ValidateRequest `
                    -Bytes $claim.Bytes -ExpectedId $requestId -Receipt $Receipt
                $operationName = [string]$request.operation
                $output = Invoke-FixedOperation -Request $request `
                    -Receipt $Receipt -Paths $Paths -RequestSha256 $requestHash
                Publish-RequestResult `
                    -Paths $Paths -Receipt $Receipt -Identity $Identity `
                    -RequestId $requestId -OperationName $operationName `
                    -RequestSha256 $requestHash -Status 'success' -ExitCode 0 `
                    -Output $output -ErrorCode $null -ErrorMessage $null `
                    -StartedAt $started
                $resultPublished = $true
            } catch {
                if ($operationName -ceq $script:GitCapabilityId -and
                    $null -ne $output -and
                    [string]$output.mutation_phase -ceq 'verified') {
                    throw
                }
                $errorCode = 'request_rejected'
                $errorStatus = 'rejected'
                if ($operationName -ceq $script:GitCapabilityId) {
                    if ($_.Exception.Message.Contains('indeterminate')) {
                        $errorCode = 'indeterminate_requires_recovery'
                        $errorStatus = 'indeterminate_requires_recovery'
                    } else {
                        $errorCode = 'rejected_no_mutation'
                        $errorStatus = 'rejected_no_mutation'
                    }
                }
                Publish-RequestResult `
                    -Paths $Paths -Receipt $Receipt -Identity $Identity `
                    -RequestId $requestId -OperationName $operationName `
                    -RequestSha256 $requestHash -Status $errorStatus -ExitCode 2 `
                    -Output $null -ErrorCode $errorCode `
                    -ErrorMessage $_.Exception.Message -StartedAt $started
                $resultPublished = $true
            }
            $processed++
        } finally {
            $claim.Stream.Dispose()
            if ($resultPublished) {
                Remove-Item -LiteralPath $requestPath -Force -ErrorAction SilentlyContinue
            }
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
        [Parameter(Mandatory = $true)][int]$WaitSeconds,
        [Parameter(Mandatory = $true)]$Payload
    )

    [void](Assert-InstalledTaskContract -Paths $Paths -Receipt $Receipt)
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
        payload = $Payload
    })
    $resultPath = Join-Path $Paths.Results ($requestId + $script:ResultSuffix)
    $deadline = [DateTimeOffset]::UtcNow.AddSeconds($WaitSeconds)
    while ([DateTimeOffset]::UtcNow -lt $deadline) {
        if (Test-Path -LiteralPath $resultPath -PathType Leaf) {
            $result = Read-StrictJsonDocument `
                -Path $resultPath -MaximumBytes 256KB -Label 'Broker result'
            Assert-ExactProperties -Document $result -Label 'Broker result' -Expected @(
                'error', 'error_code', 'executor_account', 'executor_sid', 'exit_code',
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
            if ($OperationName -ceq 'identity_probe') {
                Assert-ExactProperties -Document $result.output -Label 'identity_probe output' `
                    -Expected @(
                        'account', 'language_mode', 'powershell_version', 'process_id',
                        'session_id', 'sid', 'user_profile'
                    )
                if ([string]$result.output.account -cne [string]$Receipt.user_account -or
                    [string]$result.output.sid -cne [string]$Receipt.user_sid) {
                    throw 'identity_probe output is not the installed target user.'
                }
            } else {
                Assert-ExactProperties -Document $result.output `
                    -Label 'git_local_commit_v1 output' -Expected @(
                    'child_process_policy', 'changed_path_count', 'changed_paths',
                    'changed_paths_digest', 'commit_message_sha256', 'commit_oid',
                    'git_sha256', 'head_after', 'head_before', 'hooks_disabled',
                    'index_after_sha256', 'index_before_sha256', 'mutation_phase',
                    'network_operation', 'other_refs_sha256', 'parent', 'post_clean', 'recovery_state',
                    'ref', 'remote_refs_sha256', 'repository_id', 'transaction_id',
                    'tree_oid'
                )
                $manifest = Read-GitCapabilityManifest -Paths $Paths -Receipt $Receipt
                $requestedPaths = @($Payload.changes | ForEach-Object { [string]$_.path })
                if ([string]$result.output.head_before -cne
                        [string]$Payload.expected_head_oid -or
                    [string]$result.output.parent -cne
                        [string]$Payload.expected_head_oid -or
                    [string]$result.output.head_after -cne
                        [string]$result.output.commit_oid -or
                    [string]$result.output.ref -cne [string]$manifest.allowed_ref -or
                    [string]$result.output.repository_id -cne
                        [string]$manifest.repository_id -or
                    [string]$result.output.git_sha256 -cne
                        [string]$manifest.git_executable_sha256 -or
                    [string]$result.output.mutation_phase -cne 'verified' -or
                    [string]$result.output.recovery_state -cne 'none' -or
                    [bool]$result.output.hooks_disabled -ne $true -or
                    [bool]$result.output.network_operation -ne $false -or
                    [bool]$result.output.post_clean -ne $true -or
                    [string]$result.output.child_process_policy -cne 'single_process' -or
                    [int]$result.output.changed_path_count -ne $requestedPaths.Count -or
                    (@($result.output.changed_paths) -join "`n") -cne
                        ($requestedPaths -join "`n") -or
                    [string]$result.output.changed_paths_digest -cne
                        (Get-GitChangesDigest -Changes $Payload.changes)) {
                    throw 'git_local_commit_v1 result contract does not match this request.'
                }
                $post = Get-GitTrackedChangeSnapshot -Manifest $manifest -Paths $Paths `
                    -AllowNoChanges
                try {
                    if ([string]$post.Head -cne [string]$result.output.commit_oid -or
                        @($post.Changes).Count -ne 0 -or
                        [string]$post.RemoteRefsSha256 -cne
                            [string]$result.output.remote_refs_sha256 -or
                        [string]$post.OtherRefsSha256 -cne
                            [string]$result.output.other_refs_sha256) {
                        throw 'git_local_commit_v1 client postcondition recheck failed.'
                    }
                } finally { Close-GitSnapshotHandles -Snapshot $post }
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
    Invoke-BrokerPortableStaticSelfTest
    $managedRoot = Get-NormalizedAbsolutePath `
        -Path ([IO.Path]::GetFullPath([IO.Path]::GetTempPath())) `
        -Label 'Broker self-test process temp root'
    [void](Assert-NoReparseAncestors -Path $managedRoot `
        -Label 'Broker self-test process temp root' -RequireLeaf)
    if (-not (Test-Path -LiteralPath $managedRoot -PathType Container)) {
        throw 'Broker self-test process temp root is not a directory.'
    }
    $testRoot = Assert-ChildPath `
        -Child (Join-Path $managedRoot (
            'codex-powershell-broker-selftest-' + [Guid]::NewGuid().ToString('N')
        )) `
        -Parent $managedRoot `
        -Label 'Broker self-test root'
    New-Item -ItemType Directory -Path $testRoot | Out-Null
    try {
        $parseTokens = $null
        $parseErrors = $null
        $selfAst = [Management.Automation.Language.Parser]::ParseFile(
            $PSCommandPath, [ref]$parseTokens, [ref]$parseErrors
        )
        if (@($parseErrors).Count -ne 0) {
            throw 'Broker self-test could not parse its own source.'
        }
        $clientDefinitions = @($selfAst.FindAll({
            param($node)
            $node -is [Management.Automation.Language.FunctionDefinitionAst] -and
                [string]$node.Name -ceq 'Invoke-BrokerClient'
        }, $true))
        if ($clientDefinitions.Count -ne 1) {
            throw 'Broker self-test requires exactly one Invoke-BrokerClient function.'
        }
        $taskContractCalls = @($clientDefinitions[0].Body.FindAll({
            param($node)
            $node -is [Management.Automation.Language.CommandAst] -and
                [string]$node.GetCommandName() -ceq
                    'Assert-InstalledTaskContract'
        }, $true))
        if ($taskContractCalls.Count -ne 1) {
            throw 'Broker client preflight must call the task contract exactly once.'
        }
        $taskContractConversion = $taskContractCalls[0].Parent.Parent.Parent
        if ($taskContractConversion -isnot
                [Management.Automation.Language.ConvertExpressionAst] -or
            [string]$taskContractConversion.Type.TypeName.FullName -cne 'void') {
            throw 'Broker client preflight must suppress the internal task snapshot.'
        }
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
        $testTransactions = Join-Path $testInstall $script:GitTransactionDirectoryName
        $testHooks = Join-Path $testInstall 'empty-hooks'
        New-Item -ItemType Directory -Path $testInstall | Out-Null
        New-Item -ItemType Directory -Path $testRequests | Out-Null
        New-Item -ItemType Directory -Path $testResults | Out-Null
        New-Item -ItemType Directory -Path $testTransactions | Out-Null
        New-Item -ItemType Directory -Path $testHooks | Out-Null
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
        [IO.File]::WriteAllBytes($paths.GitTransactionLock, [byte[]]::new(0))
        if ($IsWindows) {
            $heldWorkerLock = [IO.FileStream]::new(
                $paths.WorkerLock, [IO.FileMode]::Open,
                [IO.FileAccess]::ReadWrite, [IO.FileShare]::None
            )
            try {
                $workerLockError = $null
                try { [void](Open-BrokerWorkerLock -Path $paths.WorkerLock) }
                catch { $workerLockError = $_.Exception.Message }
                if ($null -eq $workerLockError -or
                    -not $workerLockError.Contains(
                        'Broker worker lock acquisition failed',
                        [StringComparison]::Ordinal
                    ) -or
                    $workerLockError -notmatch 'win32=(32|33)') {
                    throw "Worker lock contention diagnostic drifted: $workerLockError"
                }
            } finally { $heldWorkerLock.Dispose() }
            $releasedWorkerLock = Open-BrokerWorkerLock -Path $paths.WorkerLock
            $releasedWorkerLock.Dispose()
        }

        $fixtureRoot = Join-Path $testRoot 'repository'
        New-Item -ItemType Directory -Path $fixtureRoot | Out-Null
        $fixtureGit = 'C:\Program Files\Git\mingw64\bin\git.exe'
        foreach ($invocation in @(
            @('init', '-b', 'main'),
            @('config', 'core.filemode', 'false'),
            @('config', 'core.ignorecase', 'true'),
            @('remote', 'add', 'origin', 'https://github.com/qinrm-lab/RaymanCodingSkill.git'),
            @('config', 'branch.main.remote', 'origin'),
            @('config', 'branch.main.merge', 'refs/heads/main'),
            @('config', 'branch.main.vscode-merge-base', 'origin/main')
        )) {
            & $fixtureGit -C $fixtureRoot @invocation | Out-Null
            if ($LASTEXITCODE -ne 0) {
                throw "git_local_commit_v1 fixture setup failed: $($invocation -join ' ')"
            }
        }
        [IO.File]::WriteAllText(
            (Join-Path $fixtureRoot '.gitattributes'),
            "* text=auto eol=lf`n`n*.png binary`n*.jpg binary`n*.jpeg binary`n*.gif binary`n*.ico binary`n*.pdf binary`n*.exe binary`n",
            [Text.UTF8Encoding]::new($false)
        )
        [IO.File]::WriteAllText(
            (Join-Path $fixtureRoot 'tracked.txt'), "before`n",
            [Text.UTF8Encoding]::new($false)
        )
        [IO.File]::WriteAllText(
            (Join-Path $fixtureRoot 'unchanged.txt'), "stable`n",
            [Text.UTF8Encoding]::new($false)
        )
        $mixedUpperPath = Join-Path $fixtureRoot 'README.md'
        $mixedLowerRoot = Join-Path $fixtureRoot 'crates'
        $mixedLowerPath = Join-Path $mixedLowerRoot 'lower.txt'
        New-Item -ItemType Directory -Path $mixedLowerRoot | Out-Null
        [IO.File]::WriteAllText(
            $mixedUpperPath, "upper-before`n",
            [Text.UTF8Encoding]::new($false)
        )
        [IO.File]::WriteAllText(
            $mixedLowerPath, "lower-before`n",
            [Text.UTF8Encoding]::new($false)
        )
        & $fixtureGit -C $fixtureRoot add -- .
        if ($LASTEXITCODE -ne 0) { throw 'git_local_commit_v1 fixture add failed.' }
        & $fixtureGit -C $fixtureRoot -c 'user.name=rayman' `
            -c 'user.email=32691594@qq.com' commit -m 'test: baseline' | Out-Null
        if ($LASTEXITCODE -ne 0) { throw 'git_local_commit_v1 fixture commit failed.' }
        $fixtureSignature = Get-AuthenticodeSignature -LiteralPath $fixtureGit
        $fixtureGitDir = Join-Path $fixtureRoot '.git'
        $fixtureInfoAttributes = Join-Path (Join-Path $fixtureGitDir 'info') 'attributes'
        $capabilityManifest = [ordered]@{
            schema_version = 1
            capability_id = $script:GitCapabilityId
            install_id = $installId
            repository_id = [Guid]::NewGuid().ToString('N')
            repository_root = $fixtureRoot
            repository_root_identity = Get-StrongPathIdentity `
                -Path $fixtureRoot -Directory $true
            git_dir = $fixtureGitDir
            git_dir_identity = Get-StrongPathIdentity `
                -Path $fixtureGitDir -Directory $true
            allowed_ref = 'refs/heads/main'
            git_executable_path = $fixtureGit
            git_executable_sha256 = Get-FileSha256 -Path $fixtureGit
            git_executable_identity = Get-StrongPathIdentity `
                -Path $fixtureGit -Directory $false
            git_signer_subject = [string]$fixtureSignature.SignerCertificate.Subject
            git_signer_thumbprint = [string]$fixtureSignature.SignerCertificate.Thumbprint
            object_format = 'sha1'
            author_name = 'rayman'
            author_email = '32691594@qq.com'
            hooks_root = $paths.Hooks
            transactions_root = $paths.Transactions
            git_config_sha256 = Get-FileSha256 -Path (Join-Path $fixtureGitDir 'config')
            gitattributes_sha256 = Get-FileSha256 -Path (Join-Path $fixtureRoot '.gitattributes')
            info_attributes_sha256 = if (Test-Path -LiteralPath $fixtureInfoAttributes) {
                Get-FileSha256 -Path $fixtureInfoAttributes
            } else { $null }
            info_exclude_sha256 = Get-FileSha256 `
                -Path (Join-Path (Join-Path $fixtureGitDir 'info') 'exclude')
            tracked_only = $true
            allow_untracked = $false
            allow_pre_staged = $false
            authorization_mode = 'persistent_install_grant'
            local_commit_only = $true
            push_allowed = $false
            confirmation_required = $false
            created_at_utc = [DateTimeOffset]::UtcNow.ToString('o')
        }
        Write-JsonAtomic -Path $paths.GitCapabilityManifest `
            -Document $capabilityManifest
        $capabilityManifestHash = Get-FileSha256 -Path $paths.GitCapabilityManifest
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
            capabilities = @('identity_probe', $script:GitCapabilityId)
            git_capability_manifest_path = $paths.GitCapabilityManifest
            git_capability_manifest_sha256 = $capabilityManifestHash
        })
        $receipt = Read-BrokerReceipt -Paths $paths
        $identity = Assert-WorkerBinding -Receipt $receipt -WorkerPath $testWorker
        $now = [DateTimeOffset]::UtcNow
        & $assertRejected {
            [void](New-GitCommitPayload -Paths $paths -Receipt $receipt `
                -Message 'test: reject uncommitted capability')
        } 'missing installer ready marker'
        $readySecurity = New-ExpectedFileSecurity `
            -UserSid $identity.Sid -SandboxSid $identity.Sid
        Write-JsonAtomic -Path $paths.GitCapabilityReady `
            -Security $readySecurity -Document ([ordered]@{
            schema_version = 1
            install_id = $installId
            worker_sha256 = $workerHash
            git_capability_manifest_sha256 = $capabilityManifestHash
            ready_at_utc = [DateTimeOffset]::UtcNow.ToString('o')
        })

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

        & $assertRejected {
            [void](ConvertFrom-StrictJsonBytes `
                -Bytes ([Text.Encoding]::UTF8.GetBytes('{"x":1,"X":2}')) `
                -Label 'duplicate-property self-test')
        } 'case-colliding duplicate JSON property'

        [IO.File]::WriteAllText(
            $fixtureInfoAttributes, '*.txt filter=unexpected',
            [Text.UTF8Encoding]::new($false)
        )
        & $assertRejected {
            [void](New-GitCommitPayload -Paths $paths -Receipt $receipt `
                -Message 'test: reject info attributes')
        } 'info/attributes override'
        Remove-Item -LiteralPath $fixtureInfoAttributes -Force

        $heldTree = Join-Path $fixtureRoot 'held-tree'
        $heldNested = Join-Path $heldTree 'nested'
        New-Item -ItemType Directory -Path $heldNested | Out-Null
        $heldFile = Join-Path $heldNested 'bounded.txt'
        [IO.File]::WriteAllText(
            $heldFile, "held`n", [Text.UTF8Encoding]::new($false)
        )
        $heldRead = Read-BoundedRegularFileHeld `
            -Path $heldFile -MaximumBytes 1KB `
            -RepositoryRoot $fixtureRoot `
            -RepositoryIdentity ([string]$capabilityManifest.repository_root_identity) `
            -Hold
        try {
            if ([Text.Encoding]::UTF8.GetString($heldRead.Bytes) -cne "held`n") {
                throw 'Held tracked-file self-test returned different bytes.'
            }
            & $assertRejected {
                Move-Item -LiteralPath $heldNested `
                    -Destination (Join-Path $heldTree 'renamed')
            } 'held tracked-file ancestor rename'
        } finally { $heldRead.Lease.Dispose() }
        Remove-Item -LiteralPath $heldTree -Recurse -Force

        $junctionTarget = Join-Path $testRoot 'junction-target'
        $junctionPath = Join-Path $fixtureRoot 'junction-tree'
        New-Item -ItemType Directory -Path $junctionTarget | Out-Null
        [IO.File]::WriteAllText(
            (Join-Path $junctionTarget 'outside.txt'), "outside`n",
            [Text.UTF8Encoding]::new($false)
        )
        New-Item -ItemType Junction -Path $junctionPath `
            -Target $junctionTarget | Out-Null
        try {
            & $assertRejected {
                $unexpectedHeld = $null
                try {
                    $unexpectedHeld = Read-BoundedRegularFileHeld `
                        -Path (Join-Path $junctionPath 'outside.txt') `
                        -MaximumBytes 1KB -RepositoryRoot $fixtureRoot `
                        -RepositoryIdentity (
                            [string]$capabilityManifest.repository_root_identity
                        ) -Hold
                } finally {
                    if ($null -ne $unexpectedHeld) {
                        $unexpectedHeld.Lease.Dispose()
                    }
                }
            } 'tracked-file junction ancestor'
        } finally {
            Remove-Item -LiteralPath $junctionPath -Force
            Remove-Item -LiteralPath $junctionTarget -Recurse -Force
        }

        [IO.File]::WriteAllText(
            (Join-Path $fixtureRoot 'tracked.txt'), "commit-one`r`n",
            [Text.UTF8Encoding]::new($false)
        )
        [IO.File]::WriteAllText(
            $mixedUpperPath, "upper-after`n",
            [Text.UTF8Encoding]::new($false)
        )
        [IO.File]::WriteAllText(
            $mixedLowerPath, "lower-after`n",
            [Text.UTF8Encoding]::new($false)
        )
        $directHead = (& $fixtureGit -C $fixtureRoot rev-parse HEAD).Trim()
        & $fixtureGit -C $fixtureRoot update-ref refs/heads/selftest-target $directHead
        if ($LASTEXITCODE -ne 0) {
            throw 'git_local_commit_v1 symbolic-ref self-test target setup failed.'
        }
        & $fixtureGit -C $fixtureRoot symbolic-ref refs/heads/main `
            refs/heads/selftest-target
        if ($LASTEXITCODE -ne 0) {
            throw 'git_local_commit_v1 symbolic registered-ref setup failed.'
        }
        & $assertRejected {
            [void](New-GitCommitPayload -Paths $paths -Receipt $receipt `
                -Message 'test: reject symbolic registered ref')
        } 'symbolic registered branch ref'
        & $fixtureGit -C $fixtureRoot update-ref --no-deref `
            refs/heads/main $directHead
        if ($LASTEXITCODE -ne 0) {
            throw 'git_local_commit_v1 symbolic registered-ref restore failed.'
        }
        & $fixtureGit -C $fixtureRoot update-ref -d refs/heads/selftest-target
        if ($LASTEXITCODE -ne 0) {
            throw 'git_local_commit_v1 symbolic-ref self-test target cleanup failed.'
        }
        $gitPayload = $null
        $gitAcl = Get-Acl -LiteralPath $fixtureGitDir
        $gitAclSddl = $gitAcl.GetSecurityDescriptorSddlForm(
            [Security.AccessControl.AccessControlSections]::All
        )
        $currentSid = [Security.Principal.WindowsIdentity]::GetCurrent().User
        $denyCreate = [Security.AccessControl.FileSystemAccessRule]::new(
            $currentSid,
            [Security.AccessControl.FileSystemRights]::CreateFiles,
            [Security.AccessControl.InheritanceFlags]::None,
            [Security.AccessControl.PropagationFlags]::None,
            [Security.AccessControl.AccessControlType]::Deny
        )
        [void]$gitAcl.AddAccessRule($denyCreate)
        Set-Acl -LiteralPath $fixtureGitDir -AclObject $gitAcl
        try {
            $clientCulture = [Globalization.CultureInfo]::CurrentCulture
            try {
                [Globalization.CultureInfo]::CurrentCulture =
                    [Globalization.CultureInfo]::GetCultureInfo('zh-CN')
                $gitPayload = New-GitCommitPayload -Paths $paths -Receipt $receipt `
                    -Message 'test: fixed local commit'
            } finally {
                [Globalization.CultureInfo]::CurrentCulture = $clientCulture
            }
            if (Test-Path -LiteralPath (Join-Path $fixtureGitDir 'index.lock')) {
                throw 'git_local_commit_v1 client snapshot created index.lock under a read-only Git directory.'
            }
        } finally {
            $restoreGitAcl = [Security.AccessControl.DirectorySecurity]::new()
            $restoreGitAcl.SetSecurityDescriptorSddlForm($gitAclSddl)
            Set-Acl -LiteralPath $fixtureGitDir -AclObject $restoreGitAcl
        }
        if ($null -eq $gitPayload) {
            throw 'git_local_commit_v1 read-only client snapshot returned no payload.'
        }
        $mixedPaths = @($gitPayload.changes | ForEach-Object { [string]$_.path })
        $expectedMixedPaths = @('README.md', 'crates/lower.txt', 'tracked.txt')
        if (($mixedPaths -join "`n") -cne ($expectedMixedPaths -join "`n")) {
            throw "git_local_commit_v1 mixed-case ordinal self-test returned: $($mixedPaths -join ',')"
        }
        $unsortedChanges = @($gitPayload.changes | ForEach-Object { $_ })
        [Array]::Reverse($unsortedChanges)
        & $assertRejected {
            Assert-GitCommitPayload -Receipt $receipt -Payload ([ordered]@{
                capability_manifest_sha256 = $gitPayload.capability_manifest_sha256
                expected_head_oid = $gitPayload.expected_head_oid
                changes = $unsortedChanges
                commit_message_utf8 = $gitPayload.commit_message_utf8
            })
        } 'non-ordinal request path order'
        & $assertRejected {
            Assert-GitCommitPayload -Receipt $receipt -Payload ([ordered]@{
                capability_manifest_sha256 = $gitPayload.capability_manifest_sha256
                expected_head_oid = $gitPayload.expected_head_oid
                changes = @($gitPayload.changes[0], $gitPayload.changes[0])
                commit_message_utf8 = $gitPayload.commit_message_utf8
            })
        } 'duplicate request path'
        $rawCrlfOid = Get-GitBlobOid -Bytes (
            [Text.UTF8Encoding]::new($false).GetBytes("commit-one`r`n")
        )
        $crlfChanges = @($gitPayload.changes | Where-Object {
            [string]$_.path -ceq 'tracked.txt'
        })
        $crlfPath = if ($crlfChanges.Count -eq 0) { '' } else {
            [string]$crlfChanges[0].path
        }
        $filteredCrlfOid = if ($crlfChanges.Count -eq 0) { '' } else {
            [string]$crlfChanges[0].git_blob_oid_after
        }
        if ($crlfChanges.Count -ne 1 -or $crlfPath -cne 'tracked.txt' -or
            $filteredCrlfOid -ceq $rawCrlfOid) {
            throw "git_local_commit_v1 CRLF self-test did not bind the filtered index blob OID. count=$($crlfChanges.Count) path=$crlfPath raw=$rawCrlfOid filtered=$filteredCrlfOid"
        }
        $gitRequestId = New-SelfTestRequest `
            -Paths $paths -OperationName $script:GitCapabilityId `
            -Created $now -Expires $now.AddSeconds(120) -Payload $gitPayload `
            -InstallId $installId
        $workerCulture = [Globalization.CultureInfo]::CurrentCulture
        try {
            [Globalization.CultureInfo]::CurrentCulture =
                [Globalization.CultureInfo]::GetCultureInfo('zh-CN')
            [void](Invoke-WorkerCycle -Paths $paths -Receipt $receipt -Identity $identity)
        } finally {
            [Globalization.CultureInfo]::CurrentCulture = $workerCulture
        }
        $gitResult = Read-StrictJsonDocument `
            -Path (Join-Path $paths.Results ($gitRequestId + $script:ResultSuffix)) `
            -MaximumBytes 256KB -Label 'Self-test Git commit result'
        if ([string]$gitResult.status -cne 'success' -or
            [string]$gitResult.output.parent -cne [string]$gitPayload.expected_head_oid -or
            [string]$gitResult.output.head_after -cne [string]$gitResult.output.commit_oid -or
            [bool]$gitResult.output.post_clean -ne $true -or
            [bool]$gitResult.output.network_operation -ne $false) {
            throw "git_local_commit_v1 positive self-test failed: $($gitResult.error)"
        }

        [IO.File]::WriteAllText(
            (Join-Path $fixtureRoot 'tracked.txt'), "commit-two`r`n",
            [Text.UTF8Encoding]::new($false)
        )
        $faultPayload = New-GitCommitPayload -Paths $paths -Receipt $receipt `
            -Message 'test: recover transaction'
        $badPayload = [ordered]@{
            capability_manifest_sha256 = $faultPayload.capability_manifest_sha256
            expected_head_oid = $faultPayload.expected_head_oid
            changes = $faultPayload.changes
            commit_message_utf8 = $faultPayload.commit_message_utf8
            command = 'git push'
        }
        $badPayloadId = New-SelfTestRequest `
            -Paths $paths -OperationName $script:GitCapabilityId `
            -Created $now -Expires $now.AddSeconds(120) -Payload $badPayload `
            -InstallId $installId
        [void](Invoke-WorkerCycle -Paths $paths -Receipt $receipt -Identity $identity)
        $badPayloadResult = Read-StrictJsonDocument `
            -Path (Join-Path $paths.Results ($badPayloadId + $script:ResultSuffix)) `
            -MaximumBytes 256KB -Label 'Self-test arbitrary Git payload result'
        if ([string]$badPayloadResult.status -cne 'rejected_no_mutation') {
            throw 'git_local_commit_v1 accepted an arbitrary command property.'
        }

        $script:GitSelfTestBeforeAddAction = {
            [IO.File]::WriteAllText(
                (Join-Path $fixtureRoot 'unchanged.txt'), "raced`n",
                [Text.UTF8Encoding]::new($false)
            )
        }.GetNewClosure()
        $raceId = New-SelfTestRequest `
            -Paths $paths -OperationName $script:GitCapabilityId `
            -Created $now -Expires $now.AddSeconds(120) -Payload $faultPayload `
            -InstallId $installId
        [void](Invoke-WorkerCycle -Paths $paths -Receipt $receipt -Identity $identity)
        $script:GitSelfTestBeforeAddAction = $null
        $raceResult = Read-StrictJsonDocument `
            -Path (Join-Path $paths.Results ($raceId + $script:ResultSuffix)) `
            -MaximumBytes 256KB -Label 'Self-test unrequested index race result'
        if ([string]$raceResult.status -cne 'rejected_no_mutation') {
            throw 'git_local_commit_v1 accepted an unrequested concurrent tracked change.'
        }
        [IO.File]::WriteAllText(
            (Join-Path $fixtureRoot 'unchanged.txt'), "stable`n",
            [Text.UTF8Encoding]::new($false)
        )

        $headBeforeFault = [string]$faultPayload.expected_head_oid
        $indexBeforeFault = Get-FileSha256 -Path (Join-Path $fixtureGitDir 'index')
        $script:GitSelfTestFaultPhase = 'index_staged'
        $rollbackId = New-SelfTestRequest `
            -Paths $paths -OperationName $script:GitCapabilityId `
            -Created $now -Expires $now.AddSeconds(120) -Payload $faultPayload `
            -InstallId $installId
        [void](Invoke-WorkerCycle -Paths $paths -Receipt $receipt -Identity $identity)
        $script:GitSelfTestFaultPhase = $null
        $rollbackResult = Read-StrictJsonDocument `
            -Path (Join-Path $paths.Results ($rollbackId + $script:ResultSuffix)) `
            -MaximumBytes 256KB -Label 'Self-test pre-ref fault result'
        $headAfterRollback = (& $fixtureGit -C $fixtureRoot rev-parse HEAD).Trim()
        if ([string]$rollbackResult.status -cne 'rejected_no_mutation' -or
            $headAfterRollback -cne $headBeforeFault -or
            (Get-FileSha256 -Path (Join-Path $fixtureGitDir 'index')) -cne
                $indexBeforeFault -or
            (Test-Path -LiteralPath (Join-Path $fixtureGitDir 'index.lock')) -or
            (Test-Path -LiteralPath (Join-Path $paths.Transactions `
                ($rollbackId + '.journal.json')))) {
            throw 'git_local_commit_v1 pre-ref fault did not preserve HEAD/index/dirty worktree.'
        }

        $script:GitSelfTestFaultPhase = 'ref_updated'
        $forwardId = New-SelfTestRequest `
            -Paths $paths -OperationName $script:GitCapabilityId `
            -Created $now -Expires $now.AddSeconds(120) -Payload $faultPayload `
            -InstallId $installId
        [void](Invoke-WorkerCycle -Paths $paths -Receipt $receipt -Identity $identity)
        $script:GitSelfTestFaultPhase = $null
        $forwardResult = Read-StrictJsonDocument `
            -Path (Join-Path $paths.Results ($forwardId + $script:ResultSuffix)) `
            -MaximumBytes 256KB -Label 'Self-test post-ref recovery result'
        if ([string]$forwardResult.status -cne 'success' -or
            [bool]$forwardResult.output.post_clean -ne $true) {
            throw "git_local_commit_v1 post-ref forward recovery failed: $($forwardResult.error)"
        }

        $untracked = Join-Path $fixtureRoot 'untracked.txt'
        [IO.File]::WriteAllText($untracked, 'untracked', [Text.UTF8Encoding]::new($false))
        & $assertRejected {
            [void](New-GitCommitPayload -Paths $paths -Receipt $receipt `
                -Message 'test: reject untracked')
        } 'untracked file'
        Remove-Item -LiteralPath $untracked -Force

        [IO.File]::WriteAllText(
            (Join-Path $fixtureRoot 'tracked.txt'), "staged`n",
            [Text.UTF8Encoding]::new($false)
        )
        & $fixtureGit -C $fixtureRoot add -- tracked.txt
        if ($LASTEXITCODE -ne 0) { throw 'git_local_commit_v1 staged fixture setup failed.' }
        & $assertRejected {
            [void](New-GitCommitPayload -Paths $paths -Receipt $receipt `
                -Message 'test: reject staged')
        } 'pre-staged index'
        & $fixtureGit -C $fixtureRoot reset --mixed HEAD | Out-Null
        if ($LASTEXITCODE -ne 0) { throw 'git_local_commit_v1 staged fixture reset failed.' }

        $filterSentinel = Join-Path $testRoot 'filter-must-not-run.txt'
        $filterScript = Join-Path $testRoot 'filter-sentinel.ps1'
        [IO.File]::WriteAllText(
            $filterScript,
            "[IO.File]::WriteAllText('$($filterSentinel.Replace("'", "''"))','ran')",
            [Text.UTF8Encoding]::new($false)
        )
        $absolutePwsh = [Diagnostics.Process]::GetCurrentProcess().MainModule.FileName
        & $fixtureGit -C $fixtureRoot config filter.evil.clean `
            ('"' + $absolutePwsh + '" -NoProfile -File "' + $filterScript + '"')
        & $fixtureGit -C $fixtureRoot config filter.evil.required true
        [IO.File]::WriteAllText(
            (Join-Path $fixtureRoot '.gitattributes'), "tracked.txt filter=evil`n",
            [Text.UTF8Encoding]::new($false)
        )
        [IO.File]::WriteAllText(
            (Join-Path $fixtureRoot 'tracked.txt'), "filter-attempt`n",
            [Text.UTF8Encoding]::new($false)
        )
        $filterIndex = Join-Path $paths.Transactions 'filter-job.index'
        [IO.File]::Copy((Join-Path $fixtureGitDir 'index'), $filterIndex)
        $filterBlocked = $false
        try {
            [void](Invoke-FixedGit -Manifest $capabilityManifest -Paths $paths `
                -FixedCommand add-update -IndexPath $filterIndex)
        } catch { $filterBlocked = $true }
        if (-not $filterBlocked -or (Test-Path -LiteralPath $filterSentinel)) {
            throw "git_local_commit_v1 single-process Job did not block an external clean filter. blocked=$filterBlocked sentinel=$(Test-Path -LiteralPath $filterSentinel)"
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
        if (Test-Path -LiteralPath `
            (Join-Path $paths.Requests ($validId + $script:RequestSuffix))) {
            throw 'Self-test replay left a permanently stuck request file.'
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
[void](Assert-NoReparseAncestors -Path $paths.Transactions `
    -Label 'Git transaction root' -RequireLeaf)
[void](Assert-NoReparseAncestors -Path $paths.Hooks `
    -Label 'Protected hooks root' -RequireLeaf)
$receipt = Read-BrokerReceipt -Paths $paths

if ($ProcessOnce) {
    $identity = Assert-WorkerBinding -Receipt $receipt -WorkerPath $PSCommandPath
    [void](Assert-InstalledTaskContract -Paths $paths -Receipt $receipt)
    Assert-InstalledAclContract -Paths $paths -Receipt $receipt
    [void](Invoke-WorkerCycle -Paths $paths -Receipt $receipt -Identity $identity)
    return
}

if ($Worker) {
    $identity = Assert-WorkerBinding -Receipt $receipt -WorkerPath $PSCommandPath
    $task = Assert-InstalledTaskContract -Paths $paths -Receipt $receipt
    Assert-InstalledAclContract -Paths $paths -Receipt $receipt
    $workerLock = Open-BrokerWorkerLock -Path $paths.WorkerLock
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
$payload = if ($Operation -ceq 'identity_probe') {
    if ($Yes -or -not [string]::IsNullOrEmpty($CommitMessage)) {
        throw 'identity_probe does not accept -Yes or -CommitMessage.'
    }
    [ordered]@{}
} else {
    if (-not $Yes) {
        throw 'git_local_commit_v1 requires the agent-side standing-authority assertion -Yes; do not ask the user again.'
    }
    if ([string]::IsNullOrEmpty($CommitMessage)) {
        throw 'git_local_commit_v1 requires -CommitMessage.'
    }
    New-GitCommitPayload -Paths $paths -Receipt $receipt -Message $CommitMessage
}
$result = Invoke-BrokerClient `
    -Paths $paths -Receipt $receipt `
    -OperationName $Operation -WaitSeconds $TimeoutSeconds -Payload $payload
$result | ConvertTo-Json -Depth 16
