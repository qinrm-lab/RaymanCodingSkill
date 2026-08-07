//! Codex-specific host facts, independent of any hook behavior.
//!
//! Kept separate from `codex_hook` so the Stop guard does not become a shared
//! dependency of every read-only CLI surface that only wants to report what the
//! host can do.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::Serialize;

use crate::file_io::is_link_or_reparse;
#[cfg(windows)]
pub(crate) struct WindowsTokenIdentity {
    pub(crate) account: Option<String>,
    pub(crate) sid: String,
    pub(crate) profile: Option<String>,
    pub(crate) account_error: Option<String>,
    pub(crate) profile_error: Option<String>,
}

#[cfg(windows)]
pub(crate) fn windows_token_identity() -> Result<WindowsTokenIdentity> {
    use std::io;
    use std::mem::size_of;
    use std::ptr::{null, null_mut};

    use anyhow::{Context, bail};
    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_INSUFFICIENT_BUFFER, HANDLE};
    use windows_sys::Win32::Security::{
        GetSidIdentifierAuthority, GetSidSubAuthority, GetSidSubAuthorityCount,
        GetTokenInformation, IsValidSid, LookupAccountSidW, SID_NAME_USE, TOKEN_QUERY, TOKEN_USER,
        TokenUser,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    use windows_sys::Win32::UI::Shell::GetUserProfileDirectoryW;

    struct TokenHandle(HANDLE);
    impl Drop for TokenHandle {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe {
                    CloseHandle(self.0);
                }
            }
        }
    }

    let mut raw: HANDLE = null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut raw) } == 0 {
        return Err(io::Error::last_os_error()).context("cannot open the current process token");
    }
    let token = TokenHandle(raw);
    let mut required = 0u32;
    unsafe {
        GetTokenInformation(token.0, TokenUser, null_mut(), 0, &mut required);
    }
    if required < size_of::<TOKEN_USER>() as u32 {
        bail!("token user query returned an invalid buffer length: {required}");
    }
    let words = (required as usize).div_ceil(size_of::<usize>());
    let mut buffer = vec![0usize; words];
    if unsafe {
        GetTokenInformation(
            token.0,
            TokenUser,
            buffer.as_mut_ptr().cast(),
            required,
            &mut required,
        )
    } == 0
    {
        return Err(io::Error::last_os_error()).context("cannot read the current token user");
    }
    let token_user = unsafe { &*buffer.as_ptr().cast::<TOKEN_USER>() };
    let sid = token_user.User.Sid;
    if sid.is_null() || unsafe { IsValidSid(sid) } == 0 {
        bail!("the current token contains an invalid user SID");
    }

    // The SID is the identity fact. Account-name lookup is only a display
    // convenience and can fail when a domain controller is unavailable; do
    // not discard the valid SID or its stable fingerprint in that case.
    let authority = unsafe { &*GetSidIdentifierAuthority(sid) };
    let authority = authority
        .Value
        .iter()
        .fold(0u64, |value, byte| (value << 8) | u64::from(*byte));
    let count = unsafe { *GetSidSubAuthorityCount(sid) } as u32;
    let revision = unsafe { *(sid.cast::<u8>()) };
    let mut sid_text = format!("S-{revision}-{authority}");
    for index in 0..count {
        let part = unsafe { *GetSidSubAuthority(sid, index) };
        sid_text.push_str(&format!("-{part}"));
    }

    let decode = |value: &[u16]| {
        let end = value
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(value.len());
        String::from_utf16_lossy(&value[..end])
    };

    // Bind the profile to the same token as the SID. USERPROFILE is retained
    // separately as diagnostic environment only and can never satisfy a
    // required-profile comparison.
    let (profile, profile_error) = {
        let mut length = 260u32;
        let mut value = vec![0u16; length as usize];
        loop {
            let result =
                unsafe { GetUserProfileDirectoryW(token.0, value.as_mut_ptr(), &mut length) };
            if result != 0 {
                let profile = decode(&value);
                if profile.is_empty() {
                    break (
                        None,
                        Some("token profile query returned an empty directory".into()),
                    );
                }
                break (Some(profile), None);
            }
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(ERROR_INSUFFICIENT_BUFFER as i32)
                && length as usize > value.len()
                && length <= 32_768
            {
                value.resize(length as usize, 0);
                continue;
            }
            break (
                None,
                Some(format!("cannot resolve the token-bound profile: {error}")),
            );
        }
    };

    let mut name_len = 0u32;
    let mut domain_len = 0u32;
    let mut use_type: SID_NAME_USE = 0;
    unsafe {
        LookupAccountSidW(
            null(),
            sid,
            null_mut(),
            &mut name_len,
            null_mut(),
            &mut domain_len,
            &mut use_type,
        );
    }
    if name_len == 0 {
        return Ok(WindowsTokenIdentity {
            account: None,
            sid: sid_text,
            profile,
            account_error: Some(format!(
                "cannot size the token account name: {}",
                io::Error::last_os_error()
            )),
            profile_error,
        });
    }
    let mut name = vec![0u16; name_len as usize];
    let mut domain = vec![0u16; domain_len.max(1) as usize];
    if unsafe {
        LookupAccountSidW(
            null(),
            sid,
            name.as_mut_ptr(),
            &mut name_len,
            domain.as_mut_ptr(),
            &mut domain_len,
            &mut use_type,
        )
    } == 0
    {
        return Ok(WindowsTokenIdentity {
            account: None,
            sid: sid_text,
            profile,
            account_error: Some(format!(
                "cannot resolve the token account name: {}",
                io::Error::last_os_error()
            )),
            profile_error,
        });
    }
    let name = decode(&name);
    let domain = decode(&domain);
    let account = if domain.is_empty() {
        name
    } else {
        format!("{domain}\\{name}")
    };

    Ok(WindowsTokenIdentity {
        account: Some(account),
        sid: sid_text,
        profile,
        account_error: None,
        profile_error,
    })
}

/// Resolve the Codex home directory the same way Codex itself does.
pub fn default_codex_home() -> Result<PathBuf> {
    if let Some(path) = env::var_os("CODEX_HOME").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(path));
    }
    let home = env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .ok_or_else(|| anyhow::anyhow!("cannot resolve Codex home"))?;
    Ok(PathBuf::from(home).join(".codex"))
}

/// Whether this Codex host can run its built-in patch tool at all.
///
/// The Windows `unelevated` restricted-token sandbox cannot express the managed
/// permission profile (read-only `.git`/`.agents`/`.codex` carved out of the
/// writable workspace root), so it refuses before `apply_patch` reads the file.
/// Reporting that once per command is what stops an agent from rediagnosing a
/// host defect on every context window.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct HostPatchProbe {
    /// False when this platform or host cannot exhibit the defect at all.
    pub applicable: bool,
    /// The configured `[windows] sandbox` value, when it could be read.
    pub sandbox_mode: Option<String>,
    /// False only on a positively identified broken configuration.
    pub patch_tool_usable: bool,
    pub reason: Option<String>,
    pub fix: Option<String>,
}

impl HostPatchProbe {
    fn usable(sandbox_mode: Option<String>, applicable: bool) -> Self {
        Self {
            applicable,
            sandbox_mode,
            patch_tool_usable: true,
            reason: None,
            fix: None,
        }
    }
}

/// Probe the Codex host patch tool without touching any host file.
///
/// Deliberately biased to silence: only a positively parsed `unelevated` value
/// reports a defect. An unreadable, absent, or ambiguous config reports usable,
/// because a false "your host is broken" banner on every read-only command is
/// worse than missing one.
pub fn patch_probe(codex_home: Option<&Path>) -> HostPatchProbe {
    if !cfg!(windows) {
        return HostPatchProbe::usable(None, false);
    }
    let Ok(home) = codex_home
        .map(Path::to_path_buf)
        .map_or_else(default_codex_home, Ok)
    else {
        return HostPatchProbe::usable(None, false);
    };
    let config = home.join("config.toml");
    // A linked/reparse config is refused rather than followed, matching how the
    // hooks file is handled; an unreadable one is simply not a finding.
    match fs::symlink_metadata(&config) {
        Ok(metadata) if metadata.file_type().is_file() && !is_link_or_reparse(&metadata) => {}
        _ => return HostPatchProbe::usable(None, true),
    }
    let Ok(text) = fs::read_to_string(&config) else {
        return HostPatchProbe::usable(None, true);
    };
    let mode = windows_sandbox_mode(&text);
    if mode.as_deref() == Some("unelevated") {
        return HostPatchProbe {
            applicable: true,
            sandbox_mode: mode,
            patch_tool_usable: false,
            reason: Some(
                "Codex `[windows] sandbox = \"unelevated\"` cannot enforce split writable roots, split filesystem reads, or deny-read restrictions, so the built-in apply_patch refuses before reading the target".into(),
            ),
            fix: Some(
                "set `[windows] sandbox = \"elevated\"` in the Codex config and restart Codex; until then apply patches with `git apply` from a file".into(),
            ),
        };
    }
    HostPatchProbe::usable(mode, true)
}

/// Read `sandbox` from the `[windows]` table of a TOML document.
///
/// Hand-rolled because the tool ships no TOML dependency. It only recognizes the
/// unambiguous `key = "value"` form inside a plain `[windows]` header and returns
/// `None` for anything it does not fully understand.
fn windows_sandbox_mode(text: &str) -> Option<String> {
    let mut in_windows = false;
    for line in text.lines() {
        let line = match line.split_once('#') {
            Some((before, _)) => before,
            None => line,
        }
        .trim();
        if line.starts_with('[') {
            in_windows = line == "[windows]";
            continue;
        }
        if !in_windows {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim() != "sandbox" {
            continue;
        }
        let value = value.trim();
        return value
            .strip_prefix('"')
            .and_then(|value| value.strip_suffix('"'))
            .map(str::to_string);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_sandbox_mode_reads_only_the_windows_table() {
        assert_eq!(
            windows_sandbox_mode("[windows]\nsandbox = \"unelevated\"\n").as_deref(),
            Some("unelevated")
        );
        // A `sandbox` key belonging to another table must never be attributed
        // to `[windows]`, or an unrelated config reports a host defect.
        assert_eq!(
            windows_sandbox_mode("[other]\nsandbox = \"unelevated\"\n"),
            None
        );
        assert_eq!(
            windows_sandbox_mode(
                "[windows]\nsandbox = \"elevated\"\n[other]\nsandbox = \"unelevated\"\n"
            )
            .as_deref(),
            Some("elevated")
        );
        // Commented-out and unquoted values are not understood, so they must
        // read as unknown rather than as a positive finding.
        assert_eq!(
            windows_sandbox_mode("[windows]\n# sandbox = \"unelevated\"\n"),
            None
        );
        assert_eq!(
            windows_sandbox_mode("[windows]\nsandbox = unelevated\n"),
            None
        );
        assert_eq!(
            windows_sandbox_mode("[windows]\n  sandbox   =   \"unelevated\"  # trailing\n")
                .as_deref(),
            Some("unelevated")
        );
    }

    #[test]
    fn patch_probe_reports_a_defect_only_for_a_parsed_unelevated_sandbox() {
        let home = tempfile::tempdir().unwrap();
        // Absent config: nothing is known, so nothing is claimed.
        assert!(patch_probe(Some(home.path())).patch_tool_usable);

        fs::write(
            home.path().join("config.toml"),
            "[windows]\nsandbox = \"elevated\"\n",
        )
        .unwrap();
        assert!(patch_probe(Some(home.path())).patch_tool_usable);

        fs::write(
            home.path().join("config.toml"),
            "[windows]\nsandbox = \"unelevated\"\n",
        )
        .unwrap();
        let probe = patch_probe(Some(home.path()));
        assert_eq!(probe.patch_tool_usable, cfg!(not(windows)));
        if cfg!(windows) {
            assert_eq!(probe.sandbox_mode.as_deref(), Some("unelevated"));
            assert!(probe.fix.is_some());
        }
    }
}
