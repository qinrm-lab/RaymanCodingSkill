//! One native process, assigned to a one-process Job before its first
//! instruction. Only the fixed Git backend can call this module in production.
use anyhow::{Result, bail};
use std::os::windows::{
    ffi::OsStrExt,
    io::{AsRawHandle, FromRawHandle, OwnedHandle},
};
use std::{
    collections::BTreeMap,
    fs::File,
    io::{Read, Write},
    path::Path,
};
use windows_sys::Win32::{
    Foundation::{HANDLE, HANDLE_FLAG_INHERIT, SetHandleInformation, WAIT_OBJECT_0},
    Security::SECURITY_ATTRIBUTES,
    System::{
        JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_ACTIVE_PROCESS,
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JobObjectExtendedLimitInformation, SetInformationJobObject,
        },
        Pipes::CreatePipe,
        Threading::{
            CREATE_NO_WINDOW, CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, CreateProcessW,
            DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT, GetExitCodeProcess,
            InitializeProcThreadAttributeList, PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
            PROCESS_INFORMATION, ResumeThread, STARTF_USESTDHANDLES, STARTUPINFOEXW,
            TerminateProcess, UpdateProcThreadAttribute, WaitForSingleObject,
        },
    },
};

pub(super) struct ProcessOutput {
    pub exit_code: u32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

fn owned(handle: HANDLE) -> Result<OwnedHandle> {
    if handle.is_null() {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(unsafe { OwnedHandle::from_raw_handle(handle as _) })
}
fn wide(text: &std::ffi::OsStr) -> Result<Vec<u16>> {
    let data: Vec<u16> = text.encode_wide().collect();
    if data.contains(&0) {
        bail!("NUL in native process parameter");
    }
    Ok(data.into_iter().chain(Some(0)).collect())
}
fn quote(value: &str) -> String {
    let mut out = String::from("\"");
    let mut slash = 0;
    for c in value.chars() {
        if c == '\\' {
            slash += 1;
            continue;
        }
        if c == '"' {
            out.extend(std::iter::repeat_n('\\', slash * 2 + 1));
            out.push(c);
        } else {
            out.extend(std::iter::repeat_n('\\', slash));
            out.push(c);
        }
        slash = 0;
    }
    out.extend(std::iter::repeat_n('\\', slash * 2));
    out.push('"');
    out
}
fn pipe() -> Result<(OwnedHandle, OwnedHandle)> {
    let security = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: std::ptr::null_mut(),
        bInheritHandle: 1,
    };
    let mut read = std::ptr::null_mut();
    let mut write = std::ptr::null_mut();
    if unsafe { CreatePipe(&mut read, &mut write, &security, 0) } == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok((owned(read)?, owned(write)?))
}
fn private(handle: &OwnedHandle) -> Result<()> {
    if unsafe { SetHandleInformation(handle.as_raw_handle() as _, HANDLE_FLAG_INHERIT, 0) } == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}
fn bounded(mut stream: File, limit: usize) -> Result<Vec<u8>> {
    let mut output = Vec::new();
    std::io::Read::by_ref(&mut stream)
        .take((limit + 1) as u64)
        .read_to_end(&mut output)?;
    if output.len() > limit {
        bail!("fixed process output exceeded limit");
    }
    Ok(output)
}
struct Guard {
    process: OwnedHandle,
    _thread: OwnedHandle,
    _job: OwnedHandle,
}
impl Drop for Guard {
    fn drop(&mut self) {
        let mut code = 0;
        if unsafe { GetExitCodeProcess(self.process.as_raw_handle() as _, &mut code) } == 0
            || code == 259
        {
            unsafe {
                TerminateProcess(self.process.as_raw_handle() as _, 1);
                WaitForSingleObject(self.process.as_raw_handle() as _, 5000);
            }
        }
    }
}

pub(super) fn run_single_process(
    program: &Path,
    args: &[String],
    cwd: &Path,
    environment: &BTreeMap<String, String>,
    stdin: &[u8],
    timeout_ms: u32,
    output_limit: usize,
) -> Result<ProcessOutput> {
    run_process(
        program,
        args,
        cwd,
        environment,
        stdin,
        ProcessLimits {
            timeout_ms,
            output_limit,
            allow_descendants: false,
        },
    )
}

/// Used only by the sandbox-side hook preflight. Descendants remain in a
/// kill-on-close Job but are not granted any identity beyond the caller's
/// existing Codex sandbox token.
pub(super) fn run_sandbox_process_tree(
    program: &Path,
    args: &[String],
    cwd: &Path,
    environment: &BTreeMap<String, String>,
    stdin: &[u8],
    timeout_ms: u32,
    output_limit: usize,
) -> Result<ProcessOutput> {
    run_process(
        program,
        args,
        cwd,
        environment,
        stdin,
        ProcessLimits {
            timeout_ms,
            output_limit,
            allow_descendants: true,
        },
    )
}

struct ProcessLimits {
    timeout_ms: u32,
    output_limit: usize,
    allow_descendants: bool,
}

fn run_process(
    program: &Path,
    args: &[String],
    cwd: &Path,
    environment: &BTreeMap<String, String>,
    stdin: &[u8],
    limits: ProcessLimits,
) -> Result<ProcessOutput> {
    let ProcessLimits {
        timeout_ms,
        output_limit,
        allow_descendants,
    } = limits;
    let maximum_timeout = if allow_descendants { 600_000 } else { 60_000 };
    if timeout_ms == 0
        || timeout_ms > maximum_timeout
        || output_limit == 0
        || output_limit > 64 * 1024 * 1024
        || stdin.len() > 64 * 1024 * 1024
    {
        bail!("invalid fixed process bounds");
    }
    let program_w = wide(program.as_os_str())?;
    let cwd_w = wide(cwd.as_os_str())?;
    let program_s = program
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("native executable path is not Unicode"))?;
    let mut command = quote(program_s);
    for arg in args {
        if arg.contains('\0') {
            bail!("NUL in fixed process argument");
        }
        command.push(' ');
        command.push_str(&quote(arg));
    }
    let mut command_w = wide(std::ffi::OsStr::new(&command))?;
    let mut env = Vec::<u16>::new();
    for (key, value) in environment {
        if key.is_empty() || key.contains(['=', '\0']) || value.contains('\0') {
            bail!("invalid fixed process environment");
        }
        env.extend(format!("{key}={value}").encode_utf16());
        env.push(0);
    }
    env.push(0);
    if environment.is_empty() {
        env.push(0);
    }
    let (out_read, out_write) = pipe()?;
    let (err_read, err_write) = pipe()?;
    let (in_read, in_write) = pipe()?;
    private(&out_read)?;
    private(&err_read)?;
    private(&in_write)?;
    let mut size = 0;
    unsafe {
        InitializeProcThreadAttributeList(std::ptr::null_mut(), 1, 0, &mut size);
    }
    if size == 0 {
        bail!("native handle-list size unavailable");
    }
    let mut attributes = vec![0u128; size.div_ceil(16)];
    let list = attributes.as_mut_ptr().cast();
    if unsafe { InitializeProcThreadAttributeList(list, 1, 0, &mut size) } == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    struct AttributeGuard(windows_sys::Win32::System::Threading::LPPROC_THREAD_ATTRIBUTE_LIST);
    impl Drop for AttributeGuard {
        fn drop(&mut self) {
            unsafe {
                DeleteProcThreadAttributeList(self.0);
            }
        }
    }
    let _attributes = AttributeGuard(list);
    let handles = [
        in_read.as_raw_handle(),
        out_write.as_raw_handle(),
        err_write.as_raw_handle(),
    ];
    if unsafe {
        UpdateProcThreadAttribute(
            list,
            0,
            PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
            handles.as_ptr().cast(),
            std::mem::size_of_val(&handles),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error().into());
    }
    let mut startup = STARTUPINFOEXW::default();
    startup.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
    startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    startup.StartupInfo.hStdInput = in_read.as_raw_handle() as _;
    startup.StartupInfo.hStdOutput = out_write.as_raw_handle() as _;
    startup.StartupInfo.hStdError = err_write.as_raw_handle() as _;
    startup.lpAttributeList = list;
    let job = owned(unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) })?;
    let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    if !allow_descendants {
        limits.BasicLimitInformation.LimitFlags |= JOB_OBJECT_LIMIT_ACTIVE_PROCESS;
        limits.BasicLimitInformation.ActiveProcessLimit = 1;
    }
    if unsafe {
        SetInformationJobObject(
            job.as_raw_handle() as _,
            JobObjectExtendedLimitInformation,
            (&raw const limits).cast(),
            std::mem::size_of_val(&limits) as u32,
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error().into());
    }
    let mut info = PROCESS_INFORMATION::default();
    if unsafe {
        CreateProcessW(
            program_w.as_ptr(),
            command_w.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            1,
            CREATE_SUSPENDED
                | CREATE_NO_WINDOW
                | CREATE_UNICODE_ENVIRONMENT
                | EXTENDED_STARTUPINFO_PRESENT,
            env.as_ptr().cast(),
            cwd_w.as_ptr(),
            &startup.StartupInfo,
            &mut info,
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error().into());
    }
    let guard = Guard {
        process: owned(info.hProcess)?,
        _thread: owned(info.hThread)?,
        _job: job,
    };
    if unsafe {
        AssignProcessToJobObject(
            guard._job.as_raw_handle() as _,
            guard.process.as_raw_handle() as _,
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error().into());
    }
    drop(in_read);
    drop(out_write);
    drop(err_write);
    if unsafe { ResumeThread(guard._thread.as_raw_handle() as _) } == u32::MAX {
        return Err(std::io::Error::last_os_error().into());
    }
    let out = std::thread::spawn(move || bounded(File::from(out_read), output_limit));
    let err = std::thread::spawn(move || bounded(File::from(err_read), output_limit));
    let input = stdin.to_vec();
    let input = std::thread::spawn(move || File::from(in_write).write_all(&input));
    let wait = unsafe { WaitForSingleObject(guard.process.as_raw_handle() as _, timeout_ms) };
    let mut code = 0;
    let code_ok = unsafe { GetExitCodeProcess(guard.process.as_raw_handle() as _, &mut code) };
    drop(guard); // Terminates a still-running child before joining pipe readers.
    let stdout = out
        .join()
        .map_err(|_| anyhow::anyhow!("stdout reader panicked"))?;
    let stderr = err
        .join()
        .map_err(|_| anyhow::anyhow!("stderr reader panicked"))?;
    let input_result = input
        .join()
        .map_err(|_| anyhow::anyhow!("stdin writer panicked"))?;
    if wait != WAIT_OBJECT_0 || code_ok == 0 {
        bail!("fixed process timed out or could not be waited");
    }
    // A nonzero child can close stdin early; preserve its real exit/output.
    if code == 0 {
        input_result?;
    }
    Ok(ProcessOutput {
        exit_code: code,
        stdout: stdout?,
        stderr: stderr?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(windows)]
    #[test]
    fn native_process_roundtrips_quoted_unicode_and_rejects_descendants() {
        let exe = Path::new("C:/Program Files/PowerShell/7/pwsh.exe");
        let cwd = tempfile::tempdir().unwrap();
        let mut env = BTreeMap::new();
        env.insert("SystemRoot".into(), "C:\\Windows".into());
        let args = vec![
            "-NoProfile".into(),
            "-Command".into(),
            "[Console]::Out.Write('中文引号 '); [Console]::Error.Write('stderr'); exit 0".into(),
        ];
        let output = run_single_process(exe, &args, cwd.path(), &env, &[], 10_000, 8192).unwrap();
        assert_eq!(output.exit_code, 0);
        assert_eq!(String::from_utf8(output.stdout).unwrap(), "中文引号 ");
        assert_eq!(output.stderr, b"stderr");
        let args=vec!["-NoProfile".into(),"-Command".into(),"$p=[Diagnostics.Process]::Start('C:\\Windows\\System32\\cmd.exe','/c echo forbidden>descendant.txt'); $p.WaitForExit(); exit 0".into()];
        let output = run_single_process(exe, &args, cwd.path(), &env, &[], 10_000, 8192).unwrap();
        // PowerShell normally continues after a non-terminating method error.
        assert!(!output.stderr.is_empty());
        assert!(!cwd.path().join("descendant.txt").exists());
    }
}
