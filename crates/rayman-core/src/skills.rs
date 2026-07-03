use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::models::AgentManager;

const EVIDENCE_FIRST_PROMPT: &str = "Evidence-first unknown rule: current workspace files, successful command output, goal/session/context state, and existing evidence artifacts are the only proof sources. If proof is missing, say unknown or assumption instead of a plausible answer. Distinguish verified, unknown, assumption, blocked, and advisory. Auxiliary AI, cached summaries, memory, research output, and confidence are advisory only and cannot prove completion. When reporting success/completion/verified claims, include or preserve a claim ledger with evidence_refs, search_effort, counterexample_challenges, unknowns, assumptions, and blockers; do not mark verified/success until a counterexample or adversarial challenge is cleared with current evidence.";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationResult {
    pub status: String,
    pub evidence_status: String,
    pub claim_ledger: Value,
    pub unknowns: Vec<Value>,
    pub assumptions: Vec<Value>,
    pub blockers: Vec<Value>,
    pub final_code: String,
    pub edge_cases: Vec<Value>,
    pub logic_simulation: Value,
    pub potential_bugs: Vec<Value>,
    pub fixes_applied: Vec<Value>,
    pub validation_summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewResult {
    pub review: String,
    pub evidence_status: String,
    pub claim_ledger: Value,
    pub unknowns: Vec<Value>,
    pub assumptions: Vec<Value>,
    pub blockers: Vec<Value>,
    pub issues: Vec<Value>,
    pub suggestions: Vec<String>,
    pub score: Option<i64>,
    pub structured_fields_available: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestGenerationResult {
    pub test_code: String,
    pub test_count: usize,
    pub test_types: Vec<String>,
}

pub fn generate_code(manager: &mut AgentManager, prompt: &str, language: &str) -> Result<String> {
    let prompt = code_generation_prompt(prompt, language);
    let response = manager.complete(&prompt, Some("code_generation"))?;
    Ok(extract_code(&response).unwrap_or(response))
}

pub fn validate_and_fix(
    manager: &mut AgentManager,
    code: &str,
    requirement: &str,
    language: &str,
) -> Result<ValidationResult> {
    let prompt = implementation_validation_prompt(code, requirement, language);
    let response = manager.complete(&prompt, Some("implementation_validation"))?;
    let parsed = extract_json(&response).unwrap_or_else(|| json!({}));
    let final_code = parsed
        .get("final_code")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| extract_code(&response))
        .unwrap_or_else(|| code.to_string());
    let result = ValidationResult {
        status: parsed
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("validated")
            .to_string(),
        evidence_status: parsed
            .get("evidence_status")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string(),
        claim_ledger: parsed
            .get("claim_ledger")
            .cloned()
            .unwrap_or_else(|| json!({"claims": []})),
        unknowns: value_array(parsed.get("unknowns")),
        assumptions: value_array(parsed.get("assumptions")),
        blockers: value_array(parsed.get("blockers")),
        final_code,
        edge_cases: value_array(parsed.get("edge_cases")),
        logic_simulation: parsed
            .get("logic_simulation")
            .cloned()
            .unwrap_or(Value::Null),
        potential_bugs: value_array(parsed.get("potential_bugs")),
        fixes_applied: value_array(parsed.get("fixes_applied")),
        validation_summary: parsed
            .get("validation_summary")
            .or_else(|| parsed.get("summary"))
            .and_then(Value::as_str)
            .unwrap_or("implementation validation completed")
            .to_string(),
    };
    manager.record_auxiliary_validation_outcome(
        &result.status,
        code,
        &result.final_code,
        result.fixes_applied.len(),
    )?;
    Ok(result)
}

fn code_generation_prompt(prompt: &str, language: &str) -> String {
    format!(
        "You are RaymanCodingSkill. Generate production-quality {language} code for this request. Return only the code unless explanation is necessary.\n\n{}\n{}\n\nRequest:\n{prompt}",
        EVIDENCE_FIRST_PROMPT,
        chinese_text_and_path_requirements()
    )
}

fn implementation_validation_prompt(code: &str, requirement: &str, language: &str) -> String {
    format!(
        "Validate this {language} implementation against the requirement. Return JSON with keys status, evidence_status, claim_ledger, unknowns, assumptions, blockers, final_code, edge_cases, logic_simulation, potential_bugs, fixes_applied, validation_summary. The claim_ledger claims must include evidence_refs, search_effort, and counterexample_challenges for success/completion/verified claims. Do not set evidence_status=verified without current file, successful command output, evidence artifact proof, and a cleared counterexample/adversarial challenge.\n{}\n{}\nRequirement:\n{requirement}\n\nCode:\n{code}",
        EVIDENCE_FIRST_PROMPT,
        chinese_text_and_path_requirements()
    )
}

fn chinese_text_and_path_requirements() -> &'static str {
    "Chinese/CJK safety requirements: preserve UTF-8 for user-visible text, source comments, prompts, model output, filenames, directory names, JSON/YAML/HTML, logs, and generated artifacts. If the program prints to a terminal or console, account for Windows PowerShell/cmd and Unix-like UTF-8 consoles; use platform-native Unicode/UTF-8 handling when needed. Do not use byte length as a display-width, truncation, wrapping, or alignment proxy for Chinese/CJK text."
}

pub fn review_code(
    manager: &mut AgentManager,
    code: &str,
    language: &str,
    workspace_path: Option<&str>,
    reviewed_path: Option<&str>,
) -> Result<ReviewResult> {
    let prompt = format!(
        "Review this {language} code. Lead with bugs, risks, regressions, missing tests, and obsolete code that can be removed. Return JSON if practical with keys review, evidence_status, claim_ledger, unknowns, assumptions, blockers, issues, suggestions, score. Success/completion/verified findings in claim_ledger must include evidence_refs, search_effort, and counterexample_challenges. {}\nWorkspace: {}\nReviewed path: {}\n\nCode:\n{code}",
        EVIDENCE_FIRST_PROMPT,
        workspace_path.unwrap_or(""),
        reviewed_path.unwrap_or("")
    );
    let response = manager.complete(&prompt, Some("code_review"))?;
    let parsed = extract_json(&response);
    if let Some(parsed) = parsed {
        return Ok(ReviewResult {
            review: parsed
                .get("review")
                .and_then(Value::as_str)
                .unwrap_or(&response)
                .to_string(),
            evidence_status: parsed
                .get("evidence_status")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_string(),
            claim_ledger: parsed
                .get("claim_ledger")
                .cloned()
                .unwrap_or_else(|| json!({"claims": []})),
            unknowns: value_array(parsed.get("unknowns")),
            assumptions: value_array(parsed.get("assumptions")),
            blockers: value_array(parsed.get("blockers")),
            issues: value_array(parsed.get("issues")),
            suggestions: parsed
                .get("suggestions")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default(),
            score: parsed.get("score").and_then(Value::as_i64),
            structured_fields_available: true,
        });
    }
    Ok(ReviewResult {
        review: response,
        evidence_status: "unknown".into(),
        claim_ledger: json!({"claims": []}),
        unknowns: Vec::new(),
        assumptions: Vec::new(),
        blockers: Vec::new(),
        issues: Vec::new(),
        suggestions: Vec::new(),
        score: None,
        structured_fields_available: false,
    })
}

pub fn prune_obsolete_code(
    manager: &mut AgentManager,
    code: &str,
    language: &str,
    review: &str,
) -> Result<String> {
    let prompt = obsolete_asset_pruning_prompt(code, language, review);
    let response = manager.complete(&prompt, Some("obsolete_code_pruning"))?;
    Ok(extract_code(&response).unwrap_or(response))
}

fn obsolete_asset_pruning_prompt(code: &str, language: &str, review: &str) -> String {
    format!(
        "Remove only obsolete assets identified in this review from the supplied {language} file. Obsolete assets include inactive, unreachable, replaced, or duplicate code plus stale docs, config, tests, examples/fixtures, prompts/templates, scripts/tools, dependency-manifest entries, CLI/API references, generated-artifact references, and cache/temp/session references when they appear in this file. Preserve public behavior, active compatibility paths, comments that remain true, formatting where practical, and all unrelated content. Return the complete revised {language} file only. {EVIDENCE_FIRST_PROMPT}\n\nReview:\n{review}\n\nFile:\n{code}"
    )
}

pub fn generate_tests(
    manager: &mut AgentManager,
    code: &str,
    language: &str,
    test_types: &[String],
) -> Result<TestGenerationResult> {
    let prompt = format!(
        "Generate {language} tests for this code. Cover these test types: {}. Return only test code. {EVIDENCE_FIRST_PROMPT}\n\nCode:\n{code}",
        test_types.join(", ")
    );
    let response = manager.complete(&prompt, Some("test_generation"))?;
    let test_code = extract_code(&response).unwrap_or(response);
    let test_count = test_code
        .lines()
        .filter(|line| {
            let trimmed = line.trim_start();
            trimmed.starts_with("fn test_")
                || trimmed.starts_with("#[test]")
                || trimmed.starts_with("def test_")
                || trimmed.starts_with("it(")
        })
        .count()
        .max(1);
    Ok(TestGenerationResult {
        test_code,
        test_count,
        test_types: test_types.to_vec(),
    })
}

pub fn refactor_code(
    manager: &mut AgentManager,
    code: &str,
    language: &str,
    goals: &str,
) -> Result<String> {
    let prompt = format!(
        "Refactor this {language} code for these goals: {goals}. Return only the refactored code. {EVIDENCE_FIRST_PROMPT}\n\nCode:\n{code}"
    );
    let response = manager.complete(&prompt, Some("code_refactor"))?;
    Ok(extract_code(&response).unwrap_or(response))
}

pub fn explain_code(
    manager: &mut AgentManager,
    code: &str,
    language: &str,
    detail_level: &str,
) -> Result<String> {
    let prompt = format!(
        "Explain this {language} code with {detail_level} detail. {EVIDENCE_FIRST_PROMPT}\n\nCode:\n{code}"
    );
    manager.complete(&prompt, Some("code_explain"))
}

fn value_array(value: Option<&Value>) -> Vec<Value> {
    value.and_then(Value::as_array).cloned().unwrap_or_default()
}

pub fn extract_json(text: &str) -> Option<Value> {
    if let Ok(value) = serde_json::from_str(text) {
        return Some(value);
    }
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    serde_json::from_str(&text[start..=end]).ok()
}

pub fn extract_code(text: &str) -> Option<String> {
    let mut in_block = false;
    let mut lines = Vec::new();
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            if in_block {
                return Some(lines.join("\n").trim().to_string());
            }
            in_block = true;
            continue;
        }
        if in_block {
            lines.push(line);
        }
    }
    None
}

pub fn validation_public_json(result: &ValidationResult) -> Result<Value> {
    serde_json::to_value(result).context("无法序列化验证结果")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_fenced_code() {
        let code = extract_code("text\n```rust\nfn main() {}\n```\n").unwrap();
        assert_eq!(code, "fn main() {}");
    }

    #[test]
    fn extracts_embedded_json() {
        let value = extract_json("prefix {\"status\":\"ok\"} suffix").unwrap();
        assert_eq!(value["status"], "ok");
    }

    #[test]
    fn code_generation_prompt_requires_chinese_text_and_paths() {
        let prompt = code_generation_prompt("打印中文并保存到 中文目录/output.json", "rust");

        assert!(prompt.contains("Chinese/CJK safety requirements"));
        assert!(prompt.contains("filenames, directory names"));
        assert!(prompt.contains("Windows PowerShell/cmd"));
        assert!(prompt.contains("Do not use byte length"));
        assert!(prompt.contains("Evidence-first unknown rule"));
        assert!(prompt.contains("claim ledger"));
        assert!(prompt.contains("counterexample_challenges"));
    }

    #[test]
    fn validation_prompt_checks_chinese_console_and_path_handling() {
        let prompt = implementation_validation_prompt(
            "fn main() { println!(\"中文\"); }",
            "支持中文目录名",
            "rust",
        );

        assert!(prompt.contains("preserve UTF-8"));
        assert!(prompt.contains("terminal or console"));
        assert!(prompt.contains("directory names"));
        assert!(prompt.contains("evidence_status"));
        assert!(prompt.contains("search_effort"));
        assert!(prompt.contains("counterexample_challenges"));
        assert!(prompt.contains("confidence"));
    }

    #[test]
    fn review_prompt_requires_counterexample_challenges() {
        let prompt = format!(
            "Review this rust code. Lead with bugs, risks, regressions, missing tests, and obsolete code that can be removed. Return JSON if practical with keys review, evidence_status, claim_ledger, unknowns, assumptions, blockers, issues, suggestions, score. Success/completion/verified findings in claim_ledger must include evidence_refs, search_effort, and counterexample_challenges. {}",
            EVIDENCE_FIRST_PROMPT
        );

        assert!(prompt.contains("counterexample_challenges"));
        assert!(prompt.contains("search_effort"));
    }

    #[test]
    fn validation_public_json_preserves_evidence_contract_fields() {
        let result = ValidationResult {
            status: "passed".into(),
            evidence_status: "verified".into(),
            claim_ledger: json!({
                "claims": [{
                    "id": "claim_1",
                    "text": "feature completed",
                    "search_effort": [],
                    "counterexample_challenges": []
                }]
            }),
            unknowns: vec![json!("unknown")],
            assumptions: vec![json!("assumption")],
            blockers: vec![json!("blocker")],
            final_code: "fn main() {}".into(),
            edge_cases: Vec::new(),
            logic_simulation: Value::Null,
            potential_bugs: Vec::new(),
            fixes_applied: Vec::new(),
            validation_summary: "ok".into(),
        };

        let value = validation_public_json(&result).unwrap();

        assert_eq!(value["evidence_status"], "verified");
        assert!(
            value["claim_ledger"]["claims"][0]
                .get("counterexample_challenges")
                .is_some()
        );
        assert_eq!(value["blockers"][0], "blocker");
    }

    #[test]
    fn obsolete_asset_pruning_prompt_is_review_scoped() {
        let prompt =
            obsolete_asset_pruning_prompt("old config entry", "yaml", "review identifies entry");

        assert!(prompt.contains("identified in this review"));
        assert!(prompt.contains("docs, config, tests"));
        assert!(prompt.contains("prompts/templates"));
        assert!(prompt.contains("dependency-manifest"));
        assert!(prompt.contains("Preserve public behavior"));
        assert!(prompt.contains("all unrelated content"));
    }
}
