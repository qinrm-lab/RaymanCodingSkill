//! Fixed GitHub Release asset transport.
//!
//! The signed manifest never supplies a URL.  A strict candidate version and
//! one compile-time asset name are the only inputs to the URL constructor.

use std::fmt;

use super::ReleaseVersion;
use super::trust::{AssetRole, MAX_MANIFEST_BYTES};

pub const RELEASE_ASSET_HOST: &str = "github.com";
pub const RELEASE_ASSET_PREFIX: &str = "/qinrm-lab/RaymanCodingSkill/releases/download/";
pub const MANIFEST_ASSET_NAME: &str = "rayman-update-manifest-v1.json";
pub const SIGNATURE_ASSET_NAME: &str = "rayman-update-manifest-v1.sig";
pub const MAX_SIGNATURE_BYTES: usize = 64;
pub const ASSET_TIMEOUT_MS: i32 = 15_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OfficialAssetSource {
    version: ReleaseVersion,
    asset_name: &'static str,
}

impl OfficialAssetSource {
    pub fn new(
        version: ReleaseVersion,
        asset_name: &'static str,
    ) -> Result<Self, AssetTransportError> {
        if !is_allowed_asset_name(asset_name) {
            return Err(AssetTransportError::InvalidSource);
        }
        Ok(Self {
            version,
            asset_name,
        })
    }

    pub fn version(&self) -> &ReleaseVersion {
        &self.version
    }

    pub fn asset_name(&self) -> &'static str {
        self.asset_name
    }

    pub fn object_path(&self) -> String {
        format!(
            "{RELEASE_ASSET_PREFIX}{}/{}",
            self.version.release_tag(),
            self.asset_name
        )
    }
}

pub trait AssetTransport {
    fn fetch(
        &self,
        source: &OfficialAssetSource,
        maximum_bytes: usize,
    ) -> Result<Vec<u8>, AssetTransportError>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct OfficialAssetTransport;

impl AssetTransport for OfficialAssetTransport {
    fn fetch(
        &self,
        source: &OfficialAssetSource,
        maximum_bytes: usize,
    ) -> Result<Vec<u8>, AssetTransportError> {
        if maximum_bytes == 0 || maximum_bytes > 64 * 1024 * 1024 {
            return Err(AssetTransportError::InvalidBound);
        }
        platform::fetch(source, maximum_bytes)
    }
}

pub fn manifest_source(version: ReleaseVersion) -> OfficialAssetSource {
    OfficialAssetSource::new(version, MANIFEST_ASSET_NAME)
        .expect("manifest asset name is compile-time fixed")
}

pub fn signature_source(version: ReleaseVersion) -> OfficialAssetSource {
    OfficialAssetSource::new(version, SIGNATURE_ASSET_NAME)
        .expect("signature asset name is compile-time fixed")
}

pub fn manifest_maximum_bytes() -> usize {
    MAX_MANIFEST_BYTES
}

fn is_allowed_asset_name(name: &str) -> bool {
    name == MANIFEST_ASSET_NAME
        || name == SIGNATURE_ASSET_NAME
        || AssetRole::ALL
            .into_iter()
            .any(|role| name == role.expected_name())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssetTransportError {
    InvalidSource,
    InvalidBound,
    Unavailable,
    RedirectRejected,
    ResponseTooLarge,
    MalformedResponse,
}

impl fmt::Display for AssetTransportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidSource => "release asset source is not fixed by the update protocol",
            Self::InvalidBound => "release asset size bound is invalid",
            Self::Unavailable => "release asset transport is unavailable",
            Self::RedirectRejected => "release asset redirect target is not trusted",
            Self::ResponseTooLarge => "release asset exceeds its signed size bound",
            Self::MalformedResponse => "release asset response is malformed",
        })
    }
}

impl std::error::Error for AssetTransportError {}

#[cfg(windows)]
mod platform {
    use std::ffi::c_void;
    use std::ptr::{null, null_mut};

    use windows_sys::Win32::Networking::WinHttp::{
        INTERNET_DEFAULT_HTTPS_PORT, WINHTTP_ACCESS_TYPE_NO_PROXY, WINHTTP_FLAG_SECURE,
        WINHTTP_OPTION_MAX_HTTP_AUTOMATIC_REDIRECTS, WINHTTP_OPTION_REDIRECT_POLICY,
        WINHTTP_OPTION_REDIRECT_POLICY_DISALLOW_HTTPS_TO_HTTP, WINHTTP_OPTION_URL,
        WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_QUERY_STATUS_CODE, WinHttpCloseHandle, WinHttpConnect,
        WinHttpOpen, WinHttpOpenRequest, WinHttpQueryDataAvailable, WinHttpQueryHeaders,
        WinHttpQueryOption, WinHttpReadData, WinHttpReceiveResponse, WinHttpSendRequest,
        WinHttpSetOption, WinHttpSetTimeouts,
    };

    use super::{ASSET_TIMEOUT_MS, AssetTransportError, OfficialAssetSource, RELEASE_ASSET_HOST};

    struct WinHttpHandle(*mut c_void);

    impl WinHttpHandle {
        fn from_raw(handle: *mut c_void) -> Result<Self, AssetTransportError> {
            (!handle.is_null())
                .then_some(Self(handle))
                .ok_or(AssetTransportError::Unavailable)
        }

        fn as_raw(&self) -> *mut c_void {
            self.0
        }
    }

    impl Drop for WinHttpHandle {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe {
                    WinHttpCloseHandle(self.0);
                }
            }
        }
    }

    pub(super) fn fetch(
        source: &OfficialAssetSource,
        maximum_bytes: usize,
    ) -> Result<Vec<u8>, AssetTransportError> {
        let agent = wide("RaymanCodingSkill verified update asset");
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
                ASSET_TIMEOUT_MS,
                ASSET_TIMEOUT_MS,
                ASSET_TIMEOUT_MS,
                ASSET_TIMEOUT_MS,
            )
        } == 0
        {
            return Err(AssetTransportError::Unavailable);
        }

        let host = wide(RELEASE_ASSET_HOST);
        let connection = WinHttpHandle::from_raw(unsafe {
            WinHttpConnect(
                session.as_raw(),
                host.as_ptr(),
                INTERNET_DEFAULT_HTTPS_PORT,
                0,
            )
        })?;
        let method = wide("GET");
        let object = wide(&source.object_path());
        let request = WinHttpHandle::from_raw(unsafe {
            WinHttpOpenRequest(
                connection.as_raw(),
                method.as_ptr(),
                object.as_ptr(),
                null(),
                null(),
                null(),
                WINHTTP_FLAG_SECURE,
            )
        })?;
        set_u32(
            &request,
            WINHTTP_OPTION_REDIRECT_POLICY,
            WINHTTP_OPTION_REDIRECT_POLICY_DISALLOW_HTTPS_TO_HTTP,
        )?;
        set_u32(&request, WINHTTP_OPTION_MAX_HTTP_AUTOMATIC_REDIRECTS, 1)?;

        let headers = wide("Accept: application/octet-stream\r\n");
        let header_length =
            u32::try_from(headers.len() - 1).map_err(|_| AssetTransportError::MalformedResponse)?;
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
            || unsafe { WinHttpReceiveResponse(request.as_raw(), null_mut()) } == 0
        {
            return Err(AssetTransportError::Unavailable);
        }

        let mut status = 0u32;
        let mut status_length =
            u32::try_from(std::mem::size_of_val(&status)).expect("status size fits u32");
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
            return Err(AssetTransportError::Unavailable);
        }
        if status != 200 {
            return Err(AssetTransportError::Unavailable);
        }

        let final_url = query_final_url(&request)?;
        if !allowed_final_url(&final_url) {
            return Err(AssetTransportError::RedirectRejected);
        }
        read_bounded(&request, maximum_bytes)
    }

    fn set_u32(handle: &WinHttpHandle, option: u32, value: u32) -> Result<(), AssetTransportError> {
        let length = u32::try_from(std::mem::size_of_val(&value)).expect("u32 size fits u32");
        if unsafe {
            WinHttpSetOption(
                handle.as_raw(),
                option,
                (&value as *const u32).cast(),
                length,
            )
        } == 0
        {
            return Err(AssetTransportError::Unavailable);
        }
        Ok(())
    }

    fn query_final_url(request: &WinHttpHandle) -> Result<String, AssetTransportError> {
        let mut buffer = [0u16; 4096];
        let mut length = u32::try_from(buffer.len() * std::mem::size_of::<u16>())
            .expect("fixed URL buffer fits u32");
        if unsafe {
            WinHttpQueryOption(
                request.as_raw(),
                WINHTTP_OPTION_URL,
                buffer.as_mut_ptr().cast(),
                &mut length,
            )
        } == 0
        {
            return Err(AssetTransportError::Unavailable);
        }
        let units = usize::try_from(length)
            .ok()
            .and_then(|bytes| bytes.checked_div(2))
            .ok_or(AssetTransportError::MalformedResponse)?;
        if units == 0 || units > buffer.len() {
            return Err(AssetTransportError::MalformedResponse);
        }
        let units = units.saturating_sub(usize::from(buffer[units - 1] == 0));
        String::from_utf16(&buffer[..units]).map_err(|_| AssetTransportError::MalformedResponse)
    }

    fn allowed_final_url(url: &str) -> bool {
        [
            "https://github.com/",
            "https://release-assets.githubusercontent.com/",
            "https://objects.githubusercontent.com/",
        ]
        .into_iter()
        .any(|prefix| url.starts_with(prefix))
    }

    fn read_bounded(
        request: &WinHttpHandle,
        maximum_bytes: usize,
    ) -> Result<Vec<u8>, AssetTransportError> {
        const CHUNK_SIZE: usize = 64 * 1024;
        let mut body = Vec::new();
        let mut chunk = [0u8; CHUNK_SIZE];
        loop {
            let mut available = 0u32;
            if unsafe { WinHttpQueryDataAvailable(request.as_raw(), &mut available) } == 0 {
                return Err(AssetTransportError::Unavailable);
            }
            if available == 0 {
                return Ok(body);
            }
            let available =
                usize::try_from(available).map_err(|_| AssetTransportError::MalformedResponse)?;
            let remaining = maximum_bytes
                .checked_sub(body.len())
                .ok_or(AssetTransportError::ResponseTooLarge)?;
            if available > remaining {
                return Err(AssetTransportError::ResponseTooLarge);
            }
            let requested = available.min(chunk.len());
            let mut read = 0u32;
            if unsafe {
                WinHttpReadData(
                    request.as_raw(),
                    chunk.as_mut_ptr().cast(),
                    u32::try_from(requested).expect("chunk fits u32"),
                    &mut read,
                )
            } == 0
            {
                return Err(AssetTransportError::Unavailable);
            }
            let read = usize::try_from(read).map_err(|_| AssetTransportError::MalformedResponse)?;
            if read == 0 || read > requested {
                return Err(AssetTransportError::MalformedResponse);
            }
            body.extend_from_slice(&chunk[..read]);
        }
    }

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }
}

#[cfg(not(windows))]
mod platform {
    use super::{AssetTransportError, OfficialAssetSource};

    pub(super) fn fetch(_: &OfficialAssetSource, _: usize) -> Result<Vec<u8>, AssetTransportError> {
        Err(AssetTransportError::Unavailable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asset_paths_are_fixed_and_cannot_accept_remote_names() {
        let version = ReleaseVersion::parse("2.11.0").unwrap();
        let source = manifest_source(version.clone());
        assert_eq!(
            source.object_path(),
            "/qinrm-lab/RaymanCodingSkill/releases/download/v2.11.0/rayman-update-manifest-v1.json"
        );
        assert_eq!(source.version(), &version);
        assert_eq!(source.asset_name(), MANIFEST_ASSET_NAME);
        assert_eq!(
            OfficialAssetSource::new(version, "../../evil.exe"),
            Err(AssetTransportError::InvalidSource)
        );
    }
}
