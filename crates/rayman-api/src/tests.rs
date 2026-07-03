use super::*;
use axum::body;
use axum::body::Body;
use axum::http::{HeaderValue, Method, Request, header};
use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;
use std::time::Duration;
use tower::ServiceExt;

static ENV_LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();

async fn env_lock() -> tokio::sync::MutexGuard<'static, ()> {
    ENV_LOCK
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await
}

#[tokio::test]
async fn health_is_public() {
    let temp = tempfile::tempdir().unwrap();
    let response = app(temp.path())
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn api_requires_auth_configuration() {
    let _guard = env_lock().await;
    unsafe {
        std::env::remove_var("RAYMAN_API_KEY");
        std::env::remove_var("RAYMAN_API_TOKEN");
    }
    let temp = tempfile::tempdir().unwrap();
    let response = app(temp.path())
        .oneshot(
            Request::builder()
                .uri("/api/stats")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn api_json_contract_documents_routes_and_error_shape() {
    // @ui:api_json
    let _guard = env_lock().await;
    unsafe {
        std::env::remove_var("RAYMAN_API_KEY");
        std::env::remove_var("RAYMAN_API_TOKEN");
    }
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .unwrap()
        .to_path_buf();
    let documented = rayman_core::feature_coverage::documented_api_endpoints(&root).unwrap();
    let implemented = rayman_core::feature_coverage::implemented_api_endpoints(&root).unwrap();
    for endpoint in &implemented {
        assert!(
            documented.contains(endpoint),
            "implemented endpoint missing from docs/API.md: {endpoint}"
        );
    }

    let temp = tempfile::tempdir().unwrap();
    let response = app(temp.path())
        .oneshot(
            Request::builder()
                .uri("/api/stats")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let bytes = body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    assert!(value["detail"].as_str().unwrap().contains("RAYMAN_API_KEY"));
}

#[tokio::test]
async fn cors_uses_configured_origins() {
    let _guard = env_lock().await;
    unsafe {
        std::env::set_var("RAYMAN_API_CORS_ORIGINS", "http://example.test");
    }
    let temp = tempfile::tempdir().unwrap();
    let response = app(temp.path())
        .oneshot(
            Request::builder()
                .method(Method::OPTIONS)
                .uri("/health")
                .header(header::ORIGIN, "http://example.test")
                .header(header::ACCESS_CONTROL_REQUEST_METHOD, "GET")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    unsafe {
        std::env::remove_var("RAYMAN_API_CORS_ORIGINS");
    }
    assert_eq!(
        response.headers().get(header::ACCESS_CONTROL_ALLOW_ORIGIN),
        Some(&HeaderValue::from_static("http://example.test"))
    );
}

#[tokio::test]
async fn models_status_reads_model_update_config() {
    let _guard = env_lock().await;
    unsafe {
        std::env::set_var("RAYMAN_API_KEY", "secret");
    }
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir_all(temp.path().join("config")).unwrap();
    fs::write(
        temp.path().join("config").join("default_config.yaml"),
        "config_files:\n  model_updates: \"model_updates.yaml\"\n",
    )
    .unwrap();
    fs::write(
            temp.path().join("config").join("model_updates.yaml"),
            "auto_update:\n  enabled: true\n  interval_days: 9\nlast_update: null\nupdate_sources:\n  openai: true\n",
        )
        .unwrap();

    let response = app(temp.path())
        .oneshot(
            Request::builder()
                .uri("/api/models/status")
                .header("X-API-Key", "secret")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    unsafe {
        std::env::remove_var("RAYMAN_API_KEY");
    }
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["auto_update_enabled"], true);
    assert_eq!(value["interval_days"], 9);
}

#[tokio::test]
async fn review_is_blocked_by_pending_work_before_model_config_load() {
    let _guard = env_lock().await;
    unsafe {
        std::env::set_var("RAYMAN_API_KEY", "secret");
    }
    let temp = tempfile::tempdir().unwrap();
    SessionManager::new(temp.path())
        .unwrap()
        .add_pending(
            "finish first",
            "details",
            "review",
            "test",
            "must",
            serde_json::json!({}),
        )
        .unwrap();
    let response = app(temp.path())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/review")
                .header("X-API-Key", "secret")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::json!({"code": "fn main() {}", "language": "rust"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    unsafe {
        std::env::remove_var("RAYMAN_API_KEY");
    }
    assert_eq!(response.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn context_endpoint_returns_workspace_summary() {
    let _guard = env_lock().await;
    unsafe {
        std::env::set_var("RAYMAN_API_KEY", "secret");
    }
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("README.md"), "# project").unwrap();
    fs::write(temp.path().join("SKILL.md"), "# skill").unwrap();
    rayman_core::workspace::WorkspaceActivationManager::new(temp.path())
        .unwrap()
        .enable("test", "test")
        .unwrap();

    let response = app(temp.path())
        .oneshot(
            Request::builder()
                .uri("/api/context")
                .header("X-API-Key", "secret")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    unsafe {
        std::env::remove_var("RAYMAN_API_KEY");
    }
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["counts"]["project_inputs"], 2);
    assert!(
        value["source_policy"]
            .as_str()
            .unwrap()
            .contains("Current workspace files")
    );
    assert!(
        value["understanding_protocol"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item.as_str().unwrap().contains("task-scoped context"))
    );
    assert!(
        value["required_actions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item.as_str().unwrap().contains("context refresh"))
    );
}

#[tokio::test]
async fn context_os_endpoint_writes_workspace_state() {
    let _guard = env_lock().await;
    unsafe {
        std::env::set_var("RAYMAN_API_KEY", "secret");
    }
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("README.md"), "# project").unwrap();

    let response = app(temp.path())
        .oneshot(
            Request::builder()
                .uri("/api/context/os")
                .header("X-API-Key", "secret")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["status"], "missing");

    let response = app(temp.path())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/context/os")
                .header("X-API-Key", "secret")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    unsafe {
        std::env::remove_var("RAYMAN_API_KEY");
    }
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["status"], "ready");
    assert!(
        temp.path()
            .join(".RaymanCodingSkill/context/state.json")
            .exists()
    );
    assert!(
        temp.path()
            .join(".RaymanCodingSkill/context/events.jsonl")
            .exists()
    );
}

#[tokio::test]
async fn evidence_endpoint_returns_claim_ledger() {
    let _guard = env_lock().await;
    unsafe {
        std::env::set_var("RAYMAN_API_KEY", "secret");
    }
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("README.md"), "# project").unwrap();
    fs::write(temp.path().join("SKILL.md"), "# skill").unwrap();

    let response = app(temp.path())
        .oneshot(
            Request::builder()
                .uri("/api/evidence?scope=workspace")
                .header("X-API-Key", "secret")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    unsafe {
        std::env::remove_var("RAYMAN_API_KEY");
    }
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["scope"], "workspace");
    assert_eq!(value["evidence_status"], "verified");
    assert!(value["claim_ledger"]["claims"].as_array().unwrap().len() >= 2);
}

#[tokio::test]
async fn project_impact_and_regression_endpoints_return_reports() {
    let _guard = env_lock().await;
    unsafe {
        std::env::set_var("RAYMAN_API_KEY", "secret");
    }
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir_all(temp.path().join("src")).unwrap();
    fs::write(
        temp.path().join("package.json"),
        r#"{"scripts":{"test":"vitest"}}"#,
    )
    .unwrap();
    fs::write(
        temp.path().join("src").join("math.ts"),
        "export function add() {}\n",
    )
    .unwrap();

    let response = app(temp.path())
        .oneshot(
            Request::builder()
                .uri("/api/project")
                .header("X-API-Key", "secret")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    assert!(
        value["project_adapters"]
            .as_array()
            .unwrap()
            .iter()
            .any(|adapter| adapter["language"] == "typescript")
    );

    let response = app(temp.path())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/impact")
                .header("X-API-Key", "secret")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::json!({"paths": ["src/math.ts"]}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = app(temp.path())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/regression/plan")
                .header("X-API-Key", "secret")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::json!({"paths": ["src/math.ts"]}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    unsafe {
        std::env::remove_var("RAYMAN_API_KEY");
    }
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    assert!(value["risk_level"].as_str().is_some());
    assert!(!value["risk_reasons"].as_array().unwrap().is_empty());
    assert!(!value["recommended_focus"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn project_impact_rejects_outside_workspace_paths() {
    let _guard = env_lock().await;
    unsafe {
        std::env::set_var("RAYMAN_API_KEY", "secret");
    }
    let temp = tempfile::tempdir().unwrap();
    fs::write(
        temp.path().join("Cargo.toml"),
        "[package]\nname = \"demo\"\n",
    )
    .unwrap();
    let outside = tempfile::NamedTempFile::new().unwrap();

    let response = app(temp.path())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/impact")
                .header("X-API-Key", "secret")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::json!({"paths": [outside.path().to_string_lossy()]}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    unsafe {
        std::env::remove_var("RAYMAN_API_KEY");
    }

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn assets_endpoints_return_reports() {
    let _guard = env_lock().await;
    unsafe {
        std::env::set_var("RAYMAN_API_KEY", "secret");
    }
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("old.md"), "old behavior\n").unwrap();

    let response = app(temp.path())
        .oneshot(
            Request::builder()
                .uri("/api/assets")
                .header("X-API-Key", "secret")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = app(temp.path())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/assets/retire")
                .header("X-API-Key", "secret")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "path": "old.md",
                        "replacement": "new.md",
                        "reason": "replaced",
                        "validation_command": "cargo test",
                        "apply_delete": false
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["controller_scope"], "user_controller");
    assert!(
        value["ignored_roots"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item == ".RaymanCodingSkill/tmp")
    );
    assert!(value["cleanup_plan"].as_array().is_some());
    assert!(value["detected_references"].as_array().is_some());
    assert!(
        value["blockers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item.as_str().unwrap().contains("retirement candidate"))
    );

    let response = app(temp.path())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/assets/cleanup")
                .header("X-API-Key", "secret")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::json!({"apply": true}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    assert!(!temp.path().join("old.md").exists());
    assert!(
        value["records"]
            .as_array()
            .unwrap()
            .iter()
            .any(|record| record["path"] == "old.md" && record["status"] == "retired")
    );

    fs::write(
        temp.path().join("compat.md"),
        "old compatibility behavior\n",
    )
    .unwrap();
    let response = app(temp.path())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/assets/exempt")
                .header("X-API-Key", "secret")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "path": "compat.md",
                        "reason": "compatibility window",
                        "expires_at": "2999-01-01"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    unsafe {
        std::env::remove_var("RAYMAN_API_KEY");
    }
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn assets_reject_outside_workspace_paths() {
    let _guard = env_lock().await;
    unsafe {
        std::env::set_var("RAYMAN_API_KEY", "secret");
    }
    let temp = tempfile::tempdir().unwrap();
    let outside = tempfile::NamedTempFile::new().unwrap();

    let response = app(temp.path())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/assets/retire")
                .header("X-API-Key", "secret")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "path": outside.path().to_string_lossy(),
                        "replacement": "new.md",
                        "reason": "replaced",
                        "validation_command": "cargo test"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    unsafe {
        std::env::remove_var("RAYMAN_API_KEY");
    }

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn stats_endpoint_returns_auxiliary_contribution_totals() {
    let _guard = env_lock().await;
    unsafe {
        std::env::set_var("RAYMAN_API_KEY", "secret");
    }
    let temp = tempfile::tempdir().unwrap();
    let store = rayman_core::stats::AuxiliaryContributionStore::new(temp.path()).unwrap();
    let event =
        rayman_core::stats::AuxiliaryContributionEvent::implementation_validation_with_evidence(
            "success",
            "fixed",
            true,
            1,
            vec!["validator changed final code".into()],
        );
    store.record(&event).unwrap();
    let usage = rayman_core::stats::AuxiliaryUsageStore::new(temp.path()).unwrap();
    usage
        .record(&rayman_core::stats::AuxiliaryUsageEvent {
            task: "code_generation".into(),
            status: "success".into(),
            model: Some("aux/auto".into()),
            provider: Some("aux".into()),
            available: true,
            required: true,
            skip_reason: None,
            error: None,
            provider_attempts: Vec::new(),
            duration_ms: Some(10),
            failure_kind: None,
            estimated_cost_usd: None,
            created_at: rayman_core::now_iso(),
        })
        .unwrap();
    usage
        .record_main_ai("code_generation", "openai/gpt-4o")
        .unwrap();

    let response = app(temp.path())
        .oneshot(
            Request::builder()
                .uri("/api/stats")
                .header("X-API-Key", "secret")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    unsafe {
        std::env::remove_var("RAYMAN_API_KEY");
    }
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    let totals = &value["auxiliary_ai"]["contribution_stats"]["project_total"];
    assert_eq!(totals["production_count"], 1);
    assert_eq!(totals["contribution_count"], 1);
    assert_eq!(totals["contribution_percentage"].as_f64().unwrap(), 100.0);
    assert_eq!(
        value["auxiliary_ai"]["contribution_stats"]["events"][0]["evidence"][0],
        "validator changed final code"
    );
    assert_eq!(
        value["auxiliary_ai"]["contribution_stats"]["events"][0]["reason"],
        "auxiliary validation corrected the primary result"
    );
    let usage_totals = &value["auxiliary_ai"]["usage_stats"]["project_total"];
    assert_eq!(usage_totals["attempt_count"], 1);
    assert_eq!(usage_totals["call_count"], 1);
    assert_eq!(usage_totals["queued_count"], 0);
    assert_eq!(usage_totals["main_ai_count"], 1);
    assert_eq!(
        usage_totals["auxiliary_success_rate"].as_f64().unwrap(),
        100.0
    );
    assert_eq!(
        usage_totals["auxiliary_call_success_rate"]
            .as_f64()
            .unwrap(),
        100.0
    );
    assert_eq!(
        value["auxiliary_ai"]["usage_stats"]["by_task"]["code_generation"]["success_count"],
        1
    );
    assert_eq!(
        value["auxiliary_ai"]["usage_stats"]["by_provider"]["aux"]["success_count"],
        1
    );
    assert_eq!(
        value["auxiliary_ai"]["usage_stats"]["by_provider"]["openai"]["main_ai_count"],
        1
    );
    assert_eq!(value["goals"]["active"], 0);
    assert_eq!(value["research_agents"]["total_sessions"], 0);
}

#[tokio::test]
async fn research_api_starts_and_gets_session() {
    let _guard = env_lock().await;
    unsafe {
        std::env::set_var("RAYMAN_API_KEY", "secret");
    }
    let temp = tempfile::tempdir().unwrap();

    let response = app(temp.path())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/research")
                .header("X-API-Key", "secret")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "question": "why did validation fail?",
                        "goal_id": "goal_123"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let started: Value = serde_json::from_slice(&bytes).unwrap();
    let id = started["id"].as_str().unwrap();
    assert_eq!(started["status"], "active");
    assert_eq!(started["autonomy_policy"]["can_edit_files"], false);

    let response = app(temp.path())
        .oneshot(
            Request::builder()
                .uri(format!("/api/research/{id}"))
                .header("X-API-Key", "secret")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    unsafe {
        std::env::remove_var("RAYMAN_API_KEY");
    }
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["id"], id);
    assert_eq!(value["goal_id"], "goal_123");
}

#[tokio::test]
async fn research_run_partial_model_override_is_bad_request() {
    let _guard = env_lock().await;
    unsafe {
        std::env::set_var("RAYMAN_API_KEY", "secret");
    }
    let temp = tempfile::tempdir().unwrap();

    let response = app(temp.path())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/research/research_123/run")
                .header("X-API-Key", "secret")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::json!({"model_type": "openai"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    unsafe {
        std::env::remove_var("RAYMAN_API_KEY");
    }

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn research_run_invalid_route_mode_is_bad_request() {
    let _guard = env_lock().await;
    unsafe {
        std::env::set_var("RAYMAN_API_KEY", "secret");
    }
    let temp = tempfile::tempdir().unwrap();

    let response = app(temp.path())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/research/research_123/run")
                .header("X-API-Key", "secret")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::json!({"route_mode": "parallel"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    unsafe {
        std::env::remove_var("RAYMAN_API_KEY");
    }

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn research_run_explicit_route_requires_agent_manager_config() {
    let _guard = env_lock().await;
    unsafe {
        std::env::set_var("RAYMAN_API_KEY", "secret");
    }
    let temp = tempfile::tempdir().unwrap();
    let session = ResearchManager::new(temp.path())
        .unwrap()
        .start("why did validation fail?", None)
        .unwrap();

    let response = app(temp.path())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/api/research/{}/run", session.id))
                .header("X-API-Key", "secret")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "model_type": "openai",
                        "model_name": "gpt-4o"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    unsafe {
        std::env::remove_var("RAYMAN_API_KEY");
    }

    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let stored = ResearchManager::new(temp.path())
        .unwrap()
        .status(Some(&session.id))
        .unwrap();
    assert!(stored["findings"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn goals_clarify_api_returns_structured_defaults() {
    let _guard = env_lock().await;
    unsafe {
        std::env::set_var("RAYMAN_API_KEY", "secret");
    }
    let temp = tempfile::tempdir().unwrap();

    let response = app(temp.path())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/goals/clarify")
                .header("X-API-Key", "secret")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "goal": "支持导出客户订单",
                        "requirements": ["必须导出 XLSX"],
                        "acceptance": ["导出文件包含订单号"],
                        "verification": ["cargo test -p exporter"]
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    unsafe {
        std::env::remove_var("RAYMAN_API_KEY");
    }

    assert_eq!(response.status(), StatusCode::OK);
    let bytes = body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["goal_summary"], "客户希望完成：支持导出客户订单");
    assert_eq!(value["inferred_requirements"][0], "必须导出 XLSX");
    assert!(
        value["default_choices"]
            .as_array()
            .unwrap()
            .iter()
            .any(|choice| choice["id"] == "permission_policy")
    );
}

#[tokio::test]
async fn goals_api_starts_runs_gets_and_closes_goal() {
    let _guard = env_lock().await;
    unsafe {
        std::env::set_var("RAYMAN_API_KEY", "secret");
    }
    let temp = tempfile::tempdir().unwrap();

    let response = app(temp.path())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/goals")
                .header("X-API-Key", "secret")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "goal": "ship autonomous goal runner",
                        "acceptance": ["goal can run"],
                        "verification": ["cargo test"]
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let started: Value = serde_json::from_slice(&bytes).unwrap();
    let id = started["id"].as_str().unwrap();
    assert_eq!(
        started["contract"]["clarification"]["goal_summary"],
        "Customer wants to complete: ship autonomous goal runner"
    );
    assert!(
        started["contract"]["clarification"]["default_choices"]
            .as_array()
            .unwrap()
            .iter()
            .any(|choice| choice["id"] == "permission_policy")
    );

    let response = app(temp.path())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/api/goals/{id}/run"))
                .header("X-API-Key", "secret")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["current_stage"], "plan");
    assert_eq!(value["long_run_report"]["iterations"], 1);
    assert_eq!(
        value["long_run_report"]["stopped_reason"],
        "next_step_completed"
    );

    let response = app(temp.path())
        .oneshot(
            Request::builder()
                .uri(format!("/api/goals/{id}"))
                .header("X-API-Key", "secret")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["current_stage"], "plan");

    let response = app(temp.path())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/api/goals/{id}/run"))
                .header("X-API-Key", "secret")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "until": "summary",
                        "checkpoint_interval_minutes": 0,
                        "max_repair_attempts": 2
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["current_stage"], "summary");
    assert_eq!(value["long_run_report"]["until"], "summary");
    assert!(value["long_run_report"]["iterations"].as_u64().unwrap() >= 1);

    rayman_core::context::ContextKernel::new(temp.path())
        .unwrap()
        .refresh_index()
        .unwrap();
    let response = app(temp.path())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/api/goals/{id}/run"))
                .header("X-API-Key", "secret")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::json!({"validation": "passed", "message": "cargo test passed"})
                        .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = app(temp.path())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/api/goals/{id}/close"))
                .header("X-API-Key", "secret")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "status": "success",
                        "message": "req_1: implemented and cargo test passed\nchecked: cargo test passed\nnegative check: stale success evidence not found; evidence: cargo test passed"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    unsafe {
        std::env::remove_var("RAYMAN_API_KEY");
    }
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["status"], "success");
}

#[tokio::test]
async fn goals_api_returns_not_found_for_missing_goal() {
    let _guard = env_lock().await;
    unsafe {
        std::env::set_var("RAYMAN_API_KEY", "secret");
    }
    let temp = tempfile::tempdir().unwrap();

    let response = app(temp.path())
        .oneshot(
            Request::builder()
                .uri("/api/goals/goal_missing")
                .header("X-API-Key", "secret")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    unsafe {
        std::env::remove_var("RAYMAN_API_KEY");
    }
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_endpoint_returns_auxiliary_attempt_fields() {
    let _guard = env_lock().await;
    unsafe {
        std::env::set_var("RAYMAN_API_KEY", "secret");
    }
    let aux = openai_test_server("ADVICE");
    let primary = openai_test_server("```rust\n#[test]\nfn test_x() {}\n```");
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir_all(temp.path().join("config")).unwrap();
    fs::write(
        temp.path().join("config").join("default_config.yaml"),
        format!(
            r#"config_files: {{}}
default_model:
  type: primary
  name: primary-model
models:
  primary:
    adapter: openai_compatible
    auth_required: false
    base_url: "{primary}"
    timeout: 5
  aux:
    adapter: openai_compatible
    auth_required: false
    base_url: "{aux}"
    timeout: 5
auxiliary_ai:
  enabled: true
  provider: aux
  model: aux-model
  fail_open: true
  required_when_available: true
  record_skip_reason: true
  tasks:
    - test_generation
"#,
            primary = primary,
            aux = aux
        ),
    )
    .unwrap();

    let response = app(temp.path())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/test")
                .header("X-API-Key", "secret")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::json!({"code": "fn x() {}", "language": "rust"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    unsafe {
        std::env::remove_var("RAYMAN_API_KEY");
    }

    assert_eq!(response.status(), StatusCode::OK);
    let bytes = body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["evidence_status"], "unknown");
    assert!(value["claim_ledger"]["claims"].as_array().is_some());
    assert!(!value["unknowns"].as_array().unwrap().is_empty());
    assert_eq!(value["auxiliary_ai"]["status"], "queued");
    assert_eq!(value["auxiliary_ai"]["task"], "test_generation");
    assert_eq!(value["auxiliary_ai"]["required"], true);
    assert_eq!(value["auxiliary_ai"]["selected_provider"], "aux");
    assert!(value["auxiliary_ai"]["queued_task_id"].as_str().is_some());
    assert_eq!(value["auxiliary_ai"]["async_status"], "queued");
    assert_eq!(value["auxiliary_ai"]["reconciliation_status"], "pending");
    let upgraded =
        rayman_core::config::load_yaml(temp.path().join("config").join("default_config.yaml"))
            .unwrap();
    assert!(rayman_core::config::get_path(&upgraded, "auxiliary_ai.provider").is_none());
    assert!(
        rayman_core::config::get_path(&upgraded, "auxiliary_ai.providers")
            .and_then(serde_yaml::Value::as_sequence)
            .is_some()
    );
}

#[tokio::test]
async fn partial_model_override_is_bad_request() {
    let _guard = env_lock().await;
    unsafe {
        std::env::set_var("RAYMAN_API_KEY", "secret");
    }
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir_all(temp.path().join("config")).unwrap();
    fs::write(
        temp.path().join("config").join("default_config.yaml"),
        "config_files: {}\ndefault_model:\n  type: openai\n  name: gpt-4\n",
    )
    .unwrap();
    let response = app(temp.path())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/test")
                .header("X-API-Key", "secret")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::json!({"code": "fn main() {}", "model_type": "openai"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    unsafe {
        std::env::remove_var("RAYMAN_API_KEY");
    }
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn internal_errors_are_sanitized() {
    let _guard = env_lock().await;
    unsafe {
        std::env::set_var("RAYMAN_API_KEY", "secret");
    }
    let temp = tempfile::tempdir().unwrap();
    let response = app(temp.path())
        .oneshot(
            Request::builder()
                .uri("/api/models")
                .header("X-API-Key", "secret")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    unsafe {
        std::env::remove_var("RAYMAN_API_KEY");
    }
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let bytes = body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["detail"], "Internal server error.");
}

fn openai_test_server(content: &'static str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            read_http_request(&mut stream);
            let encoded = serde_json::to_string(content).unwrap();
            let body = format!(r#"{{"choices":[{{"message":{{"content":{encoded}}}}}]}}"#);
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).unwrap();
        }
    });
    format!("http://{addr}/v1")
}

fn read_http_request(stream: &mut std::net::TcpStream) {
    stream
        .set_read_timeout(Some(Duration::from_millis(200)))
        .unwrap();
    let mut buffer = Vec::new();
    let mut chunk = [0; 1024];
    while let Ok(n) = stream.read(&mut chunk) {
        if n == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..n]);
        let request = String::from_utf8_lossy(&buffer);
        if let Some(header_end) = request.find("\r\n\r\n") {
            let content_length = request[..header_end]
                .lines()
                .find_map(|line| {
                    line.split_once(':').and_then(|(name, value)| {
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().ok())
                            .flatten()
                    })
                })
                .unwrap_or(0);
            if buffer.len() >= header_end + 4 + content_length {
                break;
            }
        }
    }
}
