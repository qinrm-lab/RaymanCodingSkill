use axum::http::{HeaderName, HeaderValue, Method, header};
use tower_http::cors::{AllowOrigin, CorsLayer};

pub(crate) fn cors_layer() -> CorsLayer {
    CorsLayer::new()
        .allow_origin(AllowOrigin::list(cors_origins()))
        .allow_methods([Method::GET, Method::POST, Method::OPTIONS])
        .allow_headers([
            header::AUTHORIZATION,
            header::CONTENT_TYPE,
            HeaderName::from_static("x-api-key"),
        ])
}

fn cors_origins() -> Vec<HeaderValue> {
    let configured = std::env::var("RAYMAN_API_CORS_ORIGINS")
        .ok()
        .into_iter()
        .flat_map(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|origin| !origin.is_empty())
                .filter_map(|origin| HeaderValue::from_str(origin).ok())
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    if configured.is_empty() {
        default_cors_origins()
    } else {
        configured
    }
}

fn default_cors_origins() -> Vec<HeaderValue> {
    ["http://127.0.0.1:8000", "http://localhost:8000"]
        .into_iter()
        .filter_map(|origin| HeaderValue::from_str(origin).ok())
        .collect()
}
