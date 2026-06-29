use anyhow::Result;
use rayman_core::models::AgentManager;
use rayman_core::stats::{AuxiliaryContributionStore, AuxiliaryUsageStore};
use serde_json::Value as JsonValue;

use crate::runtime::root;

pub(crate) const AUXILIARY_USAGE_VALUE_LABEL: &str = "辅助AI使用价值";
pub(crate) const AUXILIARY_CONTRIBUTION_LABEL: &str = "实现验证纠错贡献";
pub(crate) const NO_IMPLEMENTATION_VALIDATION_SAMPLE: &str = "暂无实现验证纠错样本";

pub(crate) fn print_project_contribution_footer() -> Result<()> {
    let root = root()?;
    let usage = AuxiliaryUsageStore::new(&root)?.report_without_round()?;
    let report = AuxiliaryContributionStore::new(root)?.report_without_round()?;
    eprintln!(
        "{AUXILIARY_USAGE_VALUE_LABEL}: 项目累计(辅助成功/辅助调用/主力AI调用)={}",
        format_usage_scope(usage.get("project_total"))
    );
    eprintln!(
        "{AUXILIARY_CONTRIBUTION_LABEL}: {}",
        format_contribution_scope(report.get("project_total"))
    );
    Ok(())
}

pub(crate) fn print_auxiliary_status(manager: &AgentManager) {
    let usage = manager.auxiliary_usage_json();
    print_auxiliary_value(&usage);
}

pub(crate) fn print_auxiliary_value(usage: &JsonValue) {
    println!("{}", auxiliary_status_line(usage));
    if let Some(stats) = usage.get("contribution_stats") {
        let round = stats.get("round");
        let total = stats.get("project_total");
        println!(
            "{AUXILIARY_CONTRIBUTION_LABEL}: 本轮={} 项目累计={}",
            format_contribution_scope(round),
            format_contribution_scope(total)
        );
    }
    for line in auxiliary_detail_lines(usage) {
        println!("{line}");
    }
}

pub(crate) fn auxiliary_status_line(usage: &JsonValue) -> String {
    let enabled = usage
        .get("enabled")
        .and_then(JsonValue::as_bool)
        .unwrap_or(false);
    let model = usage
        .get("model")
        .and_then(JsonValue::as_str)
        .unwrap_or("<none>");
    let status = usage
        .get("status")
        .and_then(JsonValue::as_str)
        .unwrap_or("not_used");
    format!("辅助AI: enabled={enabled} model={model} status={status}")
}

pub(crate) fn auxiliary_detail_lines(usage: &JsonValue) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(reason) = usage.get("skip_reason").and_then(JsonValue::as_str) {
        lines.push(format!("辅助AI跳过原因: {reason}"));
    }
    if let Some(error) = usage.get("error").and_then(JsonValue::as_str).or_else(|| {
        usage
            .get("attempt")
            .and_then(|attempt| attempt.get("error"))
            .and_then(JsonValue::as_str)
    }) {
        lines.push(format!("辅助AI错误: {error}"));
    }
    lines
}

pub(crate) fn format_contribution_scope(scope: Option<&JsonValue>) -> String {
    let scope = scope.unwrap_or(&JsonValue::Null);
    let productions = scope
        .get("production_count")
        .and_then(JsonValue::as_u64)
        .unwrap_or(0);
    let contributions = scope
        .get("contribution_count")
        .and_then(JsonValue::as_u64)
        .unwrap_or(0);
    if productions == 0 {
        return NO_IMPLEMENTATION_VALIDATION_SAMPLE.to_string();
    }
    let percentage = scope
        .get("contribution_percentage")
        .and_then(JsonValue::as_f64)
        .unwrap_or(0.0);
    format!("{contributions}/{productions} ({percentage:.1}%)")
}

pub(crate) fn format_usage_scope(scope: Option<&JsonValue>) -> String {
    let scope = scope.unwrap_or(&JsonValue::Null);
    let attempts = scope
        .get("attempt_count")
        .and_then(JsonValue::as_u64)
        .unwrap_or(0);
    let successes = scope
        .get("success_count")
        .and_then(JsonValue::as_u64)
        .unwrap_or(0);
    let failures = scope
        .get("failed_count")
        .and_then(JsonValue::as_u64)
        .unwrap_or(0);
    let skipped = scope
        .get("skipped_count")
        .and_then(JsonValue::as_u64)
        .unwrap_or(0);
    let main_ai = scope
        .get("main_ai_count")
        .and_then(JsonValue::as_u64)
        .unwrap_or(0);
    let calls = scope
        .get("call_count")
        .and_then(JsonValue::as_u64)
        .unwrap_or_else(|| successes.saturating_add(failures));
    let queued = scope
        .get("queued_count")
        .and_then(JsonValue::as_u64)
        .unwrap_or_else(|| attempts.saturating_sub(calls).saturating_sub(skipped));
    let percentage = scope
        .get("auxiliary_call_success_rate")
        .or_else(|| scope.get("auxiliary_success_rate"))
        .and_then(JsonValue::as_f64)
        .unwrap_or_else(|| {
            if calls == 0 {
                0.0
            } else {
                (successes as f64 / calls as f64) * 100.0
            }
        });
    let avg_ms = scope
        .get("avg_duration_ms")
        .and_then(JsonValue::as_f64)
        .unwrap_or(0.0);
    let provider_attempts = scope
        .get("provider_attempt_count")
        .and_then(JsonValue::as_u64)
        .unwrap_or(0);
    format!(
        "{successes}/{calls}/{main_ai} ({percentage:.1}%, records={attempts}, failed={failures}, skipped={skipped}, queued={queued}, avg_ms={avg_ms:.1}, provider_attempts={provider_attempts})"
    )
}

pub(crate) fn usage_detail_lines(scope: Option<&JsonValue>) -> Vec<String> {
    let scope = scope.unwrap_or(&JsonValue::Null);
    let mut lines = Vec::new();
    if let Some(detail) = format_count_map(scope.get("failure_kinds")) {
        lines.push(format!("失败分类: {detail}"));
    }
    if let Some(detail) = format_count_map(scope.get("skip_reasons")) {
        lines.push(format!("跳过原因: {detail}"));
    }
    let cost = scope
        .get("estimated_cost_usd")
        .and_then(JsonValue::as_f64)
        .unwrap_or(0.0);
    if cost > 0.0 {
        lines.push(format!("估算成本USD: {cost:.6}"));
    }
    lines
}

fn format_count_map(value: Option<&JsonValue>) -> Option<String> {
    let object = value.and_then(JsonValue::as_object)?;
    if object.is_empty() {
        return None;
    }
    Some(
        object
            .iter()
            .filter_map(|(key, value)| value.as_u64().map(|count| format!("{key}={count}")))
            .collect::<Vec<_>>()
            .join(", "),
    )
}
