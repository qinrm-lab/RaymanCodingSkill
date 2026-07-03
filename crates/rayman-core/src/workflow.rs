use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::evidence::{ClaimLedger, EvidenceResolver, EvidenceStatus};
use crate::models::AgentManager;
use crate::now_iso;
use crate::session::SessionManager;
use crate::skills;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomerRequirement {
    pub id: String,
    pub priority: String,
    pub text: String,
    pub status: String,
    pub evidence: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoalContract {
    pub goal: String,
    pub workflow_name: Option<String>,
    pub requirements: Vec<CustomerRequirement>,
    pub acceptance_criteria: Vec<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AtomicOperation {
    pub name: String,
    pub status: String,
    pub required: bool,
    pub output: Value,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionReport {
    pub contract: GoalContract,
    pub operations: Vec<AtomicOperation>,
    pub status: String,
    pub evidence_status: EvidenceStatus,
    pub claim_ledger: ClaimLedger,
    pub unknowns: Vec<String>,
    pub assumptions: Vec<String>,
    pub blockers: Vec<String>,
    pub pending_evidence: Vec<String>,
    pub summary: String,
    pub artifacts: Value,
    pub unfinished_requirements: Vec<Value>,
    pub next_steps: Vec<String>,
}

impl GoalContract {
    pub fn build(
        goal: &str,
        requirements: &[String],
        acceptance: &[String],
        workflow: Option<String>,
    ) -> Self {
        let requirements = if requirements.is_empty() {
            vec![goal.to_string()]
        } else {
            requirements.to_vec()
        };
        Self {
            goal: goal.to_string(),
            workflow_name: workflow,
            requirements: requirements
                .into_iter()
                .enumerate()
                .map(|(index, text)| CustomerRequirement {
                    id: format!("req_{}", index + 1),
                    priority: "must".into(),
                    text,
                    status: "pending".into(),
                    evidence: None,
                })
                .collect(),
            acceptance_criteria: acceptance.to_vec(),
            created_at: now_iso(),
        }
    }
}

pub fn run_workflow(
    manager: &mut AgentManager,
    workflow_name: &str,
    goal: &str,
    language: &str,
    source_code: Option<&str>,
    requirements: &[String],
    acceptance: &[String],
) -> Result<ExecutionReport> {
    let mut contract = GoalContract::build(
        goal,
        requirements,
        acceptance,
        Some(workflow_name.to_string()),
    );
    let blockers =
        SessionManager::new(manager.config.root.clone())?.review_blockers(source_code, None)?;
    if !blockers.is_empty() {
        let review_gate_prompt = format!(
            "Review gate blocked workflow {workflow_name}.\nGoal: {goal}\nBlockers: {}",
            serde_json::to_string(&blockers)?
        );
        let review_gate_advice =
            manager.auxiliary_advice(&review_gate_prompt, Some("review_gate"))?;
        let review_gate_auxiliary = manager.auxiliary_usage_json();
        return Ok(blocked_report(
            contract,
            workflow_name,
            blockers,
            review_gate_advice,
            review_gate_auxiliary,
        ));
    }
    let mut operations = Vec::new();
    let mut artifacts = json!({});
    let planning_prompt = format!(
        "Plan RaymanCodingSkill workflow {workflow_name} for this goal.\nLanguage: {language}\nGoal: {goal}"
    );
    let planning_advice = manager.auxiliary_advice(&planning_prompt, Some("planning"))?;
    let planning_auxiliary = manager.auxiliary_usage_json();
    artifacts["planning_auxiliary_advice"] =
        planning_advice.map(Value::String).unwrap_or(Value::Null);
    let result = match workflow_name {
        "standard_development" => {
            let code = skills::generate_code(manager, goal, language)?;
            let validation = skills::validate_and_fix(manager, &code, goal, language)?;
            artifacts["code"] = Value::String(validation.final_code.clone());
            artifacts["implementation_validation"] = serde_json::to_value(&validation)?;
            Value::String(validation.final_code)
        }
        "feature_update" => {
            let code = source_code.unwrap_or("");
            let review = skills::review_code(manager, code, language, None, None)?;
            artifacts["review"] = serde_json::to_value(&review)?;
            Value::String(review.review)
        }
        "documentation_update" => {
            let prompt = format!(
                "Synchronize documentation for this RaymanCodingSkill goal.\nLanguage: {language}\nGoal: {goal}\nSource context:\n{}",
                source_code.unwrap_or("")
            );
            let documentation = manager.complete(&prompt, Some("doc_sync"))?;
            artifacts["documentation"] = Value::String(documentation.clone());
            Value::String(documentation)
        }
        _ => {
            let code = skills::generate_code(manager, goal, language)?;
            artifacts["code"] = Value::String(code.clone());
            Value::String(code)
        }
    };
    operations.push(AtomicOperation {
        name: format!("{workflow_name}.execute"),
        status: "succeeded".into(),
        required: true,
        output: result,
        error: None,
    });
    let execution_auxiliary = manager.auxiliary_usage_json();
    let summary_prompt = format!(
        "Summarize RaymanCodingSkill workflow completion.\nWorkflow: {workflow_name}\nGoal: {goal}\nOperation output: {}",
        operations
            .last()
            .map(|operation| operation.output.to_string())
            .unwrap_or_default()
    );
    let summary_advice = manager.auxiliary_advice(&summary_prompt, Some("workflow_summary"))?;
    let summary_auxiliary = manager.auxiliary_usage_json();
    artifacts["workflow_summary_auxiliary_advice"] =
        summary_advice.map(Value::String).unwrap_or(Value::Null);
    let mut auxiliary_ai = summary_auxiliary.clone();
    if let Some(object) = auxiliary_ai.as_object_mut() {
        object.insert("planning".into(), planning_auxiliary);
        object.insert("execution".into(), execution_auxiliary);
        object.insert("summary".into(), summary_auxiliary);
    }
    artifacts["auxiliary_ai"] = auxiliary_ai;
    let resolver = EvidenceResolver::new(manager.config.root.clone())?;
    for requirement in &mut contract.requirements {
        requirement.status = "pending_evidence".into();
        requirement.evidence = None;
    }
    let claim_ledger = ClaimLedger::new(
        contract
            .requirements
            .iter()
            .map(|requirement| {
                resolver.claim_from_status(
                    requirement.id.clone(),
                    requirement.text.clone(),
                    &requirement.status,
                    requirement.evidence.as_deref(),
                )
            })
            .collect(),
    );
    let pending_evidence = contract
        .requirements
        .iter()
        .map(|requirement| {
            format!(
                "{}: attach current workspace path, successful validation command, or evidence artifact",
                requirement.id
            )
        })
        .collect::<Vec<_>>();
    let unknowns = claim_ledger.unknowns();
    let assumptions = claim_ledger.assumptions();
    let blockers = claim_ledger.blockers();
    Ok(ExecutionReport {
        contract,
        operations,
        status: "pending_evidence".into(),
        evidence_status: claim_ledger.status(),
        claim_ledger,
        unknowns,
        assumptions,
        blockers,
        pending_evidence: pending_evidence.clone(),
        summary: format!(
            "status=pending_evidence; operations succeeded=1, failed=0, skipped=0; failed_gates=0; unfinished_must={}",
            pending_evidence.len()
        ),
        artifacts,
        unfinished_requirements: pending_evidence
            .iter()
            .map(|item| json!({"type": "pending_evidence", "detail": item}))
            .collect(),
        next_steps: vec![
            "provide current file, validation command, or evidence artifact before claiming success"
                .into(),
        ],
    })
}

fn blocked_report(
    mut contract: GoalContract,
    workflow_name: &str,
    blockers: Vec<Value>,
    review_gate_advice: Option<String>,
    review_gate_auxiliary: Value,
) -> ExecutionReport {
    for requirement in &mut contract.requirements {
        requirement.status = "blocked".into();
        requirement.evidence = Some("pending or unfinished work blocked review gate".into());
    }
    let claim_ledger = ClaimLedger::new(
        contract
            .requirements
            .iter()
            .map(|requirement| crate::evidence::Claim {
                id: requirement.id.clone(),
                text: requirement.text.clone(),
                status: EvidenceStatus::Blocked,
                evidence_refs: Vec::new(),
                search_effort: Vec::new(),
                counterexample_challenges: Vec::new(),
                blockers: vec![
                    "pending or unfinished work blocked review gate; do not claim success".into(),
                ],
                checked_at: now_iso(),
            })
            .collect(),
    );
    let unknowns = claim_ledger.unknowns();
    let assumptions = claim_ledger.assumptions();
    let claim_blockers = claim_ledger.blockers();
    ExecutionReport {
        contract,
        operations: vec![AtomicOperation {
            name: format!("{workflow_name}.review_gate"),
            status: "failed".into(),
            required: true,
            output: json!({"blockers": blockers.clone()}),
            error: Some("pending or unfinished work blocked review gate".into()),
        }],
        status: "blocked".into(),
        evidence_status: EvidenceStatus::Blocked,
        claim_ledger,
        unknowns,
        assumptions,
        blockers: claim_blockers,
        pending_evidence: vec![
            "resolve review blockers before collecting completion evidence".into(),
        ],
        summary:
            "status=blocked; operations succeeded=0, failed=1, skipped=0; failed_gates=1; unfinished_must=1"
                .into(),
        artifacts: json!({
            "review_blockers": blockers,
            "review_gate_auxiliary_advice": review_gate_advice,
            "auxiliary_ai": {
                "review_gate": review_gate_auxiliary
            }
        }),
        unfinished_requirements: blockers,
        next_steps: vec!["complete or track pending and unfinished work before rerunning".into()],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::AgentManager;
    use crate::session::SessionManager;
    use std::fs;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn workflow_blocks_when_pending_work_exists() {
        let aux = openai_sequence_server(vec!["GATE"]);
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join("config")).unwrap();
        fs::write(
            temp.path().join("config").join("default_config.yaml"),
            format!(
                r#"config_files: {{}}
default_model:
  type: openai
  name: gpt-4
models:
  aux:
    adapter: openai_compatible
    auth_required: false
    base_url: "{aux}"
    timeout: 5
auxiliary_ai:
  enabled: true
  async: false
  provider: aux
  model: aux-model
  fail_open: true
  tasks:
    - review_gate
"#,
                aux = aux
            ),
        )
        .unwrap();
        SessionManager::new(temp.path())
            .unwrap()
            .add_pending("finish first", "details", "task", "test", "must", json!({}))
            .unwrap();
        let mut manager = AgentManager::new(temp.path(), None, None, None, false).unwrap();

        let report = run_workflow(
            &mut manager,
            "standard_development",
            "build feature",
            "rust",
            None,
            &[],
            &[],
        )
        .unwrap();

        assert_eq!(report.status, "blocked");
        assert_eq!(report.operations[0].status, "failed");
        assert_eq!(report.artifacts["review_gate_auxiliary_advice"], "GATE");
        assert_eq!(
            report.artifacts["auxiliary_ai"]["review_gate"]["task"].as_str(),
            Some("review_gate")
        );
    }

    #[test]
    fn standard_development_records_planning_auxiliary_evidence() {
        let aux =
            openai_sequence_server(vec!["PLAN", "CODE ADVICE", "VALIDATION ADVICE", "SUMMARY"]);
        let primary = openai_sequence_server(vec![
            "fn main() {}",
            r#"{"status":"passed","final_code":"fn main() {}","edge_cases":[],"logic_simulation":null,"potential_bugs":[],"fixes_applied":[],"validation_summary":"ok"}"#,
        ]);
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
  async: false
  provider: aux
  model: aux-model
  fail_open: true
  required_when_available: true
  tasks:
    - planning
    - code_generation
    - implementation_validation
    - workflow_summary
"#,
                primary = primary,
                aux = aux
            ),
        )
        .unwrap();
        let mut manager = AgentManager::new(temp.path(), None, None, None, false).unwrap();

        let report = run_workflow(
            &mut manager,
            "standard_development",
            "build feature",
            "rust",
            None,
            &[],
            &[],
        )
        .unwrap();

        assert_eq!(report.status, "pending_evidence");
        assert_eq!(report.evidence_status, EvidenceStatus::Unknown);
        assert!(!report.pending_evidence.is_empty());
        assert!(
            report
                .contract
                .requirements
                .iter()
                .all(|requirement| requirement.status == "pending_evidence")
        );
        assert_eq!(report.artifacts["planning_auxiliary_advice"], "PLAN");
        assert_eq!(
            report.artifacts["auxiliary_ai"]["planning"]["task"].as_str(),
            Some("planning")
        );
        assert_eq!(
            report.artifacts["auxiliary_ai"]["planning"]["status"].as_str(),
            Some("success")
        );
        assert_eq!(
            report.artifacts["auxiliary_ai"]["summary"]["task"].as_str(),
            Some("workflow_summary")
        );
        assert_eq!(
            report.artifacts["workflow_summary_auxiliary_advice"],
            "SUMMARY"
        );
    }

    #[test]
    fn documentation_update_uses_doc_sync_and_summary_auxiliary() {
        let aux = openai_sequence_server(vec!["PLAN", "DOC ADVICE", "SUMMARY"]);
        let primary = openai_sequence_server(vec!["Updated docs"]);
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
  async: false
  provider: aux
  model: aux-model
  fail_open: true
  tasks:
    - planning
    - doc_sync
    - workflow_summary
"#,
                primary = primary,
                aux = aux
            ),
        )
        .unwrap();
        let mut manager = AgentManager::new(temp.path(), None, None, None, false).unwrap();

        let report = run_workflow(
            &mut manager,
            "documentation_update",
            "update docs",
            "markdown",
            Some("source notes"),
            &[],
            &[],
        )
        .unwrap();

        assert_eq!(report.status, "pending_evidence");
        assert_eq!(report.evidence_status, EvidenceStatus::Unknown);
        assert_eq!(report.artifacts["documentation"], "Updated docs");
        assert_eq!(
            report.artifacts["auxiliary_ai"]["execution"]["task"].as_str(),
            Some("doc_sync")
        );
        assert_eq!(
            report.artifacts["auxiliary_ai"]["summary"]["task"].as_str(),
            Some("workflow_summary")
        );
    }

    fn openai_sequence_server(contents: Vec<&'static str>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        thread::spawn(move || {
            for content in contents {
                let Ok((mut stream, _)) = listener.accept() else {
                    break;
                };
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
}
