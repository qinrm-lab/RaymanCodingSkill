//! Client-neutral execution-context classification and diagnostics.
//!
//! Host adapters provide platform facts; this module owns normalization,
//! requirement comparison, and the public read-only probe contract.

use std::env;

use serde::{Deserialize, Serialize};

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
    let token = crate::codex_host::windows_token_identity();
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
}
