use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeGenerationRequest {
    pub prompt: String,
    #[serde(default = "default_language")]
    pub language: String,
    pub model_type: Option<String>,
    pub model_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeReviewRequest {
    pub code: String,
    #[serde(default = "default_language")]
    pub language: String,
    pub model_type: Option<String>,
    pub model_name: Option<String>,
    pub workspace_path: Option<String>,
    pub reviewed_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestGenerationRequest {
    pub code: String,
    #[serde(default = "default_language")]
    pub language: String,
    #[serde(default = "default_test_types")]
    pub test_types: Vec<String>,
    pub model_type: Option<String>,
    pub model_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoalClarificationRequest {
    pub goal: String,
    #[serde(default)]
    pub requirements: Vec<String>,
    #[serde(default)]
    pub acceptance: Vec<String>,
    #[serde(default)]
    pub verification: Vec<String>,
    #[serde(default)]
    pub assumptions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoalStartRequest {
    pub goal: String,
    #[serde(default = "default_workflow")]
    pub workflow: String,
    #[serde(default)]
    pub requirements: Vec<String>,
    #[serde(default)]
    pub acceptance: Vec<String>,
    #[serde(default)]
    pub verification: Vec<String>,
    #[serde(default)]
    pub assumptions: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GoalRunRequest {
    pub validation: Option<String>,
    pub message: Option<String>,
    pub until: Option<String>,
    pub checkpoint_interval_minutes: Option<u64>,
    pub max_repair_attempts: Option<u32>,
    #[serde(default)]
    pub resume: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoalCloseRequest {
    pub status: String,
    pub message: Option<String>,
    #[serde(default)]
    pub next_steps: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResearchStartRequest {
    pub question: String,
    pub goal_id: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ResearchRunRequest {
    pub model_type: Option<String>,
    pub model_name: Option<String>,
    pub route_mode: Option<String>,
    #[serde(default)]
    pub no_fallback: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ImpactRequest {
    #[serde(default)]
    pub paths: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AssetRetireApiRequest {
    pub path: String,
    pub replacement: String,
    pub reason: String,
    pub validation_command: String,
    #[serde(default)]
    pub apply_delete: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AssetExemptApiRequest {
    pub path: String,
    pub reason: String,
    pub expires_at: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AssetCleanupApiRequest {
    #[serde(default)]
    pub apply: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeGenerationResponse {
    pub code: String,
    pub language: String,
    pub model: String,
    pub evidence_status: String,
    pub claim_ledger: Value,
    pub unknowns: Vec<String>,
    pub assumptions: Vec<String>,
    pub blockers: Vec<String>,
    pub auxiliary_ai: Value,
    pub implementation_validation: Option<Value>,
    pub edge_cases: Vec<Value>,
    pub logic_simulation: Value,
    pub potential_bugs: Vec<Value>,
    pub fixes_applied: Vec<Value>,
    pub validation_summary: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeReviewResponse {
    pub review: String,
    pub issues: Vec<Value>,
    pub suggestions: Vec<String>,
    pub score: Option<i64>,
    pub structured_fields_available: bool,
    pub evidence_status: String,
    pub claim_ledger: Value,
    pub unknowns: Vec<String>,
    pub assumptions: Vec<String>,
    pub blockers: Vec<String>,
    pub auxiliary_ai: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestGenerationResponse {
    pub test_code: String,
    pub test_count: usize,
    pub test_types: Vec<String>,
    pub evidence_status: String,
    pub claim_ledger: Value,
    pub unknowns: Vec<String>,
    pub assumptions: Vec<String>,
    pub blockers: Vec<String>,
    pub auxiliary_ai: Value,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UpdateQuery {
    #[serde(default)]
    pub force: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct EvidenceQuery {
    pub scope: Option<String>,
    pub goal_id: Option<String>,
    #[serde(default)]
    pub include_advisory: bool,
}

fn default_language() -> String {
    "rust".to_string()
}

fn default_workflow() -> String {
    "standard_development".to_string()
}

fn default_test_types() -> Vec<String> {
    ["positive", "negative", "boundary", "cross"]
        .into_iter()
        .map(str::to_string)
        .collect()
}
