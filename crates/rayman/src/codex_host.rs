//! Facts about the Codex host environment, independent of any hook behavior.
//!
//! Kept separate from `codex_hook` so the Stop guard does not become a shared
//! dependency of every read-only CLI surface that only wants to report what the
//! host can do.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::file_io::is_link_or_reparse;
#[cfg(windows)]
use crate::hash::sha256_bytes;

/// Match result for one independently meaningful execution-context axis.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ContextMatch {
    Match,
    Mismatch,
    Unknown,
    NotRequired,
    NotApplicable,
}

/// Overall classification. Principal identity, profile binding, and an ACL
/// capability are deliberately not collapsed into one fingerprint.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionContextStatus {
    Match,
    PrincipalMismatch,
    ProfileMismatch,
    Unknown,
    NotRequired,
    NotApplicable,
    PlatformMismatch,
}

/// Read-only identity facts. A principal fingerprint proves only the user SID;
/// it is not an ACL-capability fingerprint and must not be used to equate
/// ordinary, elevated, restricted, or AppContainer tokens.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ExecutionContextProbe {
    pub applicable: bool,
    pub status: ExecutionContextStatus,
    pub principal_match: ContextMatch,
    pub profile_match: ContextMatch,
    pub principal_account: Option<String>,
    pub principal_sid: Option<String>,
    pub principal_fingerprint: Option<String>,
    pub environment_account: Option<String>,
    pub environment_profile: Option<String>,
    pub token_profile: Option<String>,
    pub environment_profile_matches_token: Option<bool>,
    pub required_sid: Option<String>,
    pub required_account: Option<String>,
    pub required_profile: Option<String>,
    pub requirement_source: Option<String>,
    pub requirement_fingerprint: Option<String>,
    pub capability_key_hint: Option<String>,
    pub reason: String,
}

/// Probe the process token and compare it with the host/task identity contract.
///
/// `RAYMAN_REQUIRED_SID`, `RAYMAN_REQUIRED_PRINCIPAL`, and
/// `RAYMAN_REQUIRED_PROFILE` are optional process-scoped comparison inputs.
/// They are explicitly labelled untrusted: an environment value is diagnostic
/// context, never user authorization or a persisted goal requirement. No file
/// is opened or modified.
pub fn execution_context_probe() -> ExecutionContextProbe {
    if !cfg!(windows) {
        let required_sid = env_text("RAYMAN_REQUIRED_SID");
        let required_account = env_text("RAYMAN_REQUIRED_PRINCIPAL");
        let required_profile = env_path("RAYMAN_REQUIRED_PROFILE");
        let has_windows_requirement =
            required_sid.is_some() || required_account.is_some() || required_profile.is_some();
        return ExecutionContextProbe {
            applicable: false,
            status: if has_windows_requirement {
                ExecutionContextStatus::PlatformMismatch
            } else {
                ExecutionContextStatus::NotApplicable
            },
            principal_match: if has_windows_requirement {
                ContextMatch::Mismatch
            } else {
                ContextMatch::NotApplicable
            },
            profile_match: if required_profile.is_some() {
                ContextMatch::Mismatch
            } else {
                ContextMatch::NotApplicable
            },
            principal_account: None,
            principal_sid: None,
            principal_fingerprint: None,
            environment_account: environment_account(),
            environment_profile: env_path("USERPROFILE"),
            token_profile: None,
            environment_profile_matches_token: None,
            required_sid,
            required_account,
            required_profile,
            requirement_source: has_windows_requirement
                .then(|| "process_environment_untrusted".into()),
            requirement_fingerprint: None,
            capability_key_hint: None,
            reason: if has_windows_requirement {
                "a Windows execution requirement cannot be satisfied on this platform".into()
            } else {
                "Windows execution-context comparison is not applicable".into()
            },
        };
    }

    execution_context_probe_windows()
}

fn environment_account() -> Option<String> {
    let user = env_text("USERNAME")?;
    match env_text("USERDOMAIN") {
        Some(domain) => Some(format!("{domain}\\{user}")),
        None => Some(user),
    }
}

fn env_text(name: &str) -> Option<String> {
    env::var_os(name).and_then(|value| {
        let value = value.to_str()?;
        let value = value.trim();
        (!value.is_empty() && value.len() <= 4096 && !value.chars().any(char::is_control))
            .then(|| value.to_string())
    })
}

fn env_path(name: &str) -> Option<String> {
    env_text(name).map(|value| value.trim_end_matches(['\\', '/']).to_string())
}

fn normalized_account(value: &str) -> String {
    value.trim().replace('/', "\\").to_lowercase()
}

fn normalized_path(value: &str) -> String {
    value
        .trim()
        .trim_end_matches(['\\', '/'])
        .replace('/', "\\")
        .to_lowercase()
}

fn classify_execution_context(
    principal_sid: Option<&str>,
    principal_account: Option<&str>,
    required_sid: Option<&str>,
    required_account: Option<&str>,
    token_profile: Option<&str>,
    required_profile: Option<&str>,
) -> (ExecutionContextStatus, ContextMatch, ContextMatch, String) {
    let principal_match = if let Some(required_sid) = required_sid {
        match principal_sid {
            Some(actual) if actual.eq_ignore_ascii_case(required_sid) => ContextMatch::Match,
            Some(_) => ContextMatch::Mismatch,
            None => ContextMatch::Unknown,
        }
    } else if let Some(required_account) = required_account {
        match principal_account {
            Some(actual) if normalized_account(actual) == normalized_account(required_account) => {
                ContextMatch::Match
            }
            Some(_) => ContextMatch::Mismatch,
            None => ContextMatch::Unknown,
        }
    } else {
        ContextMatch::NotRequired
    };
    let profile_match = if let Some(required_profile) = required_profile {
        match token_profile {
            Some(actual) if normalized_path(actual) == normalized_path(required_profile) => {
                ContextMatch::Match
            }
            Some(_) => ContextMatch::Mismatch,
            None => ContextMatch::Unknown,
        }
    } else {
        ContextMatch::NotRequired
    };
    let status = if principal_match == ContextMatch::Mismatch {
        ExecutionContextStatus::PrincipalMismatch
    } else if profile_match == ContextMatch::Mismatch {
        ExecutionContextStatus::ProfileMismatch
    } else if principal_match == ContextMatch::Unknown || profile_match == ContextMatch::Unknown {
        ExecutionContextStatus::Unknown
    } else if principal_match == ContextMatch::NotRequired
        && profile_match == ContextMatch::NotRequired
    {
        ExecutionContextStatus::NotRequired
    } else {
        ExecutionContextStatus::Match
    };
    let reason = match status {
        ExecutionContextStatus::Match => {
            "observed process identity/profile matches every supplied comparison input".into()
        }
        ExecutionContextStatus::PrincipalMismatch => format!(
            "observed SID/account ({}/{}) does not match the supplied SID/account ({}/{})",
            principal_sid.unwrap_or("unknown"),
            principal_account.unwrap_or("unknown"),
            required_sid.unwrap_or("unspecified"),
            required_account.unwrap_or("unspecified")
        ),
        ExecutionContextStatus::ProfileMismatch => format!(
            "token-bound profile {} does not match supplied profile {}",
            token_profile.unwrap_or("unknown"),
            required_profile.unwrap_or("unspecified")
        ),
        ExecutionContextStatus::Unknown => {
            "one or more required execution-context axes could not be observed".into()
        }
        ExecutionContextStatus::NotRequired => {
            "no explicit execution-context comparison input was supplied".into()
        }
        ExecutionContextStatus::NotApplicable | ExecutionContextStatus::PlatformMismatch => {
            unreachable!("classified only by the platform wrapper")
        }
    };
    (status, principal_match, profile_match, reason)
}

#[cfg(windows)]
fn execution_context_probe_windows() -> ExecutionContextProbe {
    let environment_account = environment_account();
    let environment_profile = env_path("USERPROFILE");
    let required_sid = env_text("RAYMAN_REQUIRED_SID");
    let required_account = env_text("RAYMAN_REQUIRED_PRINCIPAL");
    let required_profile = env_path("RAYMAN_REQUIRED_PROFILE");
    let has_requirement =
        required_sid.is_some() || required_account.is_some() || required_profile.is_some();
    let token = windows_token_identity();
    let (principal_account, principal_sid, token_profile, token_errors) = match token {
        Ok(identity) => (
            identity.account,
            Some(identity.sid),
            identity.profile,
            [identity.account_error, identity.profile_error]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>(),
        ),
        Err(error) => (None, None, None, vec![format!("{error:#}")]),
    };
    let (status, principal_match, profile_match, mut reason) = classify_execution_context(
        principal_sid.as_deref(),
        principal_account.as_deref(),
        required_sid.as_deref(),
        required_account.as_deref(),
        token_profile.as_deref(),
        required_profile.as_deref(),
    );
    if !token_errors.is_empty() {
        reason = format!("{reason}: {}", token_errors.join("; "));
    }
    let principal_fingerprint = principal_sid
        .as_deref()
        .map(|sid| sha256_bytes(format!("windows-token:{sid}").as_bytes()));
    let requirement_fingerprint = has_requirement.then(|| {
        sha256_bytes(
            format!(
                "windows-required-v2|{}|{}|{}",
                required_sid
                    .as_deref()
                    .map(str::to_ascii_uppercase)
                    .unwrap_or_default(),
                required_account
                    .as_deref()
                    .map(normalized_account)
                    .unwrap_or_default(),
                required_profile
                    .as_deref()
                    .map(normalized_path)
                    .unwrap_or_default()
            )
            .as_bytes(),
        )
    });
    let environment_profile_matches_token =
        environment_profile.as_deref().and_then(|environment| {
            token_profile
                .as_deref()
                .map(|token| normalized_path(environment) == normalized_path(token))
        });
    let principal_required = required_sid.is_some() || required_account.is_some();
    let profile_required = required_profile.is_some();
    let capability_key_hint = match (principal_required, profile_required) {
        (true, true) => Some("execution_context/principal_and_profile".into()),
        (true, false) => Some("execution_context/principal".into()),
        (false, true) => Some("execution_context/profile".into()),
        (false, false) => None,
    };

    ExecutionContextProbe {
        applicable: true,
        status,
        principal_match,
        profile_match,
        principal_account,
        principal_sid,
        principal_fingerprint,
        environment_account,
        environment_profile,
        token_profile,
        environment_profile_matches_token,
        required_sid,
        required_account,
        required_profile,
        requirement_source: has_requirement.then(|| "process_environment_untrusted".into()),
        requirement_fingerprint,
        capability_key_hint,
        reason,
    }
}

#[cfg(not(windows))]
fn execution_context_probe_windows() -> ExecutionContextProbe {
    unreachable!("Windows execution-context probe called on another platform")
}

#[cfg(windows)]
struct WindowsTokenIdentity {
    account: Option<String>,
    sid: String,
    profile: Option<String>,
    account_error: Option<String>,
    profile_error: Option<String>,
}

#[cfg(windows)]
fn windows_token_identity() -> Result<WindowsTokenIdentity> {
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
    fn execution_context_classification_separates_principal_from_profile() {
        assert_eq!(
            classify_execution_context(
                Some("S-1-5-21-1"),
                Some("HOST\\qinrm"),
                None,
                Some("host\\QINRM"),
                Some(r"C:\Users\qinrm"),
                None,
            )
            .0,
            ExecutionContextStatus::Match
        );
        assert_eq!(
            classify_execution_context(
                Some("S-1-5-21-2"),
                Some("HOST\\CodexSandboxOnline"),
                None,
                Some("HOST\\qinrm"),
                Some(r"C:\Users\qinrm"),
                None,
            )
            .0,
            ExecutionContextStatus::PrincipalMismatch
        );
        assert_eq!(
            classify_execution_context(
                Some("S-1-5-21-1"),
                Some("HOST\\qinrm"),
                None,
                Some("HOST\\qinrm"),
                Some(r"C:\Users\sandbox"),
                Some(r"C:\Users\qinrm"),
            )
            .0,
            ExecutionContextStatus::ProfileMismatch
        );
        assert_eq!(
            classify_execution_context(None, None, None, Some("HOST\\qinrm"), None, None,).0,
            ExecutionContextStatus::Unknown
        );
        assert_eq!(
            classify_execution_context(
                Some("S-1-5-21-1"),
                Some("renamed\\account"),
                Some("s-1-5-21-1"),
                Some("old\\label"),
                None,
                None,
            )
            .0,
            ExecutionContextStatus::Match,
            "an explicit SID is authoritative over a display-name alias"
        );
        assert_eq!(
            classify_execution_context(
                Some("S-1-5-21-1"),
                Some("HOST\\qinrm"),
                None,
                None,
                Some(r"C:\Users\qinrm"),
                None,
            )
            .0,
            ExecutionContextStatus::NotRequired
        );
    }

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
