//! Pure, fail-closed primitives for discovering RaymanCodingSkill releases.
//!
//! Its only concrete transport is a Windows WinHTTP provider locked to the
//! official Releases endpoint.  The module has no filesystem access, process
//! spawning, installer, or activation integration.  A caller may use the
//! injectable [`UpdateProvider`] to discover a newer release, but discovery is
//! only a prompt candidate: it never authorizes a download, install, or a
//! change to another workspace's activation contract.
//!
//! A future automatic installer must be a separate, authenticated delivery
//! protocol (signed manifest, pinned verification key, per-file hashes, and a
//! transaction with rollback).  A GitHub tag alone is not such authorization.

use std::cmp::Ordering;
use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

pub mod install;
pub mod state;
pub mod transport;
pub mod trust;

/// Fixed origin and object name for the only release-discovery request this
/// core supports.  The WinHTTP provider uses these separately rather than
/// parsing or accepting a caller-supplied URL.
pub const OFFICIAL_RELEASES_HOST: &str = "api.github.com";
pub const OFFICIAL_RELEASES_PATH: &str = "/repos/qinrm-lab/RaymanCodingSkill/releases";

/// The only release-discovery endpoint supported by this core.
///
/// The built-in Windows provider enforces HTTPS/TLS, request and response
/// limits, and a short timeout before it returns tags to this core.
pub const OFFICIAL_RELEASES_ENDPOINT: &str = concat!(
    "https://api.github.com",
    "/repos/qinrm-lab/RaymanCodingSkill/releases"
);

/// Fixed base for a user-facing release page.  It is only composed with a
/// [`ReleaseVersion`] that passed strict parsing, never an arbitrary tag.
pub const OFFICIAL_RELEASE_PAGE_ROOT: &str =
    "https://github.com/qinrm-lab/RaymanCodingSkill/releases/tag/";

/// Do not accept unbounded provider output even if a provider accidentally
/// forgets to impose an HTTP-body limit of its own.
pub const MAX_RELEASE_TAGS: usize = 128;

/// A strict `vMAJOR.MINOR.PATCH` tag is far smaller than this.  The limit keeps
/// a buggy or malicious provider from giving the comparison loop oversized
/// identifiers.
pub const MAX_RELEASE_TAG_BYTES: usize = 64;

/// The complete JSON response is bounded before it is deserialized.  This is
/// deliberately independent of the smaller accepted-tag bound above.
pub const MAX_RELEASE_RESPONSE_BYTES: usize = 1024 * 1024;

/// Every synchronous WinHTTP phase gets the same short upper bound.  A
/// background check must not hold a foreground coding task indefinitely.
pub const WINHTTP_TIMEOUT_MS: i32 = 5_000;

/// User opt-in checks default to once per day.
pub const DEFAULT_AUTO_CHECK_INTERVAL_HOURS: u16 = 24;
pub const MIN_AUTO_CHECK_INTERVAL_HOURS: u16 = 1;
pub const MAX_AUTO_CHECK_INTERVAL_HOURS: u16 = 24 * 7;
pub const UPDATE_STATE_SCHEMA_VERSION: u32 = 2;

/// A source token with no user-controlled endpoint constructor.
///
/// Passing this token into [`UpdateProvider`] makes the fixed-source boundary
/// visible at the integration seam while still allowing a fake provider in
/// tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OfficialUpdateSource {
    _private: (),
}

impl OfficialUpdateSource {
    pub const fn official() -> Self {
        Self { _private: () }
    }

    pub const fn endpoint(self) -> &'static str {
        OFFICIAL_RELEASES_ENDPOINT
    }

    /// Build a fixed GitHub release-page URL from a checked version.  This
    /// method only produces a prompt target; it does not open a browser.
    pub fn release_page(self, version: &ReleaseVersion) -> String {
        format!("{OFFICIAL_RELEASE_PAGE_ROOT}v{version}")
    }
}

/// A strict, stable SemVer release version without pre-release or build data.
///
/// `ReleaseVersion::parse` accepts `MAJOR.MINOR.PATCH`; release discovery uses
/// [`ReleaseVersion::parse_release_tag`] for the intentionally stricter
/// `vMAJOR.MINOR.PATCH` tag form.  Numeric identifiers are kept as canonical
/// strings so valid large SemVer values do not overflow a machine integer.
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub struct ReleaseVersion {
    major: String,
    minor: String,
    patch: String,
}

impl ReleaseVersion {
    pub fn parse(input: &str) -> Result<Self, VersionParseError> {
        let mut components = input.split('.');
        let Some(major) = components.next() else {
            return Err(VersionParseError::NotStrictSemVer);
        };
        let Some(minor) = components.next() else {
            return Err(VersionParseError::NotStrictSemVer);
        };
        let Some(patch) = components.next() else {
            return Err(VersionParseError::NotStrictSemVer);
        };
        if components.next().is_some()
            || !is_strict_numeric_identifier(major)
            || !is_strict_numeric_identifier(minor)
            || !is_strict_numeric_identifier(patch)
        {
            return Err(VersionParseError::NotStrictSemVer);
        }

        Ok(Self {
            major: major.into(),
            minor: minor.into(),
            patch: patch.into(),
        })
    }

    /// Parse only the exact stable tag grammar accepted from the release API.
    pub fn parse_release_tag(tag: &str) -> Result<Self, VersionParseError> {
        let Some(version) = tag.strip_prefix('v') else {
            return Err(VersionParseError::NotStrictReleaseTag);
        };
        Self::parse(version).map_err(|_| VersionParseError::NotStrictReleaseTag)
    }

    pub fn major(&self) -> &str {
        &self.major
    }

    pub fn minor(&self) -> &str {
        &self.minor
    }

    pub fn patch(&self) -> &str {
        &self.patch
    }

    pub fn release_tag(&self) -> String {
        format!("v{self}")
    }
}

impl FromStr for ReleaseVersion {
    type Err = VersionParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        Self::parse(input)
    }
}

impl fmt::Display for ReleaseVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

impl Ord for ReleaseVersion {
    fn cmp(&self, other: &Self) -> Ordering {
        compare_numeric_identifiers(&self.major, &other.major)
            .then_with(|| compare_numeric_identifiers(&self.minor, &other.minor))
            .then_with(|| compare_numeric_identifiers(&self.patch, &other.patch))
    }
}

impl PartialOrd for ReleaseVersion {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Serialize for ReleaseVersion {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for ReleaseVersion {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).map_err(de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionParseError {
    NotStrictSemVer,
    NotStrictReleaseTag,
}

impl fmt::Display for VersionParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotStrictSemVer => f.write_str("version must be stable MAJOR.MINOR.PATCH SemVer"),
            Self::NotStrictReleaseTag => {
                f.write_str("release tag must be stable vMAJOR.MINOR.PATCH SemVer")
            }
        }
    }
}

impl std::error::Error for VersionParseError {}

fn is_strict_numeric_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| byte.is_ascii_digit())
        && (value == "0" || !value.starts_with('0'))
}

fn compare_numeric_identifiers(left: &str, right: &str) -> Ordering {
    // Strict parsing removes leading zeroes, making length comparison a numeric
    // comparison even for identifiers too large for u64.
    left.len()
        .cmp(&right.len())
        .then_with(|| left.as_bytes().cmp(right.as_bytes()))
}

/// A network adapter receives only the fixed official source token.  It must
/// never accept a caller-provided endpoint, invoke git, or return unbounded
/// response data.
pub trait UpdateProvider {
    fn fetch_release_tags(
        &self,
        source: OfficialUpdateSource,
    ) -> Result<Vec<String>, UpdateProviderError>;
}

/// Provider failures are deliberately classified rather than exposed as a
/// generic arbitrary URL or install instruction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateProviderError {
    /// The running platform has no reviewed built-in discovery transport.
    UnsupportedPlatform,
    /// Transport, TLS, DNS, timeout, or other unavailable network condition.
    Unavailable,
    /// The fixed endpoint returned data that could not be safely parsed.
    MalformedResponse,
}

/// The built-in provider for the one fixed GitHub Releases endpoint.
///
/// On Windows it uses synchronous WinHTTP with HTTPS on port 443, no proxy,
/// redirects disabled, short timeouts, and a bounded response.  On other
/// targets it deliberately returns [`UpdateProviderError::UnsupportedPlatform`]
/// rather than misclassifying a structural capability boundary as a transient
/// network failure, silently substituting another stack, or invoking `git`.
#[derive(Debug, Clone, Copy, Default)]
pub struct OfficialReleaseProvider;

impl UpdateProvider for OfficialReleaseProvider {
    fn fetch_release_tags(
        &self,
        source: OfficialUpdateSource,
    ) -> Result<Vec<String>, UpdateProviderError> {
        // `OfficialUpdateSource` has no arbitrary endpoint constructor.  Keep
        // this check at the provider boundary so a future source extension
        // fails closed until it has its own independently reviewed transport.
        if source.endpoint() != OFFICIAL_RELEASES_ENDPOINT {
            return Err(UpdateProviderError::MalformedResponse);
        }
        platform::fetch_official_release_tags()
    }
}

/// Parse a complete, already bounded response from the fixed GitHub Releases
/// endpoint.  Every item must expose the GitHub `draft` and `prerelease`
/// booleans; only a non-draft, non-prerelease string `tag_name` crosses this
/// boundary.  Stable tag grammar is enforced by `check_for_update` so this
/// parser stays a faithful transport/schema layer.
#[cfg(any(windows, test))]
fn parse_official_release_tags_json(response: &[u8]) -> Result<Vec<String>, UpdateProviderError> {
    if response.len() > MAX_RELEASE_RESPONSE_BYTES {
        return Err(UpdateProviderError::MalformedResponse);
    }
    let document: serde_json::Value =
        serde_json::from_slice(response).map_err(|_| UpdateProviderError::MalformedResponse)?;
    let releases = document
        .as_array()
        .ok_or(UpdateProviderError::MalformedResponse)?;
    if releases.len() > MAX_RELEASE_TAGS {
        return Err(UpdateProviderError::MalformedResponse);
    }

    let mut tags = Vec::with_capacity(releases.len());
    for release in releases {
        let release = release
            .as_object()
            .ok_or(UpdateProviderError::MalformedResponse)?;
        let draft = release
            .get("draft")
            .and_then(serde_json::Value::as_bool)
            .ok_or(UpdateProviderError::MalformedResponse)?;
        let prerelease = release
            .get("prerelease")
            .and_then(serde_json::Value::as_bool)
            .ok_or(UpdateProviderError::MalformedResponse)?;
        if draft || prerelease {
            continue;
        }
        let tag = release
            .get("tag_name")
            .and_then(serde_json::Value::as_str)
            .ok_or(UpdateProviderError::MalformedResponse)?;
        if tag.len() > MAX_RELEASE_TAG_BYTES {
            return Err(UpdateProviderError::MalformedResponse);
        }
        tags.push(tag.to_owned());
    }
    Ok(tags)
}

#[cfg(windows)]
mod platform {
    use std::ffi::c_void;
    use std::ptr::{null, null_mut};

    use windows_sys::Win32::Networking::WinHttp::{
        INTERNET_DEFAULT_HTTPS_PORT, WINHTTP_ACCESS_TYPE_NO_PROXY, WINHTTP_DISABLE_REDIRECTS,
        WINHTTP_FLAG_SECURE, WINHTTP_OPTION_DISABLE_FEATURE, WINHTTP_OPTION_REDIRECT_POLICY,
        WINHTTP_OPTION_REDIRECT_POLICY_NEVER, WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_QUERY_STATUS_CODE,
        WinHttpCloseHandle, WinHttpConnect, WinHttpOpen, WinHttpOpenRequest,
        WinHttpQueryDataAvailable, WinHttpQueryHeaders, WinHttpReadData, WinHttpReceiveResponse,
        WinHttpSendRequest, WinHttpSetOption, WinHttpSetTimeouts,
    };

    use super::{
        MAX_RELEASE_RESPONSE_BYTES, OFFICIAL_RELEASES_HOST, OFFICIAL_RELEASES_PATH,
        UpdateProviderError, WINHTTP_TIMEOUT_MS, parse_official_release_tags_json,
    };

    struct WinHttpHandle(*mut c_void);

    impl WinHttpHandle {
        fn from_raw(handle: *mut c_void) -> Result<Self, UpdateProviderError> {
            if handle.is_null() {
                Err(UpdateProviderError::Unavailable)
            } else {
                Ok(Self(handle))
            }
        }

        fn as_raw(&self) -> *mut c_void {
            self.0
        }
    }

    impl Drop for WinHttpHandle {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // Every successful API constructor transfers one handle to
                // this RAII owner.  Close errors have no safe retry action.
                unsafe {
                    WinHttpCloseHandle(self.0);
                }
            }
        }
    }

    pub(super) fn fetch_official_release_tags() -> Result<Vec<String>, UpdateProviderError> {
        let agent = wide("RaymanCodingSkill update check");
        let session = WinHttpHandle::from_raw(unsafe {
            WinHttpOpen(
                agent.as_ptr(),
                WINHTTP_ACCESS_TYPE_NO_PROXY,
                null(),
                null(),
                0,
            )
        })?;
        if unsafe {
            WinHttpSetTimeouts(
                session.as_raw(),
                WINHTTP_TIMEOUT_MS,
                WINHTTP_TIMEOUT_MS,
                WINHTTP_TIMEOUT_MS,
                WINHTTP_TIMEOUT_MS,
            )
        } == 0
        {
            return Err(UpdateProviderError::Unavailable);
        }

        let host = wide(OFFICIAL_RELEASES_HOST);
        let connection = WinHttpHandle::from_raw(unsafe {
            // HTTPS is intentionally explicit.  No URL supplied by a caller
            // can select another host, scheme, or port.
            WinHttpConnect(
                session.as_raw(),
                host.as_ptr(),
                INTERNET_DEFAULT_HTTPS_PORT,
                0,
            )
        })?;
        let method = wide("GET");
        let object_name = wide(OFFICIAL_RELEASES_PATH);
        let request = WinHttpHandle::from_raw(unsafe {
            WinHttpOpenRequest(
                connection.as_raw(),
                method.as_ptr(),
                object_name.as_ptr(),
                null(),
                null(),
                null(),
                WINHTTP_FLAG_SECURE,
            )
        })?;

        set_u32_option(
            &request,
            WINHTTP_OPTION_DISABLE_FEATURE,
            WINHTTP_DISABLE_REDIRECTS,
        )?;
        // Defense in depth: if the feature bit is ever narrowed by a WinHTTP
        // implementation, the redirect policy is still explicitly "never".
        set_u32_option(
            &request,
            WINHTTP_OPTION_REDIRECT_POLICY,
            WINHTTP_OPTION_REDIRECT_POLICY_NEVER,
        )?;

        let headers = wide("Accept: application/vnd.github+json\r\n");
        let header_length = headers
            .len()
            .checked_sub(1)
            .and_then(|length| u32::try_from(length).ok())
            .ok_or(UpdateProviderError::MalformedResponse)?;
        if unsafe {
            WinHttpSendRequest(
                request.as_raw(),
                headers.as_ptr(),
                header_length,
                null(),
                0,
                0,
                0,
            )
        } == 0
        {
            return Err(UpdateProviderError::Unavailable);
        }
        if unsafe { WinHttpReceiveResponse(request.as_raw(), null_mut()) } == 0 {
            return Err(UpdateProviderError::Unavailable);
        }

        let mut status = 0u32;
        let mut status_length = u32::try_from(std::mem::size_of_val(&status))
            .expect("u32 status size fits WinHTTP length");
        if unsafe {
            WinHttpQueryHeaders(
                request.as_raw(),
                WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
                null(),
                (&mut status as *mut u32).cast(),
                &mut status_length,
                null_mut(),
            )
        } == 0
        {
            return Err(UpdateProviderError::Unavailable);
        }
        if status != 200 {
            return Err(UpdateProviderError::Unavailable);
        }

        let body = read_bounded_response(&request)?;
        parse_official_release_tags_json(&body)
    }

    fn set_u32_option(
        handle: &WinHttpHandle,
        option: u32,
        value: u32,
    ) -> Result<(), UpdateProviderError> {
        let length = u32::try_from(std::mem::size_of_val(&value))
            .expect("u32 option size fits WinHTTP length");
        if unsafe {
            WinHttpSetOption(
                handle.as_raw(),
                option,
                (&value as *const u32).cast(),
                length,
            )
        } == 0
        {
            return Err(UpdateProviderError::Unavailable);
        }
        Ok(())
    }

    fn read_bounded_response(request: &WinHttpHandle) -> Result<Vec<u8>, UpdateProviderError> {
        const CHUNK_SIZE: usize = 16 * 1024;
        let mut body = Vec::new();
        let mut chunk = [0u8; CHUNK_SIZE];
        loop {
            let mut available = 0u32;
            if unsafe { WinHttpQueryDataAvailable(request.as_raw(), &mut available) } == 0 {
                return Err(UpdateProviderError::Unavailable);
            }
            if available == 0 {
                return Ok(body);
            }
            let available =
                usize::try_from(available).map_err(|_| UpdateProviderError::MalformedResponse)?;
            let remaining = MAX_RELEASE_RESPONSE_BYTES
                .checked_sub(body.len())
                .ok_or(UpdateProviderError::MalformedResponse)?;
            // Do not consume the byte that crosses the advertised 1 MiB cap.
            // This is stricter than reading a fixed chunk then rejecting it.
            if available > remaining {
                return Err(UpdateProviderError::MalformedResponse);
            }
            let to_read = available.min(chunk.len());
            let mut read = 0u32;
            if unsafe {
                WinHttpReadData(
                    request.as_raw(),
                    chunk.as_mut_ptr().cast(),
                    u32::try_from(to_read).expect("bounded WinHTTP chunk fits u32"),
                    &mut read,
                )
            } == 0
            {
                return Err(UpdateProviderError::Unavailable);
            }
            if read == 0 {
                return Err(UpdateProviderError::MalformedResponse);
            }
            let read = usize::try_from(read).map_err(|_| UpdateProviderError::MalformedResponse)?;
            if read > to_read {
                return Err(UpdateProviderError::MalformedResponse);
            }
            body.extend_from_slice(&chunk[..read]);
        }
    }

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }
}

#[cfg(any(not(windows), test))]
mod unsupported_platform {
    use super::UpdateProviderError;

    pub(super) fn fetch_official_release_tags() -> Result<Vec<String>, UpdateProviderError> {
        Err(UpdateProviderError::UnsupportedPlatform)
    }
}

#[cfg(not(windows))]
use unsupported_platform as platform;

/// The outcome of release discovery.  An available version is a prompt
/// candidate, not a verified install bundle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum UpdateStatus {
    Current,
    UpdateAvailable { latest: ReleaseVersion },
    NoMatchingRelease,
    UnsupportedPlatform,
    Unavailable,
    MalformedResponse,
}

impl UpdateStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::UpdateAvailable { .. } => "update_available",
            Self::NoMatchingRelease => "no_matching_release",
            Self::UnsupportedPlatform => "unsupported_platform",
            Self::Unavailable => "unavailable",
            Self::MalformedResponse => "malformed_response",
        }
    }

    pub fn is_successful_discovery(&self) -> bool {
        matches!(
            self,
            Self::Current | Self::UpdateAvailable { .. } | Self::NoMatchingRelease
        )
    }
}

/// Immutable details of one check.  The rejected count makes ignored
/// non-stable tags observable without ever treating them as candidates.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateObservation {
    pub current: ReleaseVersion,
    pub status: UpdateStatus,
    pub rejected_tag_count: usize,
}

impl UpdateObservation {
    pub fn is_successful_discovery(&self) -> bool {
        self.status.is_successful_discovery()
    }

    /// Return a user-facing prompt target for a strictly newer candidate.
    /// Calling this method has no browser, download, or installation side
    /// effect.
    pub fn prompt(&self) -> Option<UpdatePrompt> {
        let UpdateStatus::UpdateAvailable { latest } = &self.status else {
            return None;
        };
        if latest <= &self.current {
            return None;
        }
        Some(UpdatePrompt {
            current: self.current.clone(),
            latest: latest.clone(),
            release_page: OfficialUpdateSource::official().release_page(latest),
        })
    }
}

/// A prompt is intentionally the strongest action this core can create.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UpdatePrompt {
    pub current: ReleaseVersion,
    pub latest: ReleaseVersion,
    pub release_page: String,
}

/// Check the fixed source through an injected provider.
///
/// Provider errors become an observable non-installing status so a background
/// poll cannot turn a transient network failure into a task blocker.  Invalid
/// tags are rejected individually; only a strict, newer `vMAJOR.MINOR.PATCH`
/// tag can produce [`UpdateStatus::UpdateAvailable`].
pub fn check_for_update<P: UpdateProvider>(
    provider: &P,
    current: ReleaseVersion,
) -> UpdateObservation {
    let tags = match provider.fetch_release_tags(OfficialUpdateSource::official()) {
        Ok(tags) => tags,
        Err(UpdateProviderError::UnsupportedPlatform) => {
            return UpdateObservation {
                current,
                status: UpdateStatus::UnsupportedPlatform,
                rejected_tag_count: 0,
            };
        }
        Err(UpdateProviderError::Unavailable) => {
            return UpdateObservation {
                current,
                status: UpdateStatus::Unavailable,
                rejected_tag_count: 0,
            };
        }
        Err(UpdateProviderError::MalformedResponse) => {
            return UpdateObservation {
                current,
                status: UpdateStatus::MalformedResponse,
                rejected_tag_count: 0,
            };
        }
    };

    if tags.len() > MAX_RELEASE_TAGS || tags.iter().any(|tag| tag.len() > MAX_RELEASE_TAG_BYTES) {
        return UpdateObservation {
            current,
            status: UpdateStatus::MalformedResponse,
            rejected_tag_count: 0,
        };
    }

    let mut rejected_tag_count = 0;
    let mut latest = None;
    for tag in tags {
        let Ok(candidate) = ReleaseVersion::parse_release_tag(&tag) else {
            rejected_tag_count += 1;
            continue;
        };
        if candidate > current {
            match &latest {
                Some(previous) if candidate <= *previous => {}
                _ => latest = Some(candidate),
            }
        }
    }

    let status = match latest {
        Some(latest) => UpdateStatus::UpdateAvailable { latest },
        // A well-formed release list can legitimately have no tag newer than
        // the installed CLI (including an empty list).  Malformed tags make
        // that distinction visible instead of presenting a false "current".
        None if rejected_tag_count == 0 => UpdateStatus::Current,
        None => UpdateStatus::NoMatchingRelease,
    };
    UpdateObservation {
        current,
        status,
        rejected_tag_count,
    }
}

/// Parse the version embedded by Cargo.  A package version that violates the
/// strict stable grammar is a build-time packaging fault, never a remote value.
pub fn compiled_release_version() -> ReleaseVersion {
    ReleaseVersion::parse(crate::CLI_VERSION)
        .expect("rayman package version must be stable MAJOR.MINOR.PATCH SemVer")
}

/// User-level preference and cache payload.  The caller owns where and how it
/// is persisted; this core deliberately never writes into a workspace or the
/// global skill directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateState {
    pub schema_version: u32,
    pub auto_check: bool,
    /// Independent, explicit consent for the verified worker path.  A legacy
    /// `auto_check=true` value must never become installation authority.
    #[serde(default)]
    pub auto_install: bool,
    pub interval_hours: u16,
    #[serde(default)]
    pub last_attempted_at: Option<DateTime<Utc>>,
    /// The last successfully parsed discovery observation.  Transient network
    /// and malformed-response outcomes never replace this value.
    #[serde(default)]
    pub last_successful_observation: Option<UpdateObservation>,
}

impl Default for UpdateState {
    fn default() -> Self {
        Self {
            schema_version: UPDATE_STATE_SCHEMA_VERSION,
            // The user requested software-style update notification.  Only
            // non-read-only skill calls invoke `update poll`; installation is
            // still separately opt-in and false by default.
            auto_check: true,
            auto_install: false,
            interval_hours: DEFAULT_AUTO_CHECK_INTERVAL_HOURS,
            last_attempted_at: None,
            last_successful_observation: None,
        }
    }
}

impl UpdateState {
    /// Migrate the only legacy shape.  The new installation-consent bit is
    /// always false for v1, even when legacy auto-check was enabled.
    pub fn migrate(&mut self) -> Result<(), UpdateStateError> {
        match self.schema_version {
            UPDATE_STATE_SCHEMA_VERSION => Ok(()),
            1 => {
                self.schema_version = UPDATE_STATE_SCHEMA_VERSION;
                self.auto_install = false;
                Ok(())
            }
            found => Err(UpdateStateError::UnsupportedSchema { found }),
        }
    }

    pub fn validate(&self) -> Result<(), UpdateStateError> {
        if self.schema_version != UPDATE_STATE_SCHEMA_VERSION {
            return Err(UpdateStateError::UnsupportedSchema {
                found: self.schema_version,
            });
        }
        validate_interval(self.interval_hours)?;
        if let Some(observation) = &self.last_successful_observation
            && (!observation.is_successful_discovery()
                || matches!(
                    &observation.status,
                    UpdateStatus::UpdateAvailable { latest } if latest <= &observation.current
                ))
        {
            return Err(UpdateStateError::InvalidCachedObservation);
        }
        Ok(())
    }

    /// Explicitly opt in to periodic discovery.  This changes only in-memory
    /// settings; the caller must separately persist it after its own confirmed
    /// user action.
    pub fn enable_auto_check(&mut self, interval_hours: u16) -> Result<(), UpdateStateError> {
        self.validate()?;
        validate_interval(interval_hours)?;
        self.auto_check = true;
        self.interval_hours = interval_hours;
        Ok(())
    }

    /// Stop periodic discovery.  It does not install, uninstall, or rewrite
    /// any workspace activation state.
    pub fn disable_auto_check(&mut self) {
        self.auto_check = false;
    }

    pub fn enable_auto_install(&mut self) {
        self.auto_install = true;
        self.auto_check = true;
    }

    pub fn disable_auto_install(&mut self) {
        self.auto_install = false;
    }

    /// Cached discovery is only a notification and only for the exact running
    /// version that produced it.  It can never be promoted to a trusted
    /// manifest or a downgrade prompt.
    pub fn current_cached_observation(
        &self,
        current: &ReleaseVersion,
    ) -> Option<&UpdateObservation> {
        let observation = self.last_successful_observation.as_ref()?;
        (observation.current == *current && observation.prompt().is_some()).then_some(observation)
    }

    /// Decide whether an opted-in poll is due without making a network call.
    /// A wall clock that moves backwards is treated as due so a forged future
    /// cache timestamp cannot freeze notification indefinitely.
    pub fn is_auto_check_due(&self, now: DateTime<Utc>) -> Result<bool, UpdateStateError> {
        self.validate()?;
        if !self.auto_check {
            return Ok(false);
        }
        let Some(last_attempted_at) = self.last_attempted_at else {
            return Ok(true);
        };
        let interval = Duration::hours(i64::from(self.interval_hours));
        let elapsed = now.signed_duration_since(last_attempted_at);
        if elapsed < Duration::zero() {
            return Ok(true);
        }
        Ok(elapsed >= interval)
    }

    /// Record an opted-in discovery attempt.  An unavailable or malformed
    /// result advances the attempt time to prevent a foreground task from
    /// hammering the fixed endpoint, while preserving the last good result.
    pub fn record_auto_check(
        &mut self,
        attempted_at: DateTime<Utc>,
        observation: UpdateObservation,
    ) -> Result<(), UpdateStateError> {
        self.validate()?;
        if !self.auto_check {
            return Err(UpdateStateError::AutoCheckDisabled);
        }
        self.last_attempted_at = Some(attempted_at);
        if observation.is_successful_discovery() {
            self.last_successful_observation = Some(observation);
        }
        Ok(())
    }

    /// Perform a check only when the user previously opted in and its interval
    /// is due.  The caller may persist `self` after this returns, but this core
    /// itself has no disk or install side effects.
    pub fn poll_if_due<P: UpdateProvider>(
        &mut self,
        now: DateTime<Utc>,
        provider: &P,
        current: ReleaseVersion,
    ) -> Result<Option<UpdateObservation>, UpdateStateError> {
        if !self.is_auto_check_due(now)? {
            return Ok(None);
        }
        let observation = check_for_update(provider, current);
        self.record_auto_check(now, observation.clone())?;
        Ok(Some(observation))
    }
}

fn validate_interval(interval_hours: u16) -> Result<(), UpdateStateError> {
    if !(MIN_AUTO_CHECK_INTERVAL_HOURS..=MAX_AUTO_CHECK_INTERVAL_HOURS).contains(&interval_hours) {
        return Err(UpdateStateError::InvalidInterval { interval_hours });
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateStateError {
    UnsupportedSchema { found: u32 },
    InvalidInterval { interval_hours: u16 },
    AutoCheckDisabled,
    InvalidCachedObservation,
}

impl fmt::Display for UpdateStateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedSchema { found } => write!(
                f,
                "unsupported update state schema {found}; expected {UPDATE_STATE_SCHEMA_VERSION}"
            ),
            Self::InvalidInterval { interval_hours } => write!(
                f,
                "auto-check interval {interval_hours} is outside {MIN_AUTO_CHECK_INTERVAL_HOURS}..={MAX_AUTO_CHECK_INTERVAL_HOURS} hours"
            ),
            Self::AutoCheckDisabled => f.write_str("automatic update checks are disabled"),
            Self::InvalidCachedObservation => {
                f.write_str("cached update observation is stale, unsuccessful, or a downgrade")
            }
        }
    }
}

impl std::error::Error for UpdateStateError {}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use chrono::TimeZone;

    use super::*;

    #[derive(Clone)]
    struct FakeProvider {
        response: Result<Vec<String>, UpdateProviderError>,
        calls: Cell<u32>,
        endpoint: Cell<Option<&'static str>>,
    }

    impl FakeProvider {
        fn tags(tags: &[&str]) -> Self {
            Self {
                response: Ok(tags.iter().map(|tag| (*tag).into()).collect()),
                calls: Cell::new(0),
                endpoint: Cell::new(None),
            }
        }

        fn failure(error: UpdateProviderError) -> Self {
            Self {
                response: Err(error),
                calls: Cell::new(0),
                endpoint: Cell::new(None),
            }
        }
    }

    impl UpdateProvider for FakeProvider {
        fn fetch_release_tags(
            &self,
            source: OfficialUpdateSource,
        ) -> Result<Vec<String>, UpdateProviderError> {
            self.calls.set(self.calls.get() + 1);
            self.endpoint.set(Some(source.endpoint()));
            self.response.clone()
        }
    }

    fn version(value: &str) -> ReleaseVersion {
        ReleaseVersion::parse(value).unwrap()
    }

    fn time(offset_hours: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 8, 20, 0, 0, 0).single().unwrap()
            + Duration::hours(i64::from(offset_hours))
    }

    #[test]
    fn strict_semver_rejects_noncanonical_and_prerelease_values() {
        for invalid in [
            "",
            "1",
            "1.2",
            "1.2.3.4",
            "01.2.3",
            "1.02.3",
            "1.2.03",
            "1.2.3-beta",
            "1.2.3+build",
            "v1.2.3",
            "1.2.3 ",
            "１.２.３",
        ] {
            assert!(ReleaseVersion::parse(invalid).is_err(), "{invalid}");
        }
        assert_eq!(ReleaseVersion::parse("0.0.0").unwrap().to_string(), "0.0.0");
        assert_eq!(
            ReleaseVersion::parse_release_tag("v10.20.30")
                .unwrap()
                .to_string(),
            "10.20.30"
        );
    }

    #[test]
    fn semantic_comparison_handles_large_identifiers_without_overflow() {
        assert!(version("18446744073709551616.0.0") > version("999.999.999"));
        assert!(version("2.10.0") > version("2.9.999"));
        assert_eq!(version("2.10.0"), version("2.10.0"));
    }

    #[test]
    fn check_only_uses_fixed_endpoint_and_strictly_newer_stable_tags() {
        let provider = FakeProvider::tags(&[
            "v2.10.0",
            "v2.9.9",
            "v2.11.0-rc.1",
            "main",
            "v2.11.0",
            "v2.12.0",
        ]);

        let observation = check_for_update(&provider, version("2.10.0"));

        assert_eq!(provider.calls.get(), 1);
        assert_eq!(provider.endpoint.get(), Some(OFFICIAL_RELEASES_ENDPOINT));
        assert_eq!(observation.rejected_tag_count, 2);
        assert_eq!(
            observation.status,
            UpdateStatus::UpdateAvailable {
                latest: version("2.12.0")
            }
        );
        assert_eq!(
            observation.prompt().unwrap().release_page,
            "https://github.com/qinrm-lab/RaymanCodingSkill/releases/tag/v2.12.0"
        );
    }

    #[test]
    fn github_release_parser_only_yields_public_non_prerelease_tags() {
        let response = br#"[
          {"draft": true, "prerelease": false, "tag_name": "v99.0.0"},
          {"draft": false, "prerelease": true, "tag_name": "v98.0.0"},
          {"draft": false, "prerelease": false, "tag_name": "v2.11.0"}
        ]"#;

        assert_eq!(
            parse_official_release_tags_json(response),
            Ok(vec!["v2.11.0".into()])
        );
    }

    #[test]
    fn github_release_parser_rejects_non_array_or_untrusted_stable_items() {
        for malformed in [
            br#"{}"#.as_slice(),
            br#"[{"draft": false, "prerelease": false}]"#.as_slice(),
            br#"[{"draft": false, "prerelease": false, "tag_name": 42}]"#.as_slice(),
            br#"[{"draft": "false", "prerelease": false, "tag_name": "v2.11.0"}]"#.as_slice(),
            br#"[{"draft": false, "prerelease": false, "tag_name": "v2.11.0"}"#.as_slice(),
        ] {
            assert_eq!(
                parse_official_release_tags_json(malformed),
                Err(UpdateProviderError::MalformedResponse),
                "{}",
                String::from_utf8_lossy(malformed)
            );
        }
    }

    #[test]
    fn github_release_parser_rejects_oversized_response_and_tag() {
        let oversized_response = vec![b' '; MAX_RELEASE_RESPONSE_BYTES + 1];
        assert_eq!(
            parse_official_release_tags_json(&oversized_response),
            Err(UpdateProviderError::MalformedResponse)
        );

        let oversized_tag = "v".to_owned() + &"1".repeat(MAX_RELEASE_TAG_BYTES);
        let oversized_tag_response = serde_json::json!([{
            "draft": false,
            "prerelease": false,
            "tag_name": oversized_tag,
        }])
        .to_string();
        assert_eq!(
            parse_official_release_tags_json(oversized_tag_response.as_bytes()),
            Err(UpdateProviderError::MalformedResponse)
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn built_in_provider_reports_the_structural_platform_boundary_without_winhttp() {
        let provider = OfficialReleaseProvider;
        assert_eq!(
            provider.fetch_release_tags(OfficialUpdateSource::official()),
            Err(UpdateProviderError::UnsupportedPlatform)
        );
        let observation = check_for_update(&provider, version("2.10.0"));
        assert_eq!(observation.status, UpdateStatus::UnsupportedPlatform);
        assert!(!observation.is_successful_discovery());
        assert!(observation.prompt().is_none());
    }

    #[test]
    fn unsupported_platform_adapter_is_a_zero_input_structural_refusal() {
        assert_eq!(
            unsupported_platform::fetch_official_release_tags(),
            Err(UpdateProviderError::UnsupportedPlatform)
        );
    }

    #[test]
    fn stale_or_equal_releases_never_create_a_downgrade_prompt() {
        let provider = FakeProvider::tags(&["v2.10.0", "v2.9.99"]);

        let observation = check_for_update(&provider, version("2.10.0"));

        assert_eq!(observation.status, UpdateStatus::Current);
        assert!(observation.prompt().is_none());
    }

    #[test]
    fn malformed_tags_are_rejected_and_never_promoted_to_candidates() {
        let provider = FakeProvider::tags(&["preview", "v2.11.0-beta", "2.12.0"]);

        let observation = check_for_update(&provider, version("2.10.0"));

        assert_eq!(observation.status, UpdateStatus::NoMatchingRelease);
        assert_eq!(observation.rejected_tag_count, 3);
        assert!(observation.prompt().is_none());
    }

    #[test]
    fn provider_and_bound_failures_are_observable_but_noninstalling() {
        let unsupported = check_for_update(
            &FakeProvider::failure(UpdateProviderError::UnsupportedPlatform),
            version("2.10.0"),
        );
        assert_eq!(unsupported.status, UpdateStatus::UnsupportedPlatform);
        assert_eq!(unsupported.status.as_str(), "unsupported_platform");
        assert_eq!(
            serde_json::to_value(&unsupported).unwrap()["status"]["status"],
            "unsupported_platform"
        );
        assert!(!unsupported.is_successful_discovery());
        assert!(unsupported.prompt().is_none());

        let unavailable = check_for_update(
            &FakeProvider::failure(UpdateProviderError::Unavailable),
            version("2.10.0"),
        );
        assert_eq!(unavailable.status, UpdateStatus::Unavailable);
        assert!(!unavailable.is_successful_discovery());
        assert!(unavailable.prompt().is_none());

        let too_many = FakeProvider::tags(
            &(0..=MAX_RELEASE_TAGS)
                .map(|_| "v2.10.0")
                .collect::<Vec<_>>(),
        );
        let malformed = check_for_update(&too_many, version("2.10.0"));
        assert_eq!(malformed.status, UpdateStatus::MalformedResponse);
        assert!(malformed.prompt().is_none());

        let malformed_provider = check_for_update(
            &FakeProvider::failure(UpdateProviderError::MalformedResponse),
            version("2.10.0"),
        );
        assert_eq!(malformed_provider.status, UpdateStatus::MalformedResponse);
        assert!(malformed_provider.prompt().is_none());
    }

    #[test]
    fn default_state_checks_for_notification_but_never_authorizes_installation() {
        let mut state = UpdateState::default();
        let provider = FakeProvider::tags(&["v2.11.0"]);

        assert!(state.auto_check);
        assert!(!state.auto_install);
        assert!(state.is_auto_check_due(time(0)).unwrap());
        let observation = state
            .poll_if_due(time(0), &provider, version("2.10.0"))
            .unwrap()
            .unwrap();
        assert_eq!(provider.calls.get(), 1);
        assert!(observation.prompt().is_some());
        assert_eq!(state.last_attempted_at, Some(time(0)));
        assert!(state.last_successful_observation.is_some());
        assert!(!state.auto_install);
    }

    #[test]
    fn unsupported_poll_records_only_the_attempt_and_never_caches_a_notification() {
        let mut state = UpdateState::default();
        let provider = FakeProvider::failure(UpdateProviderError::UnsupportedPlatform);

        let observation = state
            .poll_if_due(time(0), &provider, version("2.10.0"))
            .unwrap()
            .unwrap();

        assert_eq!(provider.calls.get(), 1);
        assert_eq!(observation.status, UpdateStatus::UnsupportedPlatform);
        assert!(!observation.is_successful_discovery());
        assert!(observation.prompt().is_none());
        assert_eq!(state.last_attempted_at, Some(time(0)));
        assert!(state.last_successful_observation.is_none());
        assert!(!state.auto_install);
    }

    #[test]
    fn opted_in_poll_respects_interval_and_preserves_last_good_result_on_failure() {
        let mut state = UpdateState::default();
        state.enable_auto_check(24).unwrap();
        let successful = FakeProvider::tags(&["v2.11.0"]);

        let observation = state
            .poll_if_due(time(0), &successful, version("2.10.0"))
            .unwrap()
            .unwrap();
        assert!(observation.is_successful_discovery());
        assert_eq!(successful.calls.get(), 1);
        assert_eq!(
            state.last_successful_observation.as_ref().unwrap(),
            &observation
        );
        assert!(
            state
                .poll_if_due(time(23), &successful, version("2.10.0"))
                .unwrap()
                .is_none()
        );
        assert_eq!(successful.calls.get(), 1);

        let unavailable = FakeProvider::failure(UpdateProviderError::Unavailable);
        let failed = state
            .poll_if_due(time(24), &unavailable, version("2.10.0"))
            .unwrap()
            .unwrap();
        assert_eq!(failed.status, UpdateStatus::Unavailable);
        assert_eq!(unavailable.calls.get(), 1);
        assert_eq!(
            state.last_successful_observation.as_ref().unwrap(),
            &observation
        );
        assert_eq!(state.last_attempted_at, Some(time(24)));
    }

    #[test]
    fn state_validation_rejects_bad_schema_interval_and_forged_versions() {
        let mut state = UpdateState::default();
        state.schema_version += 1;
        assert_eq!(
            state.validate(),
            Err(UpdateStateError::UnsupportedSchema {
                found: UPDATE_STATE_SCHEMA_VERSION + 1
            })
        );
        assert_eq!(
            state.enable_auto_check(24),
            Err(UpdateStateError::UnsupportedSchema {
                found: UPDATE_STATE_SCHEMA_VERSION + 1
            })
        );

        let mut state = UpdateState::default();
        assert_eq!(
            state.enable_auto_check(0),
            Err(UpdateStateError::InvalidInterval { interval_hours: 0 })
        );
        assert!(state.auto_check);

        let forged = r#"{
            "schema_version": 1,
            "auto_check": false,
            "interval_hours": 24,
            "last_successful_observation": {
                "current": "2.10.0",
                "status": {"status": "update_available", "latest": "2.11.0-beta"},
                "rejected_tag_count": 0
            }
        }"#;
        assert!(serde_json::from_str::<UpdateState>(forged).is_err());
    }

    #[test]
    fn legacy_auto_check_never_migrates_into_install_consent() {
        let mut state: UpdateState = serde_json::from_str(
            r#"{
                "schema_version": 1,
                "auto_check": true,
                "interval_hours": 24,
                "last_attempted_at": null,
                "last_successful_observation": null
            }"#,
        )
        .unwrap();
        assert!(!state.auto_install);
        state.migrate().unwrap();
        assert_eq!(state.schema_version, UPDATE_STATE_SCHEMA_VERSION);
        assert!(state.auto_check);
        assert!(!state.auto_install);
        state.validate().unwrap();
    }

    #[test]
    fn cached_prompt_rejects_stale_running_version_and_downgrade_shapes() {
        let mut state = UpdateState {
            last_successful_observation: Some(UpdateObservation {
                current: version("2.10.0"),
                status: UpdateStatus::UpdateAvailable {
                    latest: version("2.11.0"),
                },
                rejected_tag_count: 0,
            }),
            ..UpdateState::default()
        };
        state.validate().unwrap();
        assert!(
            state
                .current_cached_observation(&version("2.10.0"))
                .is_some()
        );
        assert!(
            state
                .current_cached_observation(&version("2.12.0"))
                .is_none()
        );

        state.last_successful_observation = Some(UpdateObservation {
            current: version("2.12.0"),
            status: UpdateStatus::UpdateAvailable {
                latest: version("2.11.0"),
            },
            rejected_tag_count: 0,
        });
        assert_eq!(
            state.validate(),
            Err(UpdateStateError::InvalidCachedObservation)
        );
        assert!(
            state
                .last_successful_observation
                .as_ref()
                .unwrap()
                .prompt()
                .is_none()
        );
    }

    #[test]
    fn compiled_version_is_stable_semver() {
        assert_eq!(compiled_release_version().to_string(), crate::CLI_VERSION);
    }
}
