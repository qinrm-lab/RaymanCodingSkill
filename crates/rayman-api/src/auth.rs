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
    if provided.as_deref() == Some(expected.as_str()) {
        Ok(())
    } else {
        Err(ApiError::new(
            StatusCode::UNAUTHORIZED,
            "Invalid or missing API key.",
        ))
    }
}
