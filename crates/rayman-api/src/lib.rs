use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use rayman_core::assets::{
    AssetCleanupRequest, AssetExemptRequest, AssetRetireRequest, AssetRetirementManager,
};
use rayman_core::config::ConfigManager;
use rayman_core::context::{ContextKernel, ContextSummary};
use rayman_core::evidence::{
    Claim, ClaimLedger, EvidenceCheckOptions, EvidenceStatus, check_workspace_evidence,
};
use rayman_core::goal::{
    GoalManager, GoalRunOptions, GoalRunReport, GoalRunUntil, build_goal_clarification,
};
use rayman_core::integrations::IntegrationManager;
use rayman_core::models::AgentManager;
use rayman_core::now_iso;
use rayman_core::project::ProjectAnalyzer;
use rayman_core::quality::QualityManager;
use rayman_core::research::ResearchManager;
use rayman_core::session::SessionManager;
use rayman_core::skills;
use rayman_core::stats::{AuxiliaryContributionStore, AuxiliaryUsageStore};
use serde_json::{Value, json};

mod auth;
mod cors;
mod error;
mod model_status;
mod types;

use auth::require_api_key;
use cors::cors_layer;
pub use error::ApiError;
use model_status::{model_update_sources_json, model_update_status_json};
pub use types::*;

#[derive(Clone)]
pub struct ApiState {
    pub root: PathBuf,
}

pub fn app(root: impl Into<PathBuf>) -> Router {
    Router::new()
        .route("/", get(root_handler))
        .route("/health", get(health_handler))
        .route("/api/generate", post(generate_handler))
        .route("/api/review", post(review_handler))
        .route("/api/test", post(test_handler))
        .route("/api/models", get(models_handler))
        .route("/api/models/status", get(models_status_handler))
        .route("/api/models/update", post(models_update_handler))
        .route("/api/mcp/tools", get(mcp_tools_handler))
        .route("/api/mcp/resources", get(mcp_resources_handler))
        .route("/api/plugin/manifest", get(plugin_manifest_handler))
        .route("/api/context", get(context_handler))
        .route(
            "/api/context/os",
            get(context_os_handler).post(context_os_write_handler),
        )
        .route("/api/project", get(project_handler))
        .route("/api/project/index", post(project_index_handler))
        .route("/api/impact", post(impact_handler))
        .route("/api/regression/plan", post(regression_plan_handler))
        .route("/api/assets", get(assets_handler))
        .route("/api/assets/scan", post(assets_scan_handler))
        .route("/api/assets/cleanup", post(assets_cleanup_handler))
        .route("/api/assets/retire", post(assets_retire_handler))
        .route("/api/assets/exempt", post(assets_exempt_handler))
        .route("/api/evidence", get(evidence_handler))
        .route("/api/stats", get(stats_handler))
        .route("/api/goals/clarify", post(goal_clarify_handler))
        .route("/api/goals", post(goal_start_handler))
        .route("/api/goals/{id}", get(goal_get_handler))
        .route("/api/goals/{id}/run", post(goal_run_handler))
        .route("/api/goals/{id}/close", post(goal_close_handler))
        .route("/api/research", post(research_start_handler))
        .route("/api/research/{id}", get(research_get_handler))
        .route("/api/research/{id}/run", post(research_run_handler))
        .route(
            "/api/research/{id}/reconcile",
            post(research_reconcile_handler),
        )
        .with_state(ApiState { root: root.into() })
        .layer(cors_layer())
}

pub fn mcp_app(root: impl Into<PathBuf>) -> Router {
    Router::new()
        .route("/", get(mcp_root_handler))
        .route("/mcp", post(mcp_rpc_handler))
        .with_state(ApiState { root: root.into() })
        .layer(cors_layer())
}

pub async fn serve(root: impl Into<PathBuf>, host: &str, port: u16) -> Result<()> {
    let addr: SocketAddr = format!("{host}:{port}")
        .parse()
        .with_context(|| format!("无效监听地址: {host}:{port}"))?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app(root)).await?;
    Ok(())
}

pub async fn serve_mcp(root: impl Into<PathBuf>, host: &str, port: u16) -> Result<()> {
    let addr: SocketAddr = format!("{host}:{port}")
        .parse()
        .with_context(|| format!("无效 MCP 监听地址: {host}:{port}"))?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, mcp_app(root)).await?;
    Ok(())
}

async fn root_handler() -> Json<Value> {
    Json(json!({"name": "RaymanCodingSkill API", "version": "1.0.0", "status": "running"}))
}

async fn health_handler() -> Json<Value> {
    Json(json!({"status": "healthy", "version": "1.0.0"}))
}

async fn mcp_root_handler() -> Json<Value> {
    Json(json!({
        "name": "RaymanCodingSkill MCP",
        "version": "1.0.0",
        "status": "running",
        "endpoint": "/mcp",
        "source_policy": "MCP is a read-only integration surface; Rayman evidence remains authoritative."
    }))
}

async fn mcp_rpc_handler(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(payload): Json<Value>,
) -> Result<Response, ApiError> {
    // MCP over HTTP 暴露工作区治理状态，必须与 /api/* 一样鉴权（stdio 传输不受影响）。
    require_api_key(&headers)?;
    let manager = IntegrationManager::new(state.root)?;
    Ok(match manager.mcp_rpc_response(payload) {
        Some(response) => Json(response).into_response(),
        None => StatusCode::ACCEPTED.into_response(),
    })
}

struct ApiEvidenceFields {
    evidence_status: String,
    claim_ledger: Value,
    unknowns: Vec<String>,
    assumptions: Vec<String>,
    blockers: Vec<String>,
}

fn api_unknown_evidence(text: &str) -> Result<ApiEvidenceFields> {
    let claim = Claim {
        id: "api_response".into(),
        text: text.into(),
        status: EvidenceStatus::Unknown,
        evidence_refs: Vec::new(),
        search_effort: Vec::new(),
        counterexample_challenges: Vec::new(),
        blockers: vec![
            "API model output has no current workspace path, successful validation command, or evidence artifact attached".into(),
        ],
        checked_at: now_iso(),
    };
    let ledger = ClaimLedger::new(vec![claim]);
    Ok(ApiEvidenceFields {
        evidence_status: ledger.status().as_str().into(),
        claim_ledger: serde_json::to_value(&ledger)?,
        unknowns: ledger.unknowns(),
        assumptions: ledger.assumptions(),
        blockers: ledger.blockers(),
    })
}

fn append_evidence_summary(
    value: Value,
    root: PathBuf,
    scope: &str,
    goal_id: Option<String>,
    include_advisory: bool,
) -> Result<Value> {
    let report = check_workspace_evidence(
        root,
        EvidenceCheckOptions {
            scope: scope.into(),
            goal_id,
            include_advisory,
        },
    )?;
    let mut value = value;
    let object = value
        .as_object_mut()
        .context("API evidence summary can only be appended to JSON objects")?;
    object.insert(
        "evidence_status".into(),
        Value::String(report.status.as_str().into()),
    );
    object.insert(
        "claim_ledger".into(),
        serde_json::to_value(report.claim_ledger)?,
    );
    object.insert("unknowns".into(), serde_json::to_value(report.unknowns)?);
    object.insert(
        "assumptions".into(),
        serde_json::to_value(report.assumptions)?,
    );
    object.insert("blockers".into(), serde_json::to_value(report.blockers)?);
    Ok(value)
}

fn api_goal_run_options(request: &GoalRunRequest) -> Result<GoalRunOptions> {
    Ok(GoalRunOptions {
        until: request
            .until
            .as_deref()
            .map(GoalRunUntil::parse)
            .transpose()?
            .unwrap_or_default(),
        checkpoint_interval_minutes: request.checkpoint_interval_minutes.unwrap_or(10),
        max_repair_attempts: request.max_repair_attempts.unwrap_or(3),
    })
}

fn goal_run_record_response_value(record: rayman_core::goal::GoalRecord) -> Result<Value> {
    let mut value = serde_json::to_value(record)?;
    value
        .as_object_mut()
        .context("goal run response must be a JSON object")?
        .insert(
            "long_run_report".into(),
            json!({
                "status": "recorded_validation",
                "iterations": 0,
                "stopped_reason": "validation_result_recorded",
                "checkpoints": [],
            }),
        );
    Ok(value)
}

fn goal_run_report_response_value(report: GoalRunReport) -> Result<Value> {
    let mut value = serde_json::to_value(&report.goal)?;
    value
        .as_object_mut()
        .context("goal run response must be a JSON object")?
        .insert(
            "long_run_report".into(),
            json!({
                "status": report.status,
                "until": report.until,
                "iterations": report.iterations,
                "stopped_reason": report.stopped_reason,
                "checkpoints": report.checkpoints,
                "resume_command": report.resume_command,
                "blockers": report.blockers,
                "subagent_dispatch": report.subagent_dispatch,
            }),
        );
    Ok(value)
}

async fn generate_handler(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(request): Json<CodeGenerationRequest>,
) -> Result<Json<CodeGenerationResponse>, ApiError> {
    require_api_key(&headers)?;
    validate_model_override(&request.model_type, &request.model_name)?;
    let root = state.root.clone();
    Ok(Json(
        blocking_model_call(move || {
            let mut manager =
                AgentManager::new(root, request.model_type, request.model_name, None, false)?;
            let generated =
                skills::generate_code(&mut manager, &request.prompt, &request.language)?;
            let validation = skills::validate_and_fix(
                &mut manager,
                &generated,
                &request.prompt,
                &request.language,
            )?;
            let public = skills::validation_public_json(&validation)?;
            let evidence = api_unknown_evidence(
                "generated code is not verified until backed by current file or validation command evidence",
            )?;
            Ok(CodeGenerationResponse {
                code: validation.final_code,
                language: request.language,
                model: manager.last_successful_model(),
                evidence_status: evidence.evidence_status,
                claim_ledger: evidence.claim_ledger,
                unknowns: evidence.unknowns,
                assumptions: evidence.assumptions,
                blockers: evidence.blockers,
                auxiliary_ai: manager.auxiliary_usage_json(),
                implementation_validation: Some(public),
                edge_cases: validation.edge_cases,
                logic_simulation: validation.logic_simulation,
                potential_bugs: validation.potential_bugs,
                fixes_applied: validation.fixes_applied,
                validation_summary: Some(validation.validation_summary),
            })
        })
        .await?,
    ))
}

async fn review_handler(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(request): Json<CodeReviewRequest>,
) -> Result<Json<CodeReviewResponse>, ApiError> {
    require_api_key(&headers)?;
    validate_model_override(&request.model_type, &request.model_name)?;
    enforce_review_gate(&state, &request.code, request.reviewed_path.as_deref())?;
    let root = state.root.clone();
    Ok(Json(
        blocking_model_call(move || {
            let mut manager =
                AgentManager::new(root, request.model_type, request.model_name, None, false)?;
            let result = skills::review_code(
                &mut manager,
                &request.code,
                &request.language,
                request.workspace_path.as_deref(),
                request.reviewed_path.as_deref(),
            )?;
            let evidence = api_unknown_evidence(
                "review output is advisory until backed by current file and validation evidence",
            )?;
            Ok(CodeReviewResponse {
                review: result.review,
                issues: result.issues,
                suggestions: result.suggestions,
                score: result.score,
                structured_fields_available: result.structured_fields_available,
                evidence_status: evidence.evidence_status,
                claim_ledger: evidence.claim_ledger,
                unknowns: evidence.unknowns,
                assumptions: evidence.assumptions,
                blockers: evidence.blockers,
                auxiliary_ai: manager.auxiliary_usage_json(),
            })
        })
        .await?,
    ))
}

async fn test_handler(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(request): Json<TestGenerationRequest>,
) -> Result<Json<TestGenerationResponse>, ApiError> {
    require_api_key(&headers)?;
    validate_model_override(&request.model_type, &request.model_name)?;
    let root = state.root.clone();
    Ok(Json(
        blocking_model_call(move || {
            let mut manager =
                AgentManager::new(root, request.model_type, request.model_name, None, false)?;
            let result = skills::generate_tests(
                &mut manager,
                &request.code,
                &request.language,
                &request.test_types,
            )?;
            let evidence = api_unknown_evidence(
                "generated tests are not verified until backed by current file or validation command evidence",
            )?;
            Ok(TestGenerationResponse {
                test_code: result.test_code,
                test_count: result.test_count,
                test_types: result.test_types,
                evidence_status: evidence.evidence_status,
                claim_ledger: evidence.claim_ledger,
                unknowns: evidence.unknowns,
                assumptions: evidence.assumptions,
                blockers: evidence.blockers,
                auxiliary_ai: manager.auxiliary_usage_json(),
            })
        })
        .await?,
    ))
}

async fn models_handler(
    State(state): State<ApiState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    require_api_key(&headers)?;
    let config = ConfigManager::new(state.root)?;
    let value = config
        .model_catalog()
        .map(serde_json::to_value)
        .transpose()
        .map_err(|error| ApiError::from(anyhow::Error::new(error)))?
        .unwrap_or_else(|| json!({}));
    Ok(Json(value))
}

async fn models_status_handler(
    State(state): State<ApiState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    require_api_key(&headers)?;
    let config = ConfigManager::new(state.root)?;
    Ok(Json(model_update_status_json(&config)))
}

async fn models_update_handler(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Query(query): Query<UpdateQuery>,
) -> Result<Json<Value>, ApiError> {
    require_api_key(&headers)?;
    let config = ConfigManager::new(state.root)?;
    let status = model_update_status_json(&config);
    let auto_update = status
        .get("auto_update_enabled")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let checked = query.force || auto_update;
    let auxiliary_ai = if checked {
        match config.refresh_auxiliary_ai_from_settings(query.force) {
            Ok(value) => value.unwrap_or(Value::Null),
            Err(error) => json!({"error": error.to_string()}),
        }
    } else {
        Value::Null
    };
    Ok(Json(json!({
        "updated": false,
        "checked": checked,
        "force": query.force,
        "auto_update_enabled": auto_update,
        "interval_days": status.get("interval_days").cloned().unwrap_or(Value::Null),
        "providers": model_update_sources_json(&config),
        "new_models": {},
        "report_path": Value::Null,
        "auxiliary_ai": auxiliary_ai,
        "next_update": status.get("next_update").cloned().unwrap_or(Value::Null),
        "message": if checked {
            "model catalog checked; no automatic catalog mutation performed"
        } else {
            "model auto-update disabled; use force=true to check metadata"
        }
    })))
}

async fn mcp_tools_handler(
    State(state): State<ApiState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    require_api_key(&headers)?;
    let manager = IntegrationManager::new(state.root)?;
    Ok(Json(json!({
        "tools": manager.mcp_tools(),
        "source_policy": "MCP descriptors are integration metadata; current workspace evidence remains authoritative."
    })))
}

async fn mcp_resources_handler(
    State(state): State<ApiState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    require_api_key(&headers)?;
    let manager = IntegrationManager::new(state.root)?;
    Ok(Json(json!({
        "resources": manager.mcp_resources(),
        "source_policy": "MCP resources are integration metadata; current workspace evidence remains authoritative."
    })))
}

async fn plugin_manifest_handler(
    State(state): State<ApiState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    require_api_key(&headers)?;
    let manager = IntegrationManager::new(state.root)?;
    Ok(Json(serde_json::to_value(manager.plugin_manifest())?))
}

async fn context_handler(
    State(state): State<ApiState>,
    headers: HeaderMap,
) -> Result<Json<ContextSummary>, ApiError> {
    require_api_key(&headers)?;
    let summary = ContextKernel::new(state.root)?.collect()?;
    Ok(Json(summary))
}

async fn context_os_handler(
    State(state): State<ApiState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    require_api_key(&headers)?;
    Ok(Json(ContextKernel::new(state.root)?.context_os_status()?))
}

async fn context_os_write_handler(
    State(state): State<ApiState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    require_api_key(&headers)?;
    let state = ContextKernel::new(state.root)?.refresh_context_os("api_context_os_write")?;
    Ok(Json(json!({
        "written": true,
        "status": "ready",
        "state_path": ".RaymanCodingSkill/context/state.json",
        "event_log_path": ".RaymanCodingSkill/context/events.jsonl",
        "state": state,
    })))
}

async fn project_handler(
    State(state): State<ApiState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    require_api_key(&headers)?;
    let report = ProjectAnalyzer::new(state.root)?.detect()?;
    Ok(Json(serde_json::to_value(report)?))
}

async fn project_index_handler(
    State(state): State<ApiState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    require_api_key(&headers)?;
    let root = state.root.clone();
    let report = ProjectAnalyzer::new(&root)?.write_index()?;
    ContextKernel::new(root)?.refresh_index()?;
    Ok(Json(serde_json::to_value(report)?))
}

async fn impact_handler(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(request): Json<ImpactRequest>,
) -> Result<Json<Value>, ApiError> {
    require_api_key(&headers)?;
    let paths = request
        .paths
        .into_iter()
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    let report = ProjectAnalyzer::new(state.root)?
        .impact(&paths)
        .map_err(project_api_error)?;
    Ok(Json(serde_json::to_value(report)?))
}

async fn regression_plan_handler(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(request): Json<ImpactRequest>,
) -> Result<Json<Value>, ApiError> {
    require_api_key(&headers)?;
    let paths = request
        .paths
        .into_iter()
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    let report = ProjectAnalyzer::new(state.root)?
        .regression_plan(&paths)
        .map_err(project_api_error)?;
    Ok(Json(serde_json::to_value(report)?))
}

async fn assets_handler(
    State(state): State<ApiState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    require_api_key(&headers)?;
    let report = AssetRetirementManager::new(state.root)?
        .status()
        .map_err(project_api_error)?;
    Ok(Json(serde_json::to_value(report)?))
}

async fn assets_scan_handler(
    State(state): State<ApiState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    require_api_key(&headers)?;
    let report = AssetRetirementManager::new(state.root)?
        .scan()
        .map_err(project_api_error)?;
    Ok(Json(serde_json::to_value(report)?))
}

async fn assets_cleanup_handler(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(request): Json<AssetCleanupApiRequest>,
) -> Result<Json<Value>, ApiError> {
    require_api_key(&headers)?;
    let report = AssetRetirementManager::new(state.root)?
        .cleanup(AssetCleanupRequest {
            apply: request.apply,
        })
        .map_err(project_api_error)?;
    Ok(Json(serde_json::to_value(report)?))
}

async fn assets_retire_handler(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(request): Json<AssetRetireApiRequest>,
) -> Result<Json<Value>, ApiError> {
    require_api_key(&headers)?;
    let report = AssetRetirementManager::new(state.root)?
        .retire(AssetRetireRequest {
            path: PathBuf::from(request.path),
            replacement_behavior: request.replacement,
            deletion_reason: request.reason,
            validation_command: request.validation_command,
            apply_delete: request.apply_delete,
        })
        .map_err(project_api_error)?;
    Ok(Json(serde_json::to_value(report)?))
}

async fn assets_exempt_handler(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(request): Json<AssetExemptApiRequest>,
) -> Result<Json<Value>, ApiError> {
    require_api_key(&headers)?;
    let report = AssetRetirementManager::new(state.root)?
        .exempt(AssetExemptRequest {
            path: PathBuf::from(request.path),
            retention_reason: request.reason,
            expires_at: request.expires_at,
        })
        .map_err(project_api_error)?;
    Ok(Json(serde_json::to_value(report)?))
}

async fn evidence_handler(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Query(query): Query<EvidenceQuery>,
) -> Result<Json<Value>, ApiError> {
    require_api_key(&headers)?;
    let scope = query.scope.unwrap_or_else(|| "workspace".into());
    if !["workspace", "goal", "session", "research"].contains(&scope.as_str()) {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "scope must be workspace, goal, session, or research.",
        ));
    }
    let report = check_workspace_evidence(
        state.root,
        EvidenceCheckOptions {
            scope,
            goal_id: query.goal_id,
            include_advisory: query.include_advisory,
        },
    )?;
    Ok(Json(serde_json::to_value(report)?))
}

async fn stats_handler(
    State(state): State<ApiState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    require_api_key(&headers)?;
    let usage_stats = AuxiliaryUsageStore::new(&state.root)?.report_without_round()?;
    let contribution_stats =
        AuxiliaryContributionStore::new(&state.root)?.report_without_round()?;
    let goal_stats = GoalManager::new(&state.root)?.stats()?;
    let quality_stats = QualityManager::new(&state.root)?.stats()?;
    let research_stats = ResearchManager::new(&state.root)?.stats()?;
    Ok(Json(json!({
        "requests": 0,
        "cache": {},
        "performance": {},
        "runtime": "rust",
        "goals": goal_stats,
        "research_agents": research_stats,
        "quality": quality_stats,
        "auxiliary_ai": {
            "usage_stats": usage_stats,
            "contribution_stats": contribution_stats
        }
    })))
}

async fn goal_start_handler(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(request): Json<GoalStartRequest>,
) -> Result<Json<Value>, ApiError> {
    require_api_key(&headers)?;
    let root = state.root.clone();
    let manager = GoalManager::new(&root)?;
    let record = manager.start(
        &request.goal,
        &request.workflow,
        &request.requirements,
        &request.acceptance,
        &request.verification,
        &request.assumptions,
    )?;
    let goal_id = record.id.clone();
    Ok(Json(append_evidence_summary(
        serde_json::to_value(record)?,
        root,
        "goal",
        Some(goal_id),
        false,
    )?))
}

async fn goal_clarify_handler(
    headers: HeaderMap,
    Json(request): Json<GoalClarificationRequest>,
) -> Result<Json<Value>, ApiError> {
    require_api_key(&headers)?;
    let clarification = build_goal_clarification(
        &request.goal,
        &request.requirements,
        &request.acceptance,
        &request.verification,
        &request.assumptions,
    );
    Ok(Json(serde_json::to_value(clarification)?))
}

async fn goal_get_handler(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    require_api_key(&headers)?;
    let root = state.root.clone();
    let value = GoalManager::new(&root)?
        .status(Some(&id))
        .map_err(goal_api_error)?;
    Ok(Json(append_evidence_summary(
        value,
        root,
        "goal",
        Some(id),
        false,
    )?))
}

async fn goal_run_handler(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(request): Json<GoalRunRequest>,
) -> Result<Json<Value>, ApiError> {
    require_api_key(&headers)?;
    let evidence_root = state.root.clone();
    let root = state.root.clone();
    let evidence_goal_id = id.clone();
    let value = blocking_goal_call(move || {
        let manager = GoalManager::new(&root)?;
        if let Some(validation) = request.validation.as_deref() {
            let passed = match validation {
                "passed" | "success" => true,
                "failed" | "failure" => false,
                other => bail!("validation 必须是 passed 或 failed: {other}"),
            };
            return manager
                .record_validation_result(
                    Some(&id),
                    passed,
                    request
                        .message
                        .as_deref()
                        .unwrap_or("validation result recorded"),
                )
                .and_then(goal_run_record_response_value);
        }
        let mut agent = AgentManager::new(root, None, None, None, false).ok();
        let options = api_goal_run_options(&request)?;
        let report = if request.resume {
            manager.resume(Some(&id), agent.as_mut(), options)?
        } else {
            manager.run_layered(Some(&id), agent.as_mut(), options)?
        };
        goal_run_report_response_value(report)
    })
    .await?;
    Ok(Json(append_evidence_summary(
        value,
        evidence_root,
        "goal",
        Some(evidence_goal_id),
        false,
    )?))
}

async fn goal_close_handler(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(request): Json<GoalCloseRequest>,
) -> Result<Json<Value>, ApiError> {
    require_api_key(&headers)?;
    let root = state.root.clone();
    let record = GoalManager::new(&root)?
        .close_goal(
            Some(&id),
            &request.status,
            request.message.as_deref().unwrap_or("goal completed"),
            &request.next_steps,
        )
        .map_err(goal_api_error)?;
    Ok(Json(append_evidence_summary(
        serde_json::to_value(record)?,
        root,
        "goal",
        Some(id),
        false,
    )?))
}

async fn research_start_handler(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(request): Json<ResearchStartRequest>,
) -> Result<Json<Value>, ApiError> {
    require_api_key(&headers)?;
    let root = state.root.clone();
    let session = ResearchManager::new(&root)?
        .start(&request.question, request.goal_id)
        .map_err(goal_api_error)?;
    Ok(Json(append_evidence_summary(
        serde_json::to_value(session)?,
        root,
        "research",
        None,
        true,
    )?))
}

async fn research_get_handler(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    require_api_key(&headers)?;
    let root = state.root.clone();
    let value = ResearchManager::new(&root)?
        .status(Some(&id))
        .map_err(goal_api_error)?;
    Ok(Json(append_evidence_summary(
        value, root, "research", None, true,
    )?))
}

async fn research_run_handler(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(request): Json<ResearchRunRequest>,
) -> Result<Json<Value>, ApiError> {
    require_api_key(&headers)?;
    validate_model_override(&request.model_type, &request.model_name)?;
    if let Some(route_mode) = request.route_mode.as_deref()
        && route_mode != "manual"
        && route_mode != "auto"
    {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "route_mode must be manual or auto.",
        ));
    }
    let evidence_root = state.root.clone();
    let root = state.root.clone();
    let explicit_routing_requested = request.model_type.is_some()
        || request.model_name.is_some()
        || request.route_mode.is_some()
        || request.no_fallback;
    let session = blocking_goal_call(move || {
        let manager = ResearchManager::new(&root)?;
        let agent = AgentManager::new(
            root,
            request.model_type,
            request.model_name,
            request.route_mode,
            request.no_fallback,
        );
        let mut agent = match agent {
            Ok(agent) => Some(agent),
            Err(error) if explicit_routing_requested => return Err(error),
            Err(_) => None,
        };
        manager.run_once(Some(&id), agent.as_mut())
    })
    .await?;
    Ok(Json(append_evidence_summary(
        serde_json::to_value(session)?,
        evidence_root,
        "research",
        None,
        true,
    )?))
}

async fn research_reconcile_handler(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    require_api_key(&headers)?;
    let root = state.root.clone();
    let session = ResearchManager::new(&root)?
        .reconcile(Some(&id))
        .map_err(goal_api_error)?;
    Ok(Json(append_evidence_summary(
        serde_json::to_value(session)?,
        root,
        "research",
        None,
        true,
    )?))
}

fn validate_model_override(
    model_type: &Option<String>,
    model_name: &Option<String>,
) -> Result<(), ApiError> {
    if model_type.is_some() != model_name.is_some() {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "model_type and model_name must be provided together.",
        ));
    }
    Ok(())
}

async fn blocking_model_call<T, F>(operation: F) -> Result<T, ApiError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T> + Send + 'static,
{
    tokio::task::spawn_blocking(operation)
        .await
        .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Internal server error."))?
        .map_err(ApiError::from)
}

async fn blocking_goal_call<T, F>(operation: F) -> Result<T, ApiError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T> + Send + 'static,
{
    tokio::task::spawn_blocking(operation)
        .await
        .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "Internal server error."))?
        .map_err(goal_api_error)
}

fn goal_api_error(error: anyhow::Error) -> ApiError {
    let detail = error.to_string();
    if detail.contains("validation 必须")
        || detail.contains("目标关闭状态必须")
        || detail.contains("goal run until 必须")
    {
        ApiError::new(StatusCode::BAD_REQUEST, detail)
    } else if detail.contains("无法读取目标状态") || detail.contains("没有 active/in_progress 目标")
    {
        ApiError::new(StatusCode::NOT_FOUND, "Goal not found.")
    } else {
        ApiError::from(error)
    }
}

fn project_api_error(error: anyhow::Error) -> ApiError {
    let detail = error.to_string();
    if detail.contains("changed path must be within workspace")
        || detail.contains("asset path escaped workspace")
    {
        ApiError::new(StatusCode::BAD_REQUEST, detail)
    } else {
        ApiError::from(error)
    }
}

fn enforce_review_gate(
    state: &ApiState,
    code: &str,
    reviewed_path: Option<&str>,
) -> Result<(), ApiError> {
    let reviewed_path = reviewed_path.map(|path| {
        let path = PathBuf::from(path);
        if path.is_absolute() {
            path
        } else {
            state.root.join(path)
        }
    });
    let manager = SessionManager::new(state.root.clone()).map_err(ApiError::from)?;
    let blockers = manager
        .review_blockers(Some(code), reviewed_path.as_deref())
        .map_err(ApiError::from)?;
    if blockers.is_empty() {
        return Ok(());
    }
    Err(ApiError::new(
        StatusCode::CONFLICT,
        format!(
            "Review blocked by pending or unfinished work: {}",
            format_review_blockers(&blockers)
        ),
    ))
}

fn format_review_blockers(blockers: &[Value]) -> String {
    blockers
        .iter()
        .map(|blocker| {
            let kind = blocker
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("blocker");
            if kind == "pending_work" {
                return format!(
                    "pending {} [{}]: {}",
                    blocker
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown"),
                    blocker
                        .get("status")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown"),
                    blocker.get("title").and_then(Value::as_str).unwrap_or("")
                );
            }
            let path = blocker
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or("<request>");
            let line = blocker.get("line").and_then(Value::as_u64).unwrap_or(0);
            let reason = blocker
                .get("reason")
                .and_then(Value::as_str)
                .unwrap_or("unfinished");
            format!("{path}:{line} {reason}")
        })
        .collect::<Vec<_>>()
        .join("; ")
}

#[cfg(test)]
mod tests;
