#[cfg(windows)]
use anyhow::Result;
use serde::Serialize;
#[derive(Debug, Serialize)]
pub struct ExecutionContextProbe {
    pub principal_sid: Option<String>,
    pub principal_account: Option<String>,
    pub token_profile: Option<String>,
    pub environment_account: Option<String>,
    pub errors: Vec<String>,
}
pub fn execution_context_probe() -> ExecutionContextProbe {
    #[cfg(windows)]
    {
        match windows_token_identity() {
            Ok(t) => ExecutionContextProbe {
                principal_sid: Some(t.sid),
                principal_account: t.account,
                token_profile: t.profile,
                environment_account: std::env::var("USERNAME").ok(),
                errors: [t.account_error, t.profile_error]
                    .into_iter()
                    .flatten()
                    .collect(),
            },
            Err(e) => ExecutionContextProbe {
                principal_sid: None,
                principal_account: None,
                token_profile: None,
                environment_account: std::env::var("USERNAME").ok(),
                errors: vec![format!("{e:#}")],
            },
        }
    }
    #[cfg(not(windows))]
    {
        ExecutionContextProbe {
            principal_sid: None,
            principal_account: None,
            token_profile: None,
            environment_account: std::env::var("USER").ok(),
            errors: vec!["Windows execution identity is not applicable".into()],
        }
    }
}
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
