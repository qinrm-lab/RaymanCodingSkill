use axum::http::{HeaderMap, StatusCode};

use crate::ApiError;

pub(crate) fn require_api_key(headers: &HeaderMap) -> Result<(), ApiError> {
    let expected = std::env::var("RAYMAN_API_KEY")
        .ok()
        .or_else(|| std::env::var("RAYMAN_API_TOKEN").ok())
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "API authentication is not configured; set RAYMAN_API_KEY before using /api/*.",
            )
        })?;
    let header_key = headers
        .get("X-API-Key")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let bearer = headers
        .get("Authorization")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::trim)
        .map(str::to_string);
    let provided = header_key.or(bearer);
    if provided
        .as_deref()
        .is_some_and(|value| constant_time_eq(value.as_bytes(), expected.as_bytes()))
    {
        Ok(())
    } else {
        Err(ApiError::new(
            StatusCode::UNAUTHORIZED,
            "Invalid or missing API key.",
        ))
    }
}

/// 常量时间比较，避免通过响应时序侧信道逐字节猜测 API key。
/// 长度不同直接返回 false，但仍对等长部分做全量异或以保持时序稳定。
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::constant_time_eq;

    #[test]
    fn constant_time_eq_matches_only_identical_bytes() {
        assert!(constant_time_eq(b"secret-token", b"secret-token"));
        assert!(!constant_time_eq(b"secret-token", b"secret-toke"));
        assert!(!constant_time_eq(b"secret-token", b"wrong-token!"));
        assert!(constant_time_eq(b"", b""));
    }
}
