use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use serde::Serialize;
use walkdir::WalkDir;

use crate::assets::{AssetRetirementManager, AssetRetirementReport};
use crate::auxiliary::AuxiliaryTaskStore;
use crate::feature_coverage;
use crate::quality::{
    PATTERN_REPEATED_VALUE_CENTRALIZATION, REPEATED_VALUE_CENTRALIZATION_RULE_TITLE,
};
use crate::subagent::SubagentLedgerManager;
use crate::temp::TempManager;
use crate::yaml::Value;
use crate::{docs, rayman_cli_install_target, rayman_cli_source_binary, read_text, sha256_file};

const SKILL_REQUIRED_SNIPPETS: &[&str] = &[
    "`rayman agent-skill sync` refreshes the installed `rayman` CLI binary",
    "If `rayman auxiliary` is unavailable but the canonical release binary exists, use the canonical release binary directly",
    "`rayman agent-skill status` must report Codex, Claude Code, and `rayman-cli`",
    "Missing validation/development tools are actionable work, not skip reasons",
    "Customer program delivery is incomplete until both debug and release builds compile",
    "Repeated customer-reported failures must be captured as workspace-local quality incidents",
    REPEATED_VALUE_CENTRALIZATION_RULE_TITLE,
    "Agent workflow changes that touch agent behavior, LLM security, dependency policy, release evidence, provenance-required release handoff, or regression observability must run `rayman eval run`, `cargo deny check`, `rayman security audit`, and `rayman release evidence`",
    "`rayman coverage status --check`, `rayman docs maintain --check`, `rayman audit`, stale context, pending work, active goals, unresolved asset retirement, incomplete customer project docs, unclean managed temp state, and manual/remote validation gaps block success",
    "Project Understanding Protocol",
    "cached summaries, Context Index records, remembered conclusions, and auxiliary AI output are navigation only",
    "Context OS state graph freshness",
    "Managed Temp Protocol",
    "workspace-local `.RaymanCodingSkill/tmp/`",
    "Asset Retirement Protocol",
    "Paper Claim Audit Protocol",
    "A string anchor that merely finds a word in a file is not sufficient proof",
    "Feature Preservation Protocol",
    "When context is too small or stale to prove a feature is unused, do not delete it from memory",
    "Obsolete assets default to retirement and deletion",
    "Compatibility or audit retention requires an explicit reason and expiry date",
    "rayman assets status",
    "Only explicit customer approval or `review --apply-prune` may write obsolete-asset pruning changes",
    "Pruning may remove only review-identified obsolete assets",
    "Codex host subagent ledger records spawned agent tasks, boundaries, results, and primary-agent review",
    "Unreviewed, unresolved, conflict, or overlapping subagent ledger entries block success",
];

const CLI_REQUIRED_SNIPPETS: &[&str] = &[
    "`rayman agent-skill sync` updates both host skill entries and the installed `rayman` CLI binary",
    "`rayman agent-skill status` reports `rayman-cli` as stale when the installed binary differs from the canonical release binary",
    "`rayman auxiliary status` prints queued/running/succeeded/failed/reconciled/conflict task state",
    "`rayman stats` prints persisted auxiliary AI usage totals",
    "`rayman quality gate --goal-id <id>` checks matched quality patterns and hard-blocks missing regression evidence",
    "`rayman session close --status success` fails when pending work, active goals, audit findings, asset blockers, unclean managed temp state, stale context, or review blockers remain",
    "For non-trivial project understanding, follow [Project understanding](PROJECT_UNDERSTANDING.md)",
    "`rayman assets status` reports workspace-local obsolete asset retirement state",
    "`rayman temp status` reports workspace-local managed temp runs",
    "`rayman regression run --profile auto|quick|full|shared-parallel-full|parallel-full` executes repository regression gates and appends an immutable run record to `.RaymanCodingSkill/regression/history.jsonl`",
    "`rayman eval run --profile core|full` executes deterministic agent-behavior contracts",
    "`rayman security audit` checks LLM-specific security controls",
    "`rayman release evidence --label <label>` writes a local unsigned evidence bundle",
    "`rayman subagent status` reports Codex host subagent ledger blockers",
    "`rayman subagent record` creates a bounded host-subagent ledger entry",
    "`rayman context os --write` derives `.RaymanCodingSkill/context/state.json`",
];

const PROJECT_UNDERSTANDING_REQUIRED_SNIPPETS: &[&str] = &[
    "fresh, workspace-local, hash-backed context",
    "rayman context status",
    "Reread the referenced current source",
    "obsolete, retired, or compatibility-exempt assets are not current-behavior evidence",
    "They get their own workspace-local Context OS state under `.RaymanCodingSkill/context/`",
];

const QUALITY_REQUIRED_SNIPPETS: &[&str] = &[
    ".RaymanCodingSkill/quality/incidents/*.json",
    "case_to_general_rule",
    "project_understanding_freshness",
    "obsolete_asset_retirement",
    "audit_failure_delivery_gate",
    PATTERN_REPEATED_VALUE_CENTRALIZATION,
    "agent_eval_security_provenance",
    "The gate is a hard gate",
];

const YAML_CONFIG_REQUIRED_SNIPPETS: &[&str] = &[
    "Legacy single-provider fields `auxiliary_ai.provider` and `auxiliary_ai.model` are upgraded on load",
    "`auxiliary_ai.default_timeout` defaults to 120 seconds",
    "Proxy defaults to direct when `models.<provider>.proxy` is absent",
    "The canonical `ai_ubuntu_8888` auxiliary provider must set `proxy.mode: env`",
    "must not rely on cross-project memory or cached summaries as completion evidence",
    "`runtime_temp.root` defaults to `.RaymanCodingSkill/tmp`",
    "governance/reference metadata",
    "`allow_network` is loaded as policy metadata",
    "Plaintext YAML `api_key` values are treated as LLM security blockers",
];

const API_REQUIRED_SNIPPETS: &[&str] = &[
    "`source_policy`, `understanding_protocol`, and `required_actions` fields",
    "These fields are guidance, not proof of completion",
    "GET /api/assets",
    "Asset retirement responses return",
];

const MODEL_ROUTING_REQUIRED_SNIPPETS: &[&str] = &[
    "The ordered `auxiliary_ai.providers` list is the provider order",
    "It sets `auth_required: false`, `proxy.mode: env`, and a 120-second timeout",
    "Worker reconciliation writes structured conclusions",
];

const TESTING_REQUIRED_SNIPPETS: &[&str] = &[
    "Feature coverage is the machine-checkable source of truth",
    "`test_anchors[].proves`",
    "rayman coverage status --check",
    "Agent skill tests cover synchronized host entries and CLI binary installation",
    "canonical AI-UBUNTU environment-proxy configuration",
    "quality incidents, quality pattern aggregation, hard quality gates",
    "Project-understanding tests cover additive context protocol fields",
    "Managed-temp tests cover workspace-local temp roots",
    "Obsolete-asset tests cover hash-backed retirement state",
    "Audit-delivery tests cover `session close --status success`",
    "Agent-eval tests cover deterministic quality-pattern contracts",
    "LLM-security tests cover plaintext YAML API key blocking",
    "Release/provenance tests cover local release evidence bundles",
    "Regression-history tests cover append-only JSONL records",
];

#[derive(Debug, Clone, Serialize)]
pub struct AuditFinding {
    pub path: PathBuf,
    pub line: usize,
    pub pattern: String,
    pub message: String,
}

pub fn audit_repository(root: &Path) -> Result<Vec<AuditFinding>> {
    let mut findings = Vec::new();
    audit_installed_cli_for_enabled_workspace(root, &mut findings);
    audit_asset_retirement(root, &mut findings)?;
    audit_auxiliary_tasks(root, &mut findings)?;
    audit_temp_cleanup(root, &mut findings)?;
    audit_subagent_ledger(root, &mut findings)?;
    audit_customer_project_docs(root, &mut findings)?;
    if is_canonical_rayman_repo(root) {
        audit_required_skill_text(root, &mut findings, SKILL_REQUIRED_SNIPPETS);
        audit_required_text_any(
            root,
            &mut findings,
            &[
                "docs/CLI.md",
                "references/cli-model-and-config.md",
                "references/cli-context-kernel.md",
                "references/cli-project-intelligence.md",
            ],
            CLI_REQUIRED_SNIPPETS,
        );
        audit_required_text(
            root,
            &mut findings,
            "docs/PROJECT_UNDERSTANDING.md",
            PROJECT_UNDERSTANDING_REQUIRED_SNIPPETS,
        );
        audit_required_text(
            root,
            &mut findings,
            "docs/QUALITY.md",
            QUALITY_REQUIRED_SNIPPETS,
        );
        audit_required_text(
            root,
            &mut findings,
            "docs/YAML_CONFIG.md",
            YAML_CONFIG_REQUIRED_SNIPPETS,
        );
        audit_required_text(root, &mut findings, "docs/API.md", API_REQUIRED_SNIPPETS);
        audit_required_text(
            root,
            &mut findings,
            "docs/MODEL_ROUTING.md",
            MODEL_ROUTING_REQUIRED_SNIPPETS,
        );
        audit_required_text(
            root,
            &mut findings,
            "docs/TESTING.md",
            TESTING_REQUIRED_SNIPPETS,
        );
        audit_required_text(root, &mut findings, "docs/README.md", &["Feature Coverage"]);
        audit_feature_coverage(root, &mut findings);
        audit_no_unmanaged_runtime_temp(root, &mut findings);
    }
    let forbidden = [
        (
            concat!("python ", "rayman.py"),
            "旧 CLI 入口不能留在规范文档中",
        ),
        (
            concat!("requirements", ".txt"),
            "旧依赖清单不能作为当前入口",
        ),
        (concat!("setup", ".py"), "旧打包入口不能作为当前入口"),
        (concat!("compile", "all"), "旧编译门禁不能作为当前验证"),
        (concat!("py", "test"), "旧测试门禁不能作为当前验证"),
        (concat!("Fast", "API"), "API 文档应描述 Rust axum 实现"),
        (concat!("uvi", "corn"), "API 文档应描述 rayman api serve"),
    ];
    for entry in WalkDir::new(root)
        .into_iter()
        .filter_map(|entry| entry.ok())
    {
        if !entry.file_type().is_file() || should_skip(entry.path()) {
            continue;
        }
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !is_governance_text_asset(path, name) {
            continue;
        }
        let Ok(text) = read_text(path) else {
            continue;
        };
        if name == "SKILL.md" {
            let line_count = docs::skill_main_line_count(&text);
            if line_count > docs::SKILL_MAIN_COMPACT_TRIGGER_LINES {
                findings.push(AuditFinding {
                    path: path.to_path_buf(),
                    line: docs::SKILL_MAIN_COMPACT_TRIGGER_LINES + 1,
                    pattern: "SKILL.md line budget".into(),
                    message: format!(
                        "主 skill 文件目标为 {} 行，整理触发线为 {} 行，当前 {} 行",
                        docs::SKILL_MAIN_TARGET_LINES,
                        docs::SKILL_MAIN_COMPACT_TRIGGER_LINES,
                        line_count
                    ),
                });
            }
        }
        for (line_index, line) in text.lines().enumerate() {
            for (pattern, message) in forbidden {
                if line.contains(pattern) {
                    findings.push(AuditFinding {
                        path: path.to_path_buf(),
                        line: line_index + 1,
                        pattern: pattern.to_string(),
                        message: message.to_string(),
                    });
                }
            }
            if line.trim_end().ends_with('`') && line.trim_start().starts_with("rayman ") {
                findings.push(AuditFinding {
                    path: path.to_path_buf(),
                    line: line_index + 1,
                    pattern: "PowerShell multiline backtick".into(),
                    message: "规范命令必须一行一个命令".into(),
                });
            }
            if line.contains("&& \\") {
                findings.push(AuditFinding {
                    path: path.to_path_buf(),
                    line: line_index + 1,
                    pattern: "shell continuation".into(),
                    message: "规范命令不能依赖 shell 续行或链式语法".into(),
                });
            }
        }
    }
    Ok(findings)
}

fn audit_asset_retirement(root: &Path, findings: &mut Vec<AuditFinding>) -> Result<()> {
    let manager = AssetRetirementManager::new(root)?;
    let report = manager.status()?;
    for blocker in report.blockers {
        findings.push(AuditFinding {
            path: manager.state_path().to_path_buf(),
            line: 1,
            pattern: "asset_retirement_blocker".into(),
            message: blocker,
        });
    }
    Ok(())
}

fn audit_auxiliary_tasks(root: &Path, findings: &mut Vec<AuditFinding>) -> Result<()> {
    let manager = AuxiliaryTaskStore::new(root)?;
    match manager.success_blockers() {
        Ok(blockers) => {
            for blocker in blockers {
                findings.push(AuditFinding {
                    path: root
                        .join(".RaymanCodingSkill")
                        .join("auxiliary")
                        .join("tasks"),
                    line: 1,
                    pattern: "auxiliary_task_blocker".into(),
                    message: blocker,
                });
            }
        }
        Err(error) => findings.push(AuditFinding {
            path: root
                .join(".RaymanCodingSkill")
                .join("auxiliary")
                .join("tasks"),
            line: 1,
            pattern: "auxiliary_task_blocker".into(),
            message: error.to_string(),
        }),
    }
    Ok(())
}

fn audit_temp_cleanup(root: &Path, findings: &mut Vec<AuditFinding>) -> Result<()> {
    let manager = TempManager::new(root)?;
    for blocker in manager.success_blockers()? {
        findings.push(AuditFinding {
            path: manager.root().to_path_buf(),
            line: 1,
            pattern: "managed_temp_cleanup_blocker".into(),
            message: blocker,
        });
    }
    Ok(())
}

fn audit_subagent_ledger(root: &Path, findings: &mut Vec<AuditFinding>) -> Result<()> {
    let manager = SubagentLedgerManager::new(root)?;
    for blocker in manager.success_blockers()? {
        findings.push(AuditFinding {
            path: manager.state_path().to_path_buf(),
            line: 1,
            pattern: "subagent_ledger_blocker".into(),
            message: blocker,
        });
    }
    Ok(())
}

fn audit_no_unmanaged_runtime_temp(root: &Path, findings: &mut Vec<AuditFinding>) {
    for entry in WalkDir::new(root)
        .into_iter()
        .filter_map(|entry| entry.ok())
    {
        if !entry.file_type().is_file() || should_skip(entry.path()) {
            continue;
        }
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
            continue;
        }
        let Ok(text) = read_text(path) else {
            continue;
        };
        let mut test_module_depth: Option<i32> = None;
        for (line_index, line) in text.lines().enumerate() {
            let trimmed = line.trim();
            let enters_test_module = test_module_depth.is_none()
                && (trimmed == "mod tests {"
                    || trimmed == "mod tests{"
                    || trimmed.starts_with("mod tests "));
            let in_test_module = test_module_depth.is_some() || enters_test_module;
            let forbidden = concat!("std::env::", "temp_dir()");
            if !in_test_module && line.contains(forbidden) {
                findings.push(AuditFinding {
                    path: path.to_path_buf(),
                    line: line_index + 1,
                    pattern: "unmanaged runtime temp".into(),
                    message:
                        "运行时代码必须使用 TempManager 或同目录 atomic temp，不能直接使用 std::env::temp_dir"
                            .into(),
                });
            }
            if in_test_module {
                let depth = test_module_depth.unwrap_or_default() + brace_depth_delta(line);
                test_module_depth = (depth > 0).then_some(depth);
            }
        }
    }
}

fn brace_depth_delta(line: &str) -> i32 {
    line.chars().fold(0, |depth, ch| match ch {
        '{' => depth + 1,
        '}' => depth - 1,
        _ => depth,
    })
}

pub fn assert_repository_clean(root: &Path) -> Result<()> {
    let findings = audit_repository(root)?;
    if findings.is_empty() {
        return Ok(());
    }
    let summary = format_findings_with_triage(root, &findings);
    bail!("仓库审计未通过:\n{summary}");
}

pub fn format_findings_with_triage(root: &Path, findings: &[AuditFinding]) -> String {
    findings
        .iter()
        .map(|finding| {
            let (triage, action) = finding_triage(root, finding);
            format!(
                "{}:{} {} - {} [triage={triage}; action={action}]",
                display_relative(root, &finding.path),
                finding.line,
                finding.pattern,
                finding.message
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn finding_triage(root: &Path, finding: &AuditFinding) -> (&'static str, &'static str) {
    if is_historical_or_archive_candidate(root, &finding.path) {
        return (
            "historical/archive candidate",
            "retire, archive-exempt with expiry, or register pending; do not ignore",
        );
    }
    if is_current_governance_surface(root, &finding.path)
        || finding.pattern == "asset_retirement_blocker"
        || finding.pattern == "auxiliary_task_blocker"
        || finding.pattern == "subagent_ledger_blocker"
        || finding.pattern == "customer docs incomplete"
        || finding.pattern == "required execution guarantee"
        || finding.pattern == "required file"
    {
        return ("current blocker", "fix or sync current docs/config/tests");
    }
    (
        "pre-existing blocker",
        "resolve, explicitly exempt, or register pending before success",
    )
}

fn is_historical_or_archive_candidate(root: &Path, path: &Path) -> bool {
    relative_components(root, path).any(|component| {
        matches!(
            component.as_str(),
            "archive"
                | "archives"
                | "archived"
                | "history"
                | "historical"
                | "legacy"
                | "deprecated"
                | "plans"
                | "plan"
        )
    })
}

fn is_current_governance_surface(root: &Path, path: &Path) -> bool {
    let relative = relative_components(root, path)
        .collect::<Vec<_>>()
        .join("/");
    matches!(
        relative.as_str(),
        "skill.md"
            | "readme.md"
            | "quickstart.md"
            | "docs/api.md"
            | "docs/cli.md"
            | "docs/goal_workflows.md"
            | "docs/model_routing.md"
            | "docs/project_understanding.md"
            | "docs/quality.md"
            | "docs/testing.md"
            | "docs/feature_coverage.md"
            | "docs/yaml_config.md"
    ) || relative.starts_with("config/")
}

fn relative_components<'a>(root: &'a Path, path: &'a Path) -> impl Iterator<Item = String> + 'a {
    path.strip_prefix(root)
        .unwrap_or(path)
        .components()
        .map(|component| component.as_os_str().to_string_lossy().to_lowercase())
}

fn display_relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn audit_required_text(
    root: &Path,
    findings: &mut Vec<AuditFinding>,
    relative_path: &str,
    required: &[&str],
) {
    let path = root.join(relative_path);
    let Ok(text) = read_text(&path) else {
        findings.push(AuditFinding {
            path,
            line: 1,
            pattern: "required file".into(),
            message: format!("缺少必需规范文件: {relative_path}"),
        });
        return;
    };
    for snippet in required {
        if !text.contains(snippet) {
            findings.push(AuditFinding {
                path: path.clone(),
                line: 1,
                pattern: "required execution guarantee".into(),
                message: format!("缺少执行保证文本: {snippet}"),
            });
        }
    }
}

fn audit_required_text_any(
    root: &Path,
    findings: &mut Vec<AuditFinding>,
    relative_paths: &[&str],
    required: &[&str],
) {
    let mut text = String::new();
    let mut found_any = false;
    for relative_path in relative_paths {
        let path = root.join(relative_path);
        if let Ok(file_text) = read_text(&path) {
            found_any = true;
            text.push('\n');
            text.push_str(&file_text);
        }
    }
    if !found_any {
        let relative_path = relative_paths.first().copied().unwrap_or("<none>");
        findings.push(AuditFinding {
            path: root.join(relative_path),
            line: 1,
            pattern: "required file".into(),
            message: format!("缺少必需规范文件: {relative_path}"),
        });
        return;
    }
    let report_path = root.join(relative_paths.first().copied().unwrap_or("<none>"));
    for snippet in required {
        if !text.contains(snippet) {
            findings.push(AuditFinding {
                path: report_path.clone(),
                line: 1,
                pattern: "required execution guarantee".into(),
                message: format!("缺少执行保证文本: {snippet}"),
            });
        }
    }
}

fn audit_required_skill_text(root: &Path, findings: &mut Vec<AuditFinding>, required: &[&str]) {
    let asset_retirement = current_behavior_asset_report(root, findings);
    if !asset_retirement
        .as_ref()
        .is_some_and(|report| report.is_current_behavior_path("SKILL.md"))
    {
        findings.push(AuditFinding {
            path: root.join("SKILL.md"),
            line: 1,
            pattern: "required execution guarantee".into(),
            message: "SKILL.md is not available as current-behavior skill text".into(),
        });
        return;
    }
    let path = root.join("SKILL.md");
    let Ok(mut text) = read_text(&path) else {
        findings.push(AuditFinding {
            path,
            line: 1,
            pattern: "required file".into(),
            message: "缺少必需规范文件: SKILL.md".into(),
        });
        return;
    };
    let references = root.join("references");
    if references.exists() {
        for entry in WalkDir::new(&references)
            .into_iter()
            .filter_map(|entry| entry.ok())
        {
            if !entry.file_type().is_file() {
                continue;
            }
            let relative = display_relative(root, entry.path());
            if !asset_retirement
                .as_ref()
                .is_some_and(|report| report.is_current_behavior_path(&relative))
            {
                continue;
            }
            if entry.path().extension().and_then(|ext| ext.to_str()) == Some("md")
                && let Ok(reference_text) = read_text(entry.path())
            {
                text.push('\n');
                text.push_str(&reference_text);
            }
        }
    }
    for snippet in required {
        if !text.contains(snippet) {
            findings.push(AuditFinding {
                path: root.join("SKILL.md"),
                line: 1,
                pattern: "required execution guarantee".into(),
                message: format!("缺少执行保证文本: {snippet}"),
            });
        }
    }
}

fn current_behavior_asset_report(
    root: &Path,
    findings: &mut Vec<AuditFinding>,
) -> Option<AssetRetirementReport> {
    match AssetRetirementManager::new(root).and_then(|manager| manager.status()) {
        Ok(report) => Some(report),
        Err(error) => {
            findings.push(AuditFinding {
                path: root
                    .join(".RaymanCodingSkill")
                    .join("assets")
                    .join("retirement.json"),
                line: 1,
                pattern: "asset_retirement_blocker".into(),
                message: format!(
                    "unable to load asset retirement state for current-behavior filtering: {error}"
                ),
            });
            None
        }
    }
}

fn audit_feature_coverage(root: &Path, findings: &mut Vec<AuditFinding>) {
    let Ok(report) = feature_coverage::check_feature_coverage_with_options(
        root,
        feature_coverage::FeatureCoverageOptions { strict: true },
    ) else {
        findings.push(AuditFinding {
            path: root.join(feature_coverage::FEATURE_COVERAGE_MANIFEST),
            line: 1,
            pattern: "feature coverage".into(),
            message: "无法读取或解析 feature coverage manifest".into(),
        });
        return;
    };
    for finding in report.findings {
        findings.push(AuditFinding {
            path: finding
                .path
                .unwrap_or_else(|| root.join(feature_coverage::FEATURE_COVERAGE_MANIFEST)),
            line: finding.line.max(1),
            pattern: format!("feature coverage {}", finding.kind),
            message: finding.message,
        });
    }
}

fn audit_customer_project_docs(root: &Path, findings: &mut Vec<AuditFinding>) -> Result<()> {
    let report = docs::check_customer_project_docs(root)?;
    if !report.checked || report.status == "current" {
        return Ok(());
    }
    let missing = if report.missing_topics.is_empty() {
        "generated customer documentation is stale".into()
    } else {
        report.missing_topics.join(", ")
    };
    findings.push(AuditFinding {
        path: report.managed_path,
        line: 1,
        pattern: "customer docs incomplete".into(),
        message: format!(
            "客户项目文档不完整或已过期: {missing}; 运行 rayman docs maintain 自动补全"
        ),
    });
    Ok(())
}

fn is_canonical_rayman_repo(root: &Path) -> bool {
    root.join("SKILL.md").exists()
        && root.join("crates").join("rayman-core").join("src").exists()
        && root.join("crates").join("rayman-cli").join("src").exists()
}

fn is_governance_text_asset(path: &Path, name: &str) -> bool {
    if name == "SKILL.md" || path.extension().and_then(|ext| ext.to_str()) == Some("md") {
        return true;
    }
    let under_config = path
        .components()
        .any(|component| component.as_os_str().to_string_lossy().as_ref() == "config");
    under_config
        && matches!(
            path.extension().and_then(|ext| ext.to_str()),
            Some("yaml" | "yml" | "toml" | "json")
        )
}

fn audit_installed_cli_for_enabled_workspace(root: &Path, findings: &mut Vec<AuditFinding>) {
    let state_path = root.join(".RaymanCodingSkill").join("workspace_skill.yaml");
    let Ok(text) = read_text(&state_path) else {
        return;
    };
    let Ok(state) = crate::yaml::from_str::<Value>(&text) else {
        return;
    };
    let enabled = state
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let skill = state
        .get("skill")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !enabled || skill != "raymancodingskill" {
        return;
    }
    let Some(canonical_root) = canonical_root_from_workspace_state(root, &state) else {
        return;
    };
    let Ok(source) = rayman_cli_source_binary(&canonical_root) else {
        return;
    };
    let Ok(target) = rayman_cli_install_target() else {
        return;
    };
    if !target.exists() {
        return;
    }
    let source_hash = sha256_file(&source).ok();
    let target_hash = sha256_file(&target).ok();
    if source_hash.is_some() && source_hash != target_hash {
        findings.push(AuditFinding {
            path: state_path,
            line: 1,
            pattern: "stale rayman-cli".into(),
            message: format!(
                "已安装 rayman CLI 落后于 canonical release；运行 rayman agent-skill sync: {}",
                target.display()
            ),
        });
    }
}

fn canonical_root_from_workspace_state(root: &Path, state: &Value) -> Option<PathBuf> {
    let skill_file = state.get("skill_file").and_then(Value::as_str)?;
    let skill_path = PathBuf::from(skill_file);
    let skill_root = skill_path.parent()?;
    if is_canonical_rayman_repo(skill_root) {
        return Some(skill_root.to_path_buf());
    }
    if is_canonical_rayman_repo(root) {
        return Some(root.to_path_buf());
    }
    let compiled = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)?
        .to_path_buf();
    is_canonical_rayman_repo(&compiled).then_some(compiled)
}

fn should_skip(path: &Path) -> bool {
    path.components().any(|component| {
        let value = component.as_os_str().to_string_lossy();
        [".git", "target", ".RaymanCodingSkill", "logs"].contains(&value.as_ref())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write_required_canonical_docs(root: &Path) -> Result<()> {
        fs::create_dir_all(root.join("docs"))?;
        fs::create_dir_all(root.join("crates").join("rayman-core").join("src"))?;
        fs::create_dir_all(root.join("crates").join("rayman-cli").join("src"))?;
        fs::write(root.join("SKILL.md"), SKILL_REQUIRED_SNIPPETS.join("\n"))?;
        fs::write(
            root.join("docs").join("CLI.md"),
            CLI_REQUIRED_SNIPPETS.join("\n"),
        )?;
        fs::write(
            root.join("docs").join("PROJECT_UNDERSTANDING.md"),
            PROJECT_UNDERSTANDING_REQUIRED_SNIPPETS.join("\n"),
        )?;
        fs::write(
            root.join("docs").join("QUALITY.md"),
            QUALITY_REQUIRED_SNIPPETS.join("\n"),
        )?;
        fs::write(
            root.join("docs").join("YAML_CONFIG.md"),
            YAML_CONFIG_REQUIRED_SNIPPETS.join("\n"),
        )?;
        fs::write(
            root.join("docs").join("API.md"),
            API_REQUIRED_SNIPPETS.join("\n"),
        )?;
        fs::write(
            root.join("docs").join("MODEL_ROUTING.md"),
            MODEL_ROUTING_REQUIRED_SNIPPETS.join("\n"),
        )?;
        fs::write(
            root.join("docs").join("TESTING.md"),
            TESTING_REQUIRED_SNIPPETS.join("\n"),
        )?;
        fs::write(root.join("docs").join("README.md"), "Feature Coverage\n")?;
        write_minimal_feature_coverage_repo(root)?;
        Ok(())
    }

    #[test]
    fn audit_fixtures_reuse_repeated_value_rule_constant() {
        let skill_fixture = SKILL_REQUIRED_SNIPPETS.join("\n");
        let previous_inline_rule_text = [
            "Repeated values that appear in multiple skill",
            "or program locations must be centralized",
        ]
        .join(" ");

        assert!(skill_fixture.contains(REPEATED_VALUE_CENTRALIZATION_RULE_TITLE));
        assert!(!skill_fixture.contains(&previous_inline_rule_text));
    }

    #[test]
    fn required_skill_text_ignores_compatibility_exempt_references() -> Result<()> {
        let temp = tempfile::tempdir()?;
        fs::create_dir_all(temp.path().join("references"))?;
        fs::write(temp.path().join("SKILL.md"), "# skill\n")?;
        let old_reference = temp.path().join("references").join("old-rule.md");
        fs::write(&old_reference, "stale-only-required-snippet\n")?;
        AssetRetirementManager::new(temp.path())?.exempt(crate::assets::AssetExemptRequest {
            path: old_reference,
            retention_reason: "temporary audit retention".into(),
            expires_at: "2999-01-01".into(),
        })?;

        let mut findings = Vec::new();
        audit_required_skill_text(temp.path(), &mut findings, &["stale-only-required-snippet"]);

        assert!(findings.iter().any(|finding| {
            finding.pattern == "required execution guarantee"
                && finding.message.contains("stale-only-required-snippet")
        }));
        Ok(())
    }

    #[test]
    fn required_cli_guarantees_can_live_in_split_reference_docs() -> Result<()> {
        let temp = tempfile::tempdir()?;
        fs::create_dir_all(temp.path().join("docs"))?;
        fs::create_dir_all(temp.path().join("references"))?;
        fs::write(
            temp.path().join("docs").join("CLI.md"),
            [
                "# CLI",
                "## Model And Config",
                "See [Model And Config](../references/cli-model-and-config.md) for the full rule text.",
                "## Context Kernel",
                "See [Context Kernel](../references/cli-context-kernel.md) for the full rule text.",
                "## Project Intelligence",
                "See [Project Intelligence](../references/cli-project-intelligence.md) for the full rule text.",
                CLI_REQUIRED_SNIPPETS[0],
                CLI_REQUIRED_SNIPPETS[1],
                CLI_REQUIRED_SNIPPETS[7],
                CLI_REQUIRED_SNIPPETS[8],
            ]
            .join("\n"),
        )?;
        fs::write(
            temp.path()
                .join("references")
                .join("cli-model-and-config.md"),
            [
                CLI_REQUIRED_SNIPPETS[2],
                CLI_REQUIRED_SNIPPETS[3],
                CLI_REQUIRED_SNIPPETS[4],
                CLI_REQUIRED_SNIPPETS[13],
                CLI_REQUIRED_SNIPPETS[14],
            ]
            .join("\n"),
        )?;
        fs::write(
            temp.path().join("references").join("cli-context-kernel.md"),
            [
                CLI_REQUIRED_SNIPPETS[5],
                CLI_REQUIRED_SNIPPETS[6],
                CLI_REQUIRED_SNIPPETS[15],
            ]
            .join("\n"),
        )?;
        fs::write(
            temp.path()
                .join("references")
                .join("cli-project-intelligence.md"),
            [
                CLI_REQUIRED_SNIPPETS[9],
                CLI_REQUIRED_SNIPPETS[10],
                CLI_REQUIRED_SNIPPETS[11],
                CLI_REQUIRED_SNIPPETS[12],
            ]
            .join("\n"),
        )?;

        let mut findings = Vec::new();
        audit_required_text_any(
            temp.path(),
            &mut findings,
            &[
                "docs/CLI.md",
                "references/cli-model-and-config.md",
                "references/cli-context-kernel.md",
                "references/cli-project-intelligence.md",
            ],
            CLI_REQUIRED_SNIPPETS,
        );

        assert!(findings.is_empty(), "{findings:?}");
        Ok(())
    }

    #[test]
    fn audit_requires_cli_sync_execution_guarantees() -> Result<()> {
        let temp = tempfile::tempdir()?;
        write_required_canonical_docs(temp.path())?;

        let findings = audit_repository(temp.path())?;
        assert!(
            findings.is_empty(),
            "{}",
            format_findings_with_triage(temp.path(), &findings)
        );
        Ok(())
    }

    #[test]
    fn audit_rejects_missing_cli_sync_execution_guarantees() -> Result<()> {
        let temp = tempfile::tempdir()?;
        fs::create_dir_all(temp.path().join("docs"))?;
        fs::create_dir_all(temp.path().join("crates").join("rayman-core").join("src"))?;
        fs::create_dir_all(temp.path().join("crates").join("rayman-cli").join("src"))?;
        fs::write(temp.path().join("SKILL.md"), "# skill")?;
        fs::write(temp.path().join("docs").join("CLI.md"), "# cli")?;
        fs::write(temp.path().join("docs").join("TESTING.md"), "# testing")?;

        let findings = audit_repository(temp.path())?;

        assert!(
            findings
                .iter()
                .any(|finding| finding.pattern == "required execution guarantee")
        );
        Ok(())
    }

    #[test]
    fn audit_rejects_old_command_entries_in_config_assets() -> Result<()> {
        let temp = tempfile::tempdir()?;
        fs::create_dir_all(temp.path().join("config"))?;
        fs::write(
            temp.path().join("config").join("skills.yaml"),
            "entrypoint: python rayman.py\n",
        )?;

        let findings = audit_repository(temp.path())?;

        assert!(findings.iter().any(|finding| {
            finding.pattern == "python rayman.py"
                && finding.message == "旧 CLI 入口不能留在规范文档中"
        }));
        Ok(())
    }

    #[test]
    fn audit_blocks_completed_managed_temp_until_cleaned() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let manager = TempManager::new(temp.path())?;
        let run = manager.run_dir("finished validation")?;
        run.complete()?;

        let findings = audit_repository(temp.path())?;
        assert!(findings.iter().any(|finding| {
            finding.pattern == "managed_temp_cleanup_blocker"
                && finding.message.contains("rayman temp cleanup --completed")
        }));

        manager.cleanup(&crate::temp::TempCleanupOptions {
            completed: true,
            stale: false,
            all_failed: false,
            cargo_targets: false,
        })?;

        let findings = audit_repository(temp.path())?;
        assert!(
            !findings
                .iter()
                .any(|finding| finding.pattern == "managed_temp_cleanup_blocker")
        );
        Ok(())
    }

    #[test]
    fn audit_blocks_unreviewed_subagent_ledger() -> Result<()> {
        let temp = tempfile::tempdir()?;
        crate::subagent::SubagentLedgerManager::new(temp.path())?.record(
            crate::subagent::SubagentRecordRequest {
                host_agent_id: "agent-1".into(),
                goal_id: None,
                dispatch_request_id: None,
                nickname: None,
                task: "inspect audit".into(),
                boundary: "read-only".into(),
                read_only: true,
                write_paths: Vec::new(),
            },
        )?;

        let findings = audit_repository(temp.path())?;

        assert!(findings.iter().any(|finding| {
            finding.pattern == "subagent_ledger_blocker"
                && finding.message.contains("subagent_ledger_unreviewed")
        }));
        Ok(())
    }

    #[test]
    fn audit_blocks_malformed_auxiliary_task_json() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let task_dir = temp
            .path()
            .join(".RaymanCodingSkill")
            .join("auxiliary")
            .join("tasks");
        fs::create_dir_all(&task_dir)?;
        fs::write(task_dir.join("bad.json"), "{not json")?;

        let findings = audit_repository(temp.path())?;

        assert!(findings.iter().any(|finding| {
            finding.pattern == "auxiliary_task_blocker"
                && finding.message.contains("auxiliary_task_parse_error")
        }));
        Ok(())
    }

    #[test]
    fn audit_blocks_all_managed_temp_cleanup_states() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let manager = TempManager::new(temp.path())?;
        let _active = manager.run_dir("active validation")?;
        let completed = manager.run_dir("completed validation")?;
        completed.complete()?;
        let stale = manager.run_dir("stale validation")?;
        stale.complete()?;
        make_temp_run_stale(&stale)?;
        let failed = manager.run_dir("failed validation")?;
        failed.fail()?;
        fs::create_dir_all(manager.root().join("runs").join("foreign"))?;

        let messages = audit_repository(temp.path())?
            .into_iter()
            .filter(|finding| finding.pattern == "managed_temp_cleanup_blocker")
            .map(|finding| finding.message)
            .collect::<Vec<_>>()
            .join("\n");

        assert!(messages.contains("managed_temp_active"));
        assert!(messages.contains("managed_temp_completed"));
        assert!(messages.contains("managed_temp_stale"));
        assert!(messages.contains("managed_temp_failed"));
        assert!(messages.contains("managed_temp_foreign"));
        Ok(())
    }

    #[test]
    fn audit_failure_summary_includes_triage_action() -> Result<()> {
        let temp = tempfile::tempdir()?;
        fs::create_dir_all(temp.path().join("docs").join("plans"))?;
        fs::write(
            temp.path().join("docs").join("plans").join("old.md"),
            "old validation used pytest\n",
        )?;

        let error = assert_repository_clean(temp.path()).unwrap_err();
        let text = error.to_string();

        assert!(text.contains("triage=historical/archive candidate"));
        assert!(text.contains("do not ignore"));
        Ok(())
    }

    #[test]
    fn audit_does_not_require_rayman_docs_in_customer_workspaces() -> Result<()> {
        let temp = tempfile::tempdir()?;
        fs::write(temp.path().join("SKILL.md"), "# customer skill")?;

        let findings = audit_repository(temp.path())?;

        assert!(findings.is_empty());
        Ok(())
    }

    #[test]
    fn audit_blocks_customer_code_project_until_docs_are_auto_completed() -> Result<()> {
        let temp = tempfile::tempdir()?;
        fs::write(
            temp.path().join("package.json"),
            r#"{"scripts":{"test":"vitest"}}"#,
        )?;
        fs::create_dir_all(temp.path().join("src"))?;
        fs::write(
            temp.path().join("src").join("index.ts"),
            "export function run() { return 1; }\n",
        )?;

        let findings = audit_repository(temp.path())?;
        assert!(findings.iter().any(|finding| {
            finding.pattern == "customer docs incomplete"
                && finding.message.contains("rayman docs maintain")
        }));

        docs::maintain_html_docs(docs::DocsMaintainOptions {
            root: temp.path().to_path_buf(),
            output: Some(temp.path().join("docs").join("project-docs.html")),
            prompt: None,
            prompt_file: None,
            model_output: None,
            dry_run: false,
            check: false,
            apply_prune: false,
        })?;

        let findings = audit_repository(temp.path())?;
        assert!(
            !findings
                .iter()
                .any(|finding| finding.pattern == "customer docs incomplete")
        );
        Ok(())
    }

    #[test]
    fn audit_rejects_unmanaged_runtime_temp_dir_in_canonical_source() -> Result<()> {
        let temp = tempfile::tempdir()?;
        write_required_canonical_docs(temp.path())?;
        fs::write(
            temp.path()
                .join("crates")
                .join("rayman-core")
                .join("src")
                .join("bad.rs"),
            "pub fn bad() { let _ = std::env::temp_dir(); }\n",
        )?;

        let findings = audit_repository(temp.path())?;

        assert!(
            findings
                .iter()
                .any(|finding| finding.pattern == "unmanaged runtime temp")
        );
        Ok(())
    }

    #[test]
    fn audit_feature_coverage_uses_strict_validation_records() -> Result<()> {
        let temp = tempfile::tempdir()?;
        write_strict_feature_coverage_repo(temp.path())?;
        let mut findings = Vec::new();

        audit_feature_coverage(temp.path(), &mut findings);

        assert!(
            findings.iter().any(|finding| {
                finding.pattern == "feature coverage claim_validation_records_missing"
                    && finding.message.contains("strict_audit_claim")
            }),
            "findings: {findings:#?}"
        );
        Ok(())
    }

    #[test]
    fn unmanaged_runtime_temp_scan_resumes_after_test_module() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let src = temp.path().join("src");
        fs::create_dir_all(&src)?;
        fs::write(
            src.join("lib.rs"),
            [
                "#[cfg(test)]",
                "mod tests {",
                "    fn allowed_in_test() { let _ = std::env::temp_dir(); }",
                "}",
                "pub fn production_after_tests() { let _ = std::env::temp_dir(); }",
            ]
            .join("\n"),
        )?;
        let mut findings = Vec::new();

        audit_no_unmanaged_runtime_temp(temp.path(), &mut findings);

        assert_eq!(
            findings
                .iter()
                .filter(|finding| finding.pattern == "unmanaged runtime temp")
                .count(),
            1
        );
        assert_eq!(findings[0].line, 5);
        Ok(())
    }

    #[test]
    fn customer_workspace_audit_checks_stale_installed_cli_without_requiring_docs() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let state_dir = temp.path().join(".RaymanCodingSkill");
        fs::create_dir_all(&state_dir)?;
        fs::write(
            state_dir.join("workspace_skill.yaml"),
            format!(
                "skill: raymancodingskill\nenabled: true\nskill_file: {}\n",
                temp.path().join("SKILL.md").display()
            ),
        )?;
        fs::write(temp.path().join("SKILL.md"), "# customer skill")?;

        let findings = audit_repository(temp.path())?;

        assert!(!findings.iter().any(|finding| {
            finding.pattern == "required file" || finding.pattern == "required execution guarantee"
        }));
        Ok(())
    }

    fn write_minimal_feature_coverage_repo(root: &Path) -> Result<()> {
        fs::create_dir_all(root.join("config"))?;
        fs::create_dir_all(root.join("docs"))?;
        fs::create_dir_all(root.join("crates").join("rayman-cli").join("tests"))?;
        fs::create_dir_all(root.join("crates").join("rayman-api").join("src"))?;
        fs::write(
            root.join("docs").join("README.md"),
            "# Documentation Index\n\nFeature Coverage\n",
        )?;
        fs::write(
            root.join("crates")
                .join("rayman-cli")
                .join("src")
                .join("main.rs"),
            "enum Command {}\n",
        )?;
        fs::write(
            root.join("crates")
                .join("rayman-core")
                .join("src")
                .join("audit.rs"),
            "fn audit_feature_coverage() {}\n",
        )?;
        fs::write(
            root.join("crates")
                .join("rayman-api")
                .join("src")
                .join("lib.rs"),
            "fn routes() { Router::new().route(\"/api/assets\", get(handler)); }\n",
        )?;
        fs::write(
            root.join("crates")
                .join("rayman-cli")
                .join("tests")
                .join("ui_contract.rs"),
            "// @ui:cli\n",
        )?;
        fs::write(
            root.join("config").join("feature_coverage.yaml"),
            r#"
features:
  - id: governance
    title: Governance
    doc_anchors:
      - path: SKILL.md
        contains: rayman agent-skill sync
      - path: docs/README.md
        contains: Feature Coverage
      - path: docs/CLI.md
        contains: rayman agent-skill sync
      - path: docs/PROJECT_UNDERSTANDING.md
        contains: fresh, workspace-local, hash-backed context
      - path: docs/QUALITY.md
        contains: case_to_general_rule
      - path: docs/YAML_CONFIG.md
        contains: auxiliary_ai.default_timeout
      - path: docs/API.md
        contains: GET /api/assets
      - path: docs/MODEL_ROUTING.md
        contains: auxiliary_ai.providers
      - path: docs/TESTING.md
        contains: Feature coverage is the machine-checkable source of truth
    implementation_anchors:
      - path: crates/rayman-cli/src/main.rs
        contains: enum Command
    test_anchors:
      - path: crates/rayman-cli/tests/ui_contract.rs
        contains: "@ui:cli"
        proves:
          - rayman agent-skill sync
          - rayman agent-skill status
          - rayman auxiliary status
          - rayman stats
          - rayman quality gate
          - rayman session close
          - rayman context os
          - rayman assets status
          - rayman temp status
          - rayman regression run
          - rayman eval run
          - rayman security audit
          - rayman release evidence
          - rayman subagent record
          - rayman subagent status
          - GET /api/assets
    validation_commands:
      - cargo test
    ui_surfaces:
      - cli
    public_commands:
      - rayman agent-skill sync
      - rayman agent-skill status
      - rayman auxiliary status
      - rayman stats
      - rayman quality gate
      - rayman session close
      - rayman context os
      - rayman assets status
      - rayman temp status
      - rayman regression run
      - rayman eval run
      - rayman security audit
      - rayman release evidence
      - rayman subagent record
      - rayman subagent status
    api_endpoints:
      - GET /api/assets
"#,
        )?;
        Ok(())
    }

    fn write_strict_feature_coverage_repo(root: &Path) -> Result<()> {
        fs::create_dir_all(root.join("config"))?;
        fs::create_dir_all(root.join("docs"))?;
        fs::create_dir_all(root.join("crates").join("rayman-cli").join("src"))?;
        fs::create_dir_all(root.join("crates").join("rayman-cli").join("tests"))?;
        fs::create_dir_all(root.join("crates").join("rayman-api").join("src"))?;
        fs::write(root.join("docs").join("CLI.md"), "# CLI\nrayman audit\n")?;
        fs::write(root.join("docs").join("API.md"), "# API\n")?;
        fs::write(
            root.join("crates")
                .join("rayman-cli")
                .join("src")
                .join("main.rs"),
            "enum Command {}\n",
        )?;
        fs::write(
            root.join("crates")
                .join("rayman-cli")
                .join("tests")
                .join("ui_contract.rs"),
            "// @ui:cli\nfn cli_help() {}\n",
        )?;
        fs::write(
            root.join("crates")
                .join("rayman-api")
                .join("src")
                .join("lib.rs"),
            "pub fn app() {}\n",
        )?;
        fs::write(
            root.join(feature_coverage::FEATURE_COVERAGE_MANIFEST),
            r##"
features:
  - id: audit
    title: Audit
    doc_anchors:
      - path: docs/CLI.md
        contains: "# CLI"
    implementation_anchors:
      - path: crates/rayman-cli/src/main.rs
        contains: enum Command
    test_anchors:
      - path: crates/rayman-cli/tests/ui_contract.rs
        contains: fn cli_help
        proves:
          - rayman audit
          - strict_audit_claim
    validation_commands:
      - "cargo test -p rayman-core audit::"
    ui_surfaces:
      - cli
    public_commands:
      - rayman audit
    claim_checks:
      - id: strict_audit_claim
        claim: Audit uses strict validation records.
        strict_validation: true
        implementation_anchors:
          - path: crates/rayman-cli/src/main.rs
            contains: enum Command
        test_anchors:
          - path: crates/rayman-cli/tests/ui_contract.rs
            contains: fn cli_help
            proves:
              - strict_audit_claim
        validation_commands:
          - "cargo test -p rayman-core audit::"
  - id: api
    title: API
    doc_anchors:
      - path: docs/API.md
        contains: "# API"
    implementation_anchors:
      - path: crates/rayman-api/src/lib.rs
        contains: pub fn app
    test_anchors:
      - path: crates/rayman-cli/tests/ui_contract.rs
        contains: "@ui:cli"
    validation_commands:
      - cargo test -p rayman-api
"##,
        )?;
        Ok(())
    }

    fn make_temp_run_stale(run: &crate::temp::TempRun) -> Result<()> {
        let text = fs::read_to_string(&run.metadata_path)?;
        let mut metadata: crate::temp::TempRunMetadata = serde_json::from_str(&text)?;
        let old = (chrono::Utc::now() - chrono::Duration::hours(48)).to_rfc3339();
        metadata.created_at = old.clone();
        metadata.updated_at = old;
        fs::write(
            &run.metadata_path,
            serde_json::to_string_pretty(&serde_json::json!(metadata))?,
        )?;
        Ok(())
    }
}
