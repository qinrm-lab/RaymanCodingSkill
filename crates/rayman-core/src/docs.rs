use std::collections::{BTreeSet, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use walkdir::WalkDir;

use crate::assets::{AssetRetirementManager, AssetRetirementReport};
use crate::project::{ProjectAnalyzer, ProjectIndex};
use crate::stats::AuxiliaryContributionStore;
use crate::temp::atomic_temp_path;
use crate::{display_path, ensure_within, now_iso, read_text, write_text};

pub const SKILL_RULE_TRIGGER_CHARS: usize = 20_000;
pub const SKILL_RULE_TARGET_CHARS: usize = 11_999;
pub const SKILL_RULE_MIN_REDUCTION_PERCENT: usize = 20;
pub const SKILL_MAIN_TARGET_LINES: usize = 100;
pub const SKILL_MAIN_COMPACT_TRIGGER_LINES: usize = 125;
pub const CUSTOMER_PROJECT_GUIDE_RELATIVE: &str = "docs/PROJECT_GUIDE.md";

const CUSTOMER_PROJECT_GUIDE_MARKER: &str =
    "<!-- rayman-generated-customer-project-guide: true -->";
const CUSTOMER_README_MARKER: &str = "<!-- rayman-generated-customer-readme: true -->";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillRuleSplitSummary {
    pub root: PathBuf,
    pub dry_run: bool,
    pub scanned_files: usize,
    pub split_files: usize,
    pub skipped_files: usize,
    pub reports: Vec<SkillRuleFileReport>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillRuleFileReport {
    pub path: PathBuf,
    pub original_chars: usize,
    pub final_chars: usize,
    pub action: String,
    pub references: Vec<PathBuf>,
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct DocsMaintainOptions {
    pub root: PathBuf,
    pub output: Option<PathBuf>,
    pub prompt: Option<String>,
    pub prompt_file: Option<PathBuf>,
    pub model_output: Option<PathBuf>,
    pub dry_run: bool,
    pub check: bool,
    pub apply_prune: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DocsMaintainReport {
    pub root: PathBuf,
    pub output: PathBuf,
    pub dry_run: bool,
    pub check: bool,
    pub status: String,
    pub updated: bool,
    pub stale: bool,
    pub generated_at: String,
    pub sections: Vec<String>,
    pub evidence_files: Vec<PathBuf>,
    pub developer_understanding_sources: Vec<PathBuf>,
    pub auxiliary_ai_contribution: Value,
    pub pruned_assets: Vec<PathBuf>,
    pub obsolete_asset_blockers: Vec<String>,
    pub required_actions: Vec<String>,
    pub asset_retirement: AssetRetirementReport,
    pub customer_docs: CustomerDocsReport,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct CustomerDocsReport {
    pub checked: bool,
    pub status: String,
    pub managed_path: PathBuf,
    pub readme_path: PathBuf,
    pub required_topics: Vec<String>,
    pub covered_topics: Vec<String>,
    pub missing_topics: Vec<String>,
    pub stale_files: Vec<PathBuf>,
    pub generated_files: Vec<PathBuf>,
    pub required_actions: Vec<String>,
}

struct DocsMaintenanceDraft {
    html: String,
    sections: Vec<String>,
    evidence_files: Vec<PathBuf>,
    developer_understanding_sources: Vec<PathBuf>,
}

pub fn compact_skill_rules(root: &Path, dry_run: bool) -> Result<SkillRuleSplitSummary> {
    let root = root
        .canonicalize()
        .with_context(|| format!("无法解析 skill 根目录: {}", root.display()))?;
    let files = skill_rule_markdown_files(&root)?;
    let mut split_files = 0usize;
    let mut skipped_files = 0usize;
    let mut reports = Vec::new();

    for path in files {
        let text =
            fs::read_to_string(&path).with_context(|| format!("无法读取: {}", path.display()))?;
        let original_chars = char_count(&text);
        if !skill_rule_needs_split(&path, &text) {
            skipped_files += 1;
            reports.push(SkillRuleFileReport {
                path,
                original_chars,
                final_chars: original_chars,
                action: "skipped".into(),
                references: Vec::new(),
                message: format!(
                    "at or below {SKILL_RULE_TRIGGER_CHARS} characters and {SKILL_MAIN_COMPACT_TRIGGER_LINES} line main SKILL.md trigger"
                ),
            });
            continue;
        }

        let plan = plan_lossless_skill_rule_split(&root, &path, &text)?;
        if !dry_run {
            apply_lossless_split(&path, &plan)?;
        }
        split_files += 1;
        reports.push(SkillRuleFileReport {
            path,
            original_chars,
            final_chars: char_count(&plan.source_text),
            action: if dry_run { "would_split" } else { "split" }.into(),
            references: plan
                .references
                .iter()
                .map(|reference| reference.path.clone())
                .collect(),
            message: "lossless split into reference files".into(),
        });
    }

    Ok(SkillRuleSplitSummary {
        root,
        dry_run,
        scanned_files: reports.len(),
        split_files,
        skipped_files,
        reports,
    })
}

pub fn maintain_html_docs(options: DocsMaintainOptions) -> Result<DocsMaintainReport> {
    let root = options
        .root
        .canonicalize()
        .with_context(|| format!("无法解析文档维护根目录: {}", options.root.display()))?;
    let output = match &options.output {
        Some(path) => ensure_within(path, &root, "docs maintain output escaped workspace")?,
        None => root.join("docs").join("project-docs.html"),
    };
    let prompt_text = read_optional_input(
        &root,
        options.prompt.as_deref(),
        options.prompt_file.as_ref(),
    )?;
    let model_output = read_optional_file(&root, options.model_output.as_ref())?;
    let customer_docs = maintain_customer_project_docs(&root, options.check, options.dry_run)?;
    let draft = render_docs_maintenance_html(
        &root,
        &output,
        prompt_text.as_deref(),
        model_output.as_deref(),
        &customer_docs,
    )?;
    let existing = output.exists().then(|| read_text(&output)).transpose()?;
    let stale = existing.as_deref() != Some(draft.html.as_str());
    let mut pruned_assets = Vec::new();
    if options.apply_prune && !options.dry_run && !options.check {
        pruned_assets = prune_stale_generated_docs(&root, &output)?;
    }
    if !options.dry_run && !options.check && stale {
        write_text(&output, &draft.html)?;
    }
    let asset_retirement = AssetRetirementManager::new(root.clone())?.scan()?;
    let auxiliary_ai_contribution = AuxiliaryContributionStore::new(root.clone())?
        .report_without_round()
        .unwrap_or(Value::Null);
    let obsolete_asset_blockers = asset_retirement.blockers.clone();
    let mut required_actions = Vec::new();
    if options.check && stale {
        required_actions.push(format!(
            "Regenerate HTML docs with rayman docs maintain --output {}",
            display_path(&output)
        ));
    }
    if options.check && customer_docs.status == "stale" {
        required_actions.extend(customer_docs.required_actions.clone());
    }
    if options.dry_run && stale {
        required_actions.push("Run without --dry-run to update the generated HTML docs.".into());
    }
    if options.dry_run && customer_docs.status == "would_update" {
        required_actions.extend(customer_docs.required_actions.clone());
    }
    if !obsolete_asset_blockers.is_empty() {
        required_actions.extend(asset_retirement.required_actions.clone());
    }
    if options.apply_prune && options.dry_run {
        required_actions
            .push("Run without --dry-run to prune stale Rayman-generated HTML docs.".into());
    }
    if required_actions.is_empty() {
        required_actions
            .push("Documentation is current and no obsolete asset blockers were reported.".into());
    }
    let status = if !obsolete_asset_blockers.is_empty() {
        "blocked"
    } else if options.check && (stale || customer_docs.status == "stale") {
        "stale"
    } else if options.dry_run && (stale || customer_docs.status == "would_update") {
        "would_update"
    } else {
        "current"
    };
    let reported_stale = if !options.dry_run && !options.check && obsolete_asset_blockers.is_empty()
    {
        false
    } else {
        stale
    };
    Ok(DocsMaintainReport {
        root,
        output,
        dry_run: options.dry_run,
        check: options.check,
        status: status.into(),
        updated: !options.dry_run && !options.check && stale,
        stale: reported_stale,
        generated_at: now_iso(),
        sections: draft.sections,
        evidence_files: draft.evidence_files,
        developer_understanding_sources: draft.developer_understanding_sources,
        auxiliary_ai_contribution,
        pruned_assets,
        obsolete_asset_blockers,
        required_actions,
        asset_retirement,
        customer_docs,
    })
}

pub fn check_customer_project_docs(root: &Path) -> Result<CustomerDocsReport> {
    let root = root
        .canonicalize()
        .with_context(|| format!("无法解析客户项目文档根目录: {}", root.display()))?;
    maintain_customer_project_docs(&root, true, false)
}

fn maintain_customer_project_docs(
    root: &Path,
    check: bool,
    dry_run: bool,
) -> Result<CustomerDocsReport> {
    let managed_path = root.join(CUSTOMER_PROJECT_GUIDE_RELATIVE);
    let readme_path = root.join("README.md");
    let skipped = |reason: &str| CustomerDocsReport {
        checked: false,
        status: "skipped".into(),
        managed_path: managed_path.clone(),
        readme_path: readme_path.clone(),
        required_topics: Vec::new(),
        covered_topics: Vec::new(),
        missing_topics: Vec::new(),
        stale_files: Vec::new(),
        generated_files: Vec::new(),
        required_actions: vec![reason.into()],
    };

    if is_canonical_rayman_repo_root(root) {
        return Ok(skipped(
            "Canonical RaymanCodingSkill docs use the repository feature-coverage gate instead of customer project auto-fill.",
        ));
    }

    let index = ProjectAnalyzer::new(root)?.index()?;
    if index.project_adapters.is_empty() {
        return Ok(skipped(
            "No supported customer source manifests or source files were detected.",
        ));
    }

    let required_topics = customer_required_doc_topics(&index);
    let desired_guide = render_customer_project_guide(root, &index, &required_topics)?;
    let desired_readme = (!readme_path.exists()).then(|| render_customer_readme(root));

    let mut covered = customer_doc_topic_coverage(root, &required_topics)?;
    let mut missing = required_topics
        .iter()
        .filter(|topic| !covered.contains(topic.as_str()))
        .cloned()
        .collect::<Vec<_>>();

    let guide_stale = if managed_path.exists() {
        read_text(&managed_path).unwrap_or_default() != desired_guide
    } else {
        !missing.is_empty()
    };
    let readme_stale = desired_readme.is_some();
    let mut stale_files = Vec::new();
    if guide_stale {
        stale_files.push(managed_path.clone());
    }
    if readme_stale {
        stale_files.push(readme_path.clone());
    }

    let mut generated_files = Vec::new();
    if !check && !dry_run {
        if guide_stale {
            write_text(&managed_path, &desired_guide)?;
            generated_files.push(managed_path.clone());
            for topic in &required_topics {
                covered.insert(topic.clone());
            }
            missing.clear();
        }
        if let Some(readme) = desired_readme {
            write_text(&readme_path, &readme)?;
            generated_files.push(readme_path.clone());
            covered.insert("overview".into());
        }
        stale_files.clear();
    }

    let mut covered_topics = covered.into_iter().collect::<Vec<_>>();
    covered_topics.retain(|topic| required_topics.contains(topic));
    let required_actions = if stale_files.is_empty() && missing.is_empty() {
        vec!["Customer project documentation is complete.".into()]
    } else if check {
        vec![format!(
            "Run rayman docs maintain to auto-complete missing customer project docs: {}",
            missing.join(", ")
        )]
    } else if dry_run {
        vec![format!(
            "Run rayman docs maintain without --dry-run to write customer docs: {}",
            missing.join(", ")
        )]
    } else {
        vec!["Customer project documentation was auto-completed.".into()]
    };
    let status = if !stale_files.is_empty() || !missing.is_empty() {
        if check {
            "stale"
        } else if dry_run {
            "would_update"
        } else {
            "current"
        }
    } else {
        "current"
    };

    Ok(CustomerDocsReport {
        checked: true,
        status: status.into(),
        managed_path,
        readme_path,
        required_topics,
        covered_topics,
        missing_topics: missing,
        stale_files,
        generated_files,
        required_actions,
    })
}

fn is_canonical_rayman_repo_root(root: &Path) -> bool {
    root.join("SKILL.md").exists()
        && root.join("crates").join("rayman-core").join("src").exists()
        && root.join("crates").join("rayman-cli").join("src").exists()
}

fn customer_required_doc_topics(_index: &ProjectIndex) -> Vec<String> {
    [
        "overview",
        "setup",
        "usage",
        "architecture",
        "configuration",
        "testing",
    ]
    .iter()
    .map(|topic| (*topic).to_string())
    .collect()
}

fn customer_doc_topic_coverage(
    root: &Path,
    required_topics: &[String],
) -> Result<BTreeSet<String>> {
    let mut covered = BTreeSet::new();
    for (relative, text) in collect_customer_markdown_docs(root)? {
        let haystack = format!(
            "{}\n{}",
            relative.to_ascii_lowercase(),
            text.to_ascii_lowercase()
        );
        for topic in required_topics {
            if customer_topic_keywords(topic)
                .iter()
                .any(|keyword| haystack.contains(keyword))
            {
                covered.insert(topic.clone());
            }
        }
    }
    Ok(covered)
}

fn collect_customer_markdown_docs(root: &Path) -> Result<Vec<(String, String)>> {
    let mut docs = Vec::new();
    for entry in WalkDir::new(root)
        .into_iter()
        .filter_map(|entry| entry.ok())
    {
        if !entry.file_type().is_file() || should_skip_customer_doc_file(entry.path(), root) {
            continue;
        }
        if entry.path().extension().and_then(|ext| ext.to_str()) != Some("md") {
            continue;
        }
        let relative = display_relative(root, entry.path())?;
        let text = fs::read_to_string(entry.path()).unwrap_or_default();
        docs.push((relative, text));
    }
    Ok(docs)
}

fn should_skip_customer_doc_file(path: &Path, root: &Path) -> bool {
    path.strip_prefix(root)
        .ok()
        .map(|relative| {
            relative.components().any(|component| {
                [".git", "target", ".RaymanCodingSkill", "logs", ".tmp"]
                    .contains(&component.as_os_str().to_string_lossy().as_ref())
            })
        })
        .unwrap_or(true)
}

fn customer_topic_keywords(topic: &str) -> &'static [&'static str] {
    match topic {
        "overview" => &[
            "overview",
            "introduction",
            "purpose",
            "readme",
            "概览",
            "简介",
        ],
        "setup" => &[
            "setup",
            "install",
            "installation",
            "quickstart",
            "getting started",
            "dependencies",
            "requirements",
            "安装",
            "快速开始",
            "依赖",
        ],
        "usage" => &[
            "usage", "run", "command", "api", "cli", "使用", "运行", "命令",
        ],
        "architecture" => &[
            "architecture",
            "design",
            "module",
            "structure",
            "entry point",
            "架构",
            "设计",
            "模块",
        ],
        "configuration" => &[
            "configuration",
            "config",
            "environment",
            "env",
            "settings",
            "配置",
            "环境变量",
        ],
        "testing" => &[
            "testing",
            "test",
            "validation",
            "verify",
            "quality",
            "测试",
            "验证",
        ],
        _ => &[],
    }
}

fn render_customer_readme(root: &Path) -> String {
    let name = root
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("Project");
    format!(
        "{CUSTOMER_README_MARKER}\n# {name}\n\nThis README was generated by RaymanCodingSkill because the project did not have a README. See [Project Guide](docs/PROJECT_GUIDE.md) for setup, usage, architecture, configuration, and testing documentation.\n"
    )
}

fn render_customer_project_guide(
    root: &Path,
    index: &ProjectIndex,
    required_topics: &[String],
) -> Result<String> {
    let languages = index
        .project_adapters
        .iter()
        .map(|adapter| adapter.language.clone())
        .collect::<BTreeSet<_>>();
    let manifests = index
        .project_adapters
        .iter()
        .flat_map(|adapter| {
            adapter
                .manifests
                .iter()
                .map(|manifest| manifest.path.clone())
        })
        .collect::<BTreeSet<_>>();
    let roots = index
        .project_adapters
        .iter()
        .flat_map(|adapter| adapter.project_roots.iter().cloned())
        .collect::<BTreeSet<_>>();
    let entry_points = index
        .language_indexes
        .iter()
        .flat_map(|language| language.entry_points.iter().map(|entry| entry.path.clone()))
        .collect::<BTreeSet<_>>();
    let verification = index
        .language_indexes
        .iter()
        .flat_map(|language| language.verification_commands.iter().cloned())
        .collect::<BTreeSet<_>>();
    let config_files = customer_config_files(root)?;

    let mut out = String::new();
    out.push_str(CUSTOMER_PROJECT_GUIDE_MARKER);
    out.push_str("\n# Project Guide\n\n");
    out.push_str("This Rayman-managed document fills customer project documentation gaps from current workspace evidence. Regenerate it with `rayman docs maintain`; use `rayman docs maintain --check` in gates.\n\n");
    out.push_str("## Overview\n\n");
    out.push_str(&format!(
        "- Detected languages: {}\n",
        comma_or_none(languages.iter())
    ));
    out.push_str(&format!(
        "- Project roots: {}\n",
        comma_or_none(roots.iter())
    ));
    out.push_str(&format!(
        "- Required documentation topics: {}\n\n",
        comma_or_none(required_topics.iter())
    ));

    out.push_str("## Setup\n\n");
    out.push_str("Install the project dependencies before running or validating changes.\n\n");
    push_markdown_list(&mut out, setup_commands(&languages));
    out.push('\n');

    out.push_str("## Usage\n\n");
    out.push_str(
        "Use the detected entry points and manifests as the first current-behavior map.\n\n",
    );
    out.push_str("### Entry Points\n\n");
    push_markdown_list(&mut out, entry_points.iter().cloned().collect::<Vec<_>>());
    out.push_str("\n### Manifests\n\n");
    push_markdown_list(&mut out, manifests.iter().cloned().collect::<Vec<_>>());
    out.push('\n');

    out.push_str("## Architecture\n\n");
    out.push_str("The current architecture summary is derived from source roots, entry points, public symbols, and import edges discovered by `rayman project index`.\n\n");
    out.push_str("### Source Roots\n\n");
    push_markdown_list(&mut out, roots.iter().cloned().collect::<Vec<_>>());
    out.push_str("\n### Public Symbols\n\n");
    push_markdown_list(&mut out, customer_public_symbols(index));
    out.push('\n');

    out.push_str("## Configuration\n\n");
    out.push_str("Configuration documentation must stay aligned with committed manifests, examples, and environment templates. Do not store real secrets in docs.\n\n");
    push_markdown_list(&mut out, config_files);
    out.push('\n');

    out.push_str("## Testing And Validation\n\n");
    out.push_str(
        "Run focused tests first, then the broad gates relevant to the detected languages.\n\n",
    );
    push_markdown_list(&mut out, verification.iter().cloned().collect::<Vec<_>>());
    out.push('\n');

    out.push_str("## Documentation Maintenance\n\n");
    out.push_str("- Run `rayman docs maintain` after behavior, setup, configuration, or validation commands change.\n");
    out.push_str(
        "- Run `rayman docs maintain --check` before reporting the customer project as complete.\n",
    );
    out.push_str("- Keep manual docs authoritative; this generated guide fills gaps without replacing hand-written files.\n");
    Ok(out)
}

fn customer_config_files(root: &Path) -> Result<Vec<String>> {
    let mut files = BTreeSet::new();
    for entry in WalkDir::new(root)
        .into_iter()
        .filter_map(|entry| entry.ok())
    {
        if !entry.file_type().is_file() || should_skip_customer_doc_file(entry.path(), root) {
            continue;
        }
        let path = entry.path();
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let ext = path
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if matches!(
            name.as_str(),
            ".env.example"
                | ".env.template"
                | "cargo.toml"
                | "package.json"
                | "tsconfig.json"
                | "pyproject.toml"
                | "go.mod"
        ) || matches!(
            ext.as_str(),
            "toml" | "yaml" | "yml" | "json" | "csproj" | "sln"
        ) {
            files.insert(display_relative(root, path)?);
        }
    }
    Ok(files.into_iter().collect())
}

fn setup_commands(languages: &BTreeSet<String>) -> Vec<String> {
    let mut commands = Vec::new();
    if languages.contains("rust") {
        commands.push("cargo fetch".into());
        commands.push("cargo build".into());
    }
    if languages.contains("javascript") || languages.contains("typescript") {
        commands.push("npm install".into());
    }
    if languages.contains("python") {
        commands.push("python -m pip install -e .".into());
    }
    if languages.contains("csharp") {
        commands.push("dotnet restore".into());
        commands.push("dotnet build".into());
    }
    if languages.contains("go") {
        commands.push("go mod download".into());
    }
    commands
}

fn customer_public_symbols(index: &ProjectIndex) -> Vec<String> {
    let mut symbols = index
        .language_indexes
        .iter()
        .flat_map(|language| {
            language
                .symbols
                .iter()
                .filter(|symbol| symbol.visibility == "public")
                .map(|symbol| {
                    format!(
                        "{} `{}` in {}:{}",
                        symbol.kind, symbol.name, symbol.path, symbol.line
                    )
                })
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .take(40)
        .collect::<Vec<_>>();
    if symbols.is_empty() {
        symbols.push("No public symbols were detected by the lightweight source index.".into());
    }
    symbols
}

fn comma_or_none<'a>(items: impl Iterator<Item = &'a String>) -> String {
    let values = items.cloned().collect::<Vec<_>>();
    if values.is_empty() {
        "none".into()
    } else {
        values.join(", ")
    }
}

fn push_markdown_list(out: &mut String, items: Vec<String>) {
    if items.is_empty() {
        out.push_str("- None detected yet.\n");
        return;
    }
    for item in items {
        out.push_str("- `");
        out.push_str(&item.replace('`', "'"));
        out.push_str("`\n");
    }
}

pub fn skill_main_line_count(text: &str) -> usize {
    text.lines().count()
}

pub fn assert_skill_main_line_budget(text: &str) -> Result<()> {
    let line_count = skill_main_line_count(text);
    if line_count > SKILL_MAIN_COMPACT_TRIGGER_LINES {
        bail!(
            "SKILL.md 超过整理触发线 {SKILL_MAIN_COMPACT_TRIGGER_LINES} 行: {line_count}; 目标行数为 {SKILL_MAIN_TARGET_LINES}"
        );
    }
    Ok(())
}

#[derive(Debug, Clone)]
struct MarkdownBlock {
    index: usize,
    movable: bool,
    heading_line: Option<String>,
    title: String,
    slug: String,
    text: String,
}

#[derive(Debug, Clone)]
struct ReferenceFile {
    path: PathBuf,
    text: String,
    block_index: usize,
}

#[derive(Debug, Clone)]
struct SplitPlan {
    source_text: String,
    references: Vec<ReferenceFile>,
}

fn plan_lossless_skill_rule_split(root: &Path, path: &Path, text: &str) -> Result<SplitPlan> {
    if !skill_rule_needs_split(path, text) {
        return Ok(SplitPlan {
            source_text: text.to_string(),
            references: Vec::new(),
        });
    }

    let newline = dominant_newline(text);
    let frontmatter_end = frontmatter_end(text);
    let frontmatter = text[..frontmatter_end].to_string();
    let blocks = markdown_blocks(text, frontmatter_end);
    if blocks.is_empty() {
        bail!("无法无损拆分没有 Markdown 块的文件: {}", path.display());
    }

    let mut used_targets = HashSet::new();
    let planned_targets = blocks
        .iter()
        .map(|block| unique_reference_path(root, path, block, &mut used_targets))
        .collect::<Result<Vec<_>>>()?;
    let mut moved = HashSet::new();
    let mut source_text = render_split_source(
        root,
        path,
        &frontmatter,
        &blocks,
        &planned_targets,
        &moved,
        newline,
    )?;

    while split_source_exceeds_target(path, &source_text) {
        let Some(block) = blocks
            .iter()
            .filter(|block| block.movable && !moved.contains(&block.index))
            .max_by_key(|block| char_count(&block.text))
        else {
            bail!(
                "无法将 {} 无损拆分到 {} 字符以下",
                path.display(),
                SKILL_RULE_TARGET_CHARS
            );
        };
        moved.insert(block.index);
        source_text = render_split_source(
            root,
            path,
            &frontmatter,
            &blocks,
            &planned_targets,
            &moved,
            newline,
        )?;
    }

    let source_rel = display_relative(root, path)?;
    let references = blocks
        .iter()
        .filter(|block| moved.contains(&block.index))
        .map(|block| ReferenceFile {
            path: planned_targets[block.index].clone(),
            text: reference_text(&source_rel, block, newline),
            block_index: block.index,
        })
        .collect::<Vec<_>>();

    verify_lossless_split(path, &blocks, &moved, &source_text, &references)?;
    Ok(SplitPlan {
        source_text,
        references,
    })
}

fn render_split_source(
    root: &Path,
    source_path: &Path,
    frontmatter: &str,
    blocks: &[MarkdownBlock],
    planned_targets: &[PathBuf],
    moved: &HashSet<usize>,
    newline: &str,
) -> Result<String> {
    let mut rendered = String::new();
    rendered.push_str(frontmatter);
    for block in blocks {
        if moved.contains(&block.index) {
            let link = relative_markdown_link(root, source_path, &planned_targets[block.index])?;
            rendered.push_str(&source_placeholder(block, &link, newline));
        } else {
            rendered.push_str(&block.text);
        }
    }
    Ok(rendered)
}

fn skill_rule_needs_split(path: &Path, text: &str) -> bool {
    char_count(text) > SKILL_RULE_TRIGGER_CHARS
        || (is_main_skill_file(path)
            && skill_main_line_count(text) > SKILL_MAIN_COMPACT_TRIGGER_LINES)
}

fn split_source_exceeds_target(path: &Path, text: &str) -> bool {
    char_count(text) >= SKILL_RULE_TARGET_CHARS
        || (is_main_skill_file(path) && skill_main_line_count(text) > SKILL_MAIN_TARGET_LINES)
}

fn is_main_skill_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case("SKILL.md"))
}

fn apply_lossless_split(source_path: &Path, plan: &SplitPlan) -> Result<()> {
    let source_temp = temp_path_for(source_path);
    write_temp_file(&source_temp, &plan.source_text)?;
    let mut reference_temps = Vec::new();
    for reference in &plan.references {
        let temp = temp_path_for(&reference.path);
        write_temp_file(&temp, &reference.text)?;
        reference_temps.push((reference.path.clone(), temp));
    }

    let mut created_references = Vec::new();
    for (final_path, temp) in &reference_temps {
        if final_path.exists() {
            cleanup_temps(&source_temp, &reference_temps);
            bail!("引用文件已存在，拒绝覆盖: {}", final_path.display());
        }
        fs::copy(temp, final_path)
            .with_context(|| format!("无法写入引用文件: {}", final_path.display()))?;
        created_references.push(final_path.clone());
    }

    if let Err(error) = fs::copy(&source_temp, source_path)
        .with_context(|| format!("无法写回拆分后的规则文件: {}", source_path.display()))
    {
        for created in created_references {
            let _ = fs::remove_file(created);
        }
        cleanup_temps(&source_temp, &reference_temps);
        return Err(error);
    }

    cleanup_temps(&source_temp, &reference_temps);
    Ok(())
}

fn verify_lossless_split(
    path: &Path,
    blocks: &[MarkdownBlock],
    moved: &HashSet<usize>,
    source_text: &str,
    references: &[ReferenceFile],
) -> Result<()> {
    if char_count(source_text) >= SKILL_RULE_TARGET_CHARS {
        bail!(
            "拆分后仍超过目标: {} chars in {}",
            char_count(source_text),
            path.display()
        );
    }
    if is_main_skill_file(path) && skill_main_line_count(source_text) > SKILL_MAIN_TARGET_LINES {
        bail!(
            "拆分后主 SKILL.md 仍超过目标行数 {SKILL_MAIN_TARGET_LINES}: {} lines in {}",
            skill_main_line_count(source_text),
            path.display()
        );
    }
    let original_chars: usize = blocks.iter().map(|block| char_count(&block.text)).sum();
    let final_chars = char_count(source_text);
    if final_chars * 100 > original_chars * (100 - SKILL_RULE_MIN_REDUCTION_PERCENT) {
        bail!(
            "拆分后主文件体量减少不足 {SKILL_RULE_MIN_REDUCTION_PERCENT}%: {}",
            path.display()
        );
    }
    if references.is_empty() {
        bail!("超过阈值但没有生成引用文件: {}", path.display());
    }
    for block in blocks.iter().filter(|block| moved.contains(&block.index)) {
        let Some(reference) = references
            .iter()
            .find(|reference| reference.block_index == block.index)
        else {
            bail!("缺少被移动块的引用文件: {}", block.title);
        };
        if !reference.text.contains(&block.text) {
            bail!("引用文件未逐字保留被移动内容: {}", block.title);
        }
    }
    Ok(())
}

fn markdown_blocks(text: &str, body_start: usize) -> Vec<MarkdownBlock> {
    let headings = markdown_headings(text, body_start);
    let group_level = headings
        .iter()
        .filter(|heading| heading.level >= 2)
        .map(|heading| heading.level)
        .min()
        .or_else(|| headings.iter().map(|heading| heading.level).min());
    let Some(group_level) = group_level else {
        return if body_start < text.len() && !text[body_start..].trim().is_empty() {
            vec![MarkdownBlock {
                index: 0,
                movable: true,
                heading_line: None,
                title: "Moved rule details".into(),
                slug: "moved-rule-details".into(),
                text: text[body_start..].to_string(),
            }]
        } else {
            Vec::new()
        };
    };

    let starts = headings
        .iter()
        .filter(|heading| heading.level == group_level)
        .map(|heading| heading.start)
        .collect::<Vec<_>>();

    let mut block_specs = Vec::new();
    if starts.is_empty() {
        if body_start < text.len() {
            block_specs.push((body_start, text.len(), true));
        }
    } else {
        if starts[0] > body_start {
            block_specs.push((body_start, starts[0], false));
        }
        for (index, start) in starts.iter().enumerate() {
            let end = starts.get(index + 1).copied().unwrap_or(text.len());
            block_specs.push((*start, end, true));
        }
    }

    block_specs
        .into_iter()
        .filter_map(|(start, end, movable)| {
            let block_text = text[start..end].to_string();
            if block_text.trim().is_empty() {
                return None;
            }
            let heading_line = first_markdown_heading_line(&block_text);
            let title = heading_line
                .as_ref()
                .map(|line| line.trim_start_matches('#').trim().to_string())
                .filter(|line| !line.is_empty())
                .unwrap_or_else(|| "Moved rule details".into());
            let index = 0;
            Some(MarkdownBlock {
                index,
                movable,
                heading_line,
                slug: slugify(&title),
                title,
                text: block_text,
            })
        })
        .enumerate()
        .map(|(index, mut block)| {
            block.index = index;
            block
        })
        .collect()
}

#[derive(Debug, Clone)]
struct MarkdownHeading {
    start: usize,
    level: usize,
}

fn markdown_headings(text: &str, body_start: usize) -> Vec<MarkdownHeading> {
    let mut headings = Vec::new();
    let mut in_fence = false;
    for (start, _end, line) in line_spans(&text[body_start..]) {
        if is_fence_line(line) {
            in_fence = !in_fence;
            continue;
        }
        if !in_fence && let Some(level) = markdown_heading_level(line) {
            headings.push(MarkdownHeading {
                start: body_start + start,
                level,
            });
        }
    }
    headings
}

fn first_markdown_heading_line(text: &str) -> Option<String> {
    let mut in_fence = false;
    for line in text.lines() {
        if is_fence_line(line) {
            in_fence = !in_fence;
            continue;
        }
        if !in_fence && markdown_heading_level(line).is_some() {
            return Some(trim_line_ending(line));
        }
    }
    None
}

fn skill_rule_markdown_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in WalkDir::new(root)
        .into_iter()
        .filter_map(|entry| entry.ok())
    {
        if !entry.file_type().is_file() || should_skip_markdown(entry.path()) {
            continue;
        }
        if matches!(
            entry
                .path()
                .extension()
                .and_then(|extension| extension.to_str()),
            Some("md")
        ) {
            files.push(entry.path().to_path_buf());
        }
    }
    files.sort();
    Ok(files)
}

fn should_skip_markdown(path: &Path) -> bool {
    if path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with(".compact.md"))
    {
        return true;
    }
    path.components().any(|component| {
        let value = component.as_os_str().to_string_lossy();
        [".git", "target", ".RaymanCodingSkill", "logs"].contains(&value.as_ref())
    })
}

fn unique_reference_path(
    root: &Path,
    source_path: &Path,
    block: &MarkdownBlock,
    used_targets: &mut HashSet<PathBuf>,
) -> Result<PathBuf> {
    let source_stem = source_path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .map(slugify)
        .filter(|stem| !stem.is_empty())
        .unwrap_or_else(|| "skill-rules".into());
    let reference_dir = if is_under_named_dir(source_path, "references") {
        source_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| root.join("references"))
    } else {
        root.join("references")
    };
    let mut suffix = 1usize;
    loop {
        let file_name = if suffix == 1 {
            format!("{source_stem}-{}.md", block.slug)
        } else {
            format!("{source_stem}-{}-{suffix}.md", block.slug)
        };
        let candidate = reference_dir.join(file_name);
        if !candidate.exists() && !used_targets.contains(&candidate) && candidate != source_path {
            used_targets.insert(candidate.clone());
            return Ok(candidate);
        }
        suffix += 1;
    }
}

fn source_placeholder(block: &MarkdownBlock, link: &str, newline: &str) -> String {
    let label = if block.title.is_empty() {
        "full moved rule text"
    } else {
        &block.title
    };
    if let Some(heading_line) = &block.heading_line {
        format!("{heading_line}{newline}See [{label}]({link}) for the full rule text.{newline}")
    } else {
        format!("See [{label}]({link}) for the moved rule text.{newline}")
    }
}

fn reference_text(source_rel: &str, block: &MarkdownBlock, newline: &str) -> String {
    format!(
        "# Extracted Skill Rules{newline}{newline}Source: `{source_rel}`{newline}{newline}{}",
        block.text
    )
}

fn frontmatter_end(text: &str) -> usize {
    let mut spans = line_spans(text).into_iter();
    let Some((_first_start, first_end, first_line)) = spans.next() else {
        return 0;
    };
    if trim_line_ending(first_line).trim() != "---" {
        return 0;
    }
    for (_start, end, line) in spans {
        if trim_line_ending(line).trim() == "---" {
            return end;
        }
    }
    first_end
}

fn line_spans(text: &str) -> Vec<(usize, usize, &str)> {
    let mut spans = Vec::new();
    let mut start = 0usize;
    for line in text.split_inclusive('\n') {
        let end = start + line.len();
        spans.push((start, end, line));
        start = end;
    }
    if start < text.len() {
        spans.push((start, text.len(), &text[start..]));
    }
    spans
}

fn markdown_heading_level(line: &str) -> Option<usize> {
    let trimmed = line.trim_start();
    let level = trimmed.chars().take_while(|char| *char == '#').count();
    if !(1..=6).contains(&level) {
        return None;
    }
    let rest = &trimmed[level..];
    if rest.is_empty() || rest.starts_with(char::is_whitespace) {
        Some(level)
    } else {
        None
    }
}

fn is_fence_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with("```") || trimmed.starts_with("~~~")
}

fn trim_line_ending(line: &str) -> String {
    line.trim_end_matches(['\r', '\n']).to_string()
}

fn dominant_newline(text: &str) -> &str {
    if text.contains("\r\n") { "\r\n" } else { "\n" }
}

fn char_count(text: &str) -> usize {
    text.chars().count()
}

fn slugify(value: &str) -> String {
    let mut slug = String::new();
    let mut last_dash = false;
    for char in value.chars() {
        if char.is_ascii_alphanumeric() {
            slug.push(char.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash {
            slug.push('-');
            last_dash = true;
        }
    }
    let slug = slug.trim_matches('-').to_string();
    if slug.is_empty() {
        "section".into()
    } else {
        slug
    }
}

fn is_under_named_dir(path: &Path, dir_name: &str) -> bool {
    path.components()
        .any(|component| component.as_os_str().to_string_lossy() == dir_name)
}

fn relative_markdown_link(root: &Path, source_path: &Path, target_path: &Path) -> Result<String> {
    let source_parent = source_path.parent().unwrap_or(root);
    let source_parent_rel = source_parent.strip_prefix(root).unwrap_or(source_parent);
    let target_rel = target_path.strip_prefix(root).unwrap_or(target_path);
    let mut parts = Vec::new();
    for component in source_parent_rel.components() {
        if matches!(component, std::path::Component::Normal(_)) {
            parts.push("..".to_string());
        }
    }
    for component in target_rel.components() {
        if let std::path::Component::Normal(value) = component {
            parts.push(value.to_string_lossy().to_string());
        }
    }
    if parts.is_empty() {
        bail!("无法生成相对链接: {}", target_path.display());
    }
    Ok(parts.join("/"))
}

fn display_relative(root: &Path, path: &Path) -> Result<String> {
    let relative = path.strip_prefix(root).unwrap_or(path);
    let parts = relative
        .components()
        .filter_map(|component| match component {
            std::path::Component::Normal(value) => Some(value.to_string_lossy().to_string()),
            _ => None,
        })
        .collect::<Vec<_>>();
    if parts.is_empty() {
        bail!("无法生成相对路径: {}", path.display());
    }
    Ok(parts.join("/"))
}

fn temp_path_for(path: &Path) -> PathBuf {
    atomic_temp_path(path, "docs-split")
}

fn write_temp_file(path: &Path, text: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("无法创建目录: {}", parent.display()))?;
    }
    fs::write(path, text).with_context(|| format!("无法写入临时文件: {}", path.display()))
}

fn cleanup_temps(source_temp: &Path, reference_temps: &[(PathBuf, PathBuf)]) {
    let _ = fs::remove_file(source_temp);
    for (_final_path, temp) in reference_temps {
        let _ = fs::remove_file(temp);
    }
}

fn render_docs_maintenance_html(
    root: &Path,
    output: &Path,
    prompt_text: Option<&str>,
    model_output: Option<&str>,
    customer_docs: &CustomerDocsReport,
) -> Result<DocsMaintenanceDraft> {
    let obsolete_assets = AssetRetirementManager::new(root.to_path_buf())?.scan()?;
    let evidence = collect_doc_evidence(root, output, &obsolete_assets)?;
    let boundaries =
        section_from_markdown(&evidence, "SKILL.md", "Boundaries").unwrap_or_else(|| {
            "RaymanCodingSkill is limited to programming workflows in the current workspace.".into()
        });
    let cli_manual = evidence
        .iter()
        .find(|file| file.relative == "docs/CLI.md")
        .map(|file| compact_markdown_text(&file.text, 9_000))
        .unwrap_or_else(|| "CLI documentation is not present in this workspace.".into());
    let developer_architecture = developer_architecture_summary(&evidence);
    let prompt_summary = prompt_summary(&evidence);
    let auxiliary_contribution = AuxiliaryContributionStore::new(root.to_path_buf())?
        .report_without_round()
        .unwrap_or(Value::Null);
    let mut sections = Vec::new();
    let mut html = String::new();
    html.push_str("<!doctype html>\n<html lang=\"en\">\n<head>\n");
    html.push_str("<meta charset=\"utf-8\">\n");
    html.push_str("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n");
    html.push_str("<meta name=\"rayman-generated-doc\" content=\"true\">\n");
    html.push_str("<title>Project Developer Documentation</title>\n");
    html.push_str("<style>");
    html.push_str("body{font-family:Segoe UI,Arial,sans-serif;line-height:1.55;margin:0;color:#202124;background:#f7f8fa}");
    html.push_str("main{max-width:1080px;margin:0 auto;padding:32px 24px 56px;background:white;min-height:100vh}");
    html.push_str("h1{font-size:30px;margin:0 0 8px}h2{font-size:22px;margin-top:32px;border-bottom:1px solid #d6d9de;padding-bottom:6px}");
    html.push_str("h3{font-size:17px;margin-top:20px}pre{white-space:pre-wrap;background:#f1f3f5;padding:14px;border-radius:6px;overflow:auto}");
    html.push_str("code{background:#eef1f4;padding:1px 4px;border-radius:4px}.meta{color:#5f6368}.grid{display:grid;grid-template-columns:repeat(auto-fit,minmax(240px,1fr));gap:12px}.box{border:1px solid #d6d9de;border-radius:6px;padding:12px;background:#fbfcfd}");
    html.push_str("ul{padding-left:22px}.warn{border-left:4px solid #b3261e;padding-left:12px}.ok{border-left:4px solid #137333;padding-left:12px}");
    html.push_str("</style>\n</head>\n<body>\n<main>\n");
    html.push_str("<h1>Project Developer Documentation</h1>\n");
    html.push_str(&format!(
        "<p class=\"meta\">Generated by RaymanCodingSkill from current workspace files. Output: <code>{}</code></p>\n",
        escape_html(&display_path(output))
    ));

    push_section(
        &mut html,
        &mut sections,
        "Project Overview",
        &project_overview(&evidence),
    );
    if customer_docs.checked {
        push_section(
            &mut html,
            &mut sections,
            "Customer Documentation Completeness",
            &customer_docs_section(customer_docs),
        );
    }
    push_section(
        &mut html,
        &mut sections,
        "Functional Boundaries",
        &markdown_to_html_fragment(&boundaries),
    );
    push_section(
        &mut html,
        &mut sections,
        "CLI Usage Manual",
        &markdown_to_html_fragment(&cli_manual),
    );
    push_section(
        &mut html,
        &mut sections,
        "Developer Architecture",
        &developer_architecture,
    );
    push_section(
        &mut html,
        &mut sections,
        "Prompt And Model Understanding",
        &prompt_summary,
    );
    push_section(
        &mut html,
        &mut sections,
        "Developer Understanding From Model Output",
        &developer_understanding_section(prompt_text, model_output),
    );
    push_section(
        &mut html,
        &mut sections,
        "Auxiliary AI Contribution",
        &auxiliary_contribution_section(&auxiliary_contribution),
    );
    push_section(
        &mut html,
        &mut sections,
        "Obsolete Asset Cleanup",
        &obsolete_asset_section(&obsolete_assets),
    );
    push_section(
        &mut html,
        &mut sections,
        "Evidence Files",
        &evidence_section(&evidence),
    );
    html.push_str("</main>\n</body>\n</html>\n");

    Ok(DocsMaintenanceDraft {
        html,
        sections,
        evidence_files: evidence.iter().map(|file| file.path.clone()).collect(),
        developer_understanding_sources: developer_sources(root, prompt_text, model_output),
    })
}

#[derive(Debug, Clone)]
struct DocEvidenceFile {
    relative: String,
    path: PathBuf,
    text: String,
}

fn collect_doc_evidence(
    root: &Path,
    output: &Path,
    asset_retirement: &AssetRetirementReport,
) -> Result<Vec<DocEvidenceFile>> {
    let mut files = Vec::new();
    for entry in WalkDir::new(root)
        .into_iter()
        .filter_map(|entry| entry.ok())
    {
        if !entry.file_type().is_file()
            || should_skip_docs_maintenance_file(entry.path(), root, output)
        {
            continue;
        }
        let relative = display_relative(root, entry.path())?;
        if !asset_retirement.is_current_behavior_path(&relative) {
            continue;
        }
        if is_docs_maintenance_evidence(&relative) {
            let text = fs::read_to_string(entry.path()).unwrap_or_default();
            files.push(DocEvidenceFile {
                relative,
                path: entry.path().to_path_buf(),
                text,
            });
        }
    }
    files.sort_by(|left, right| left.relative.cmp(&right.relative));
    Ok(files)
}

fn should_skip_docs_maintenance_file(path: &Path, root: &Path, output: &Path) -> bool {
    if path == output {
        return true;
    }
    if is_rayman_generated_html(path) {
        return true;
    }
    path.strip_prefix(root)
        .ok()
        .map(|relative| {
            relative.components().any(|component| {
                [".git", "target", ".RaymanCodingSkill", "logs", ".tmp"]
                    .contains(&component.as_os_str().to_string_lossy().as_ref())
            })
        })
        .unwrap_or(true)
}

fn is_rayman_generated_html(path: &Path) -> bool {
    if path.extension().and_then(|ext| ext.to_str()) != Some("html") {
        return false;
    }
    fs::read_to_string(path)
        .map(|text| text.contains("rayman-generated-doc"))
        .unwrap_or(false)
}

fn is_docs_maintenance_evidence(relative: &str) -> bool {
    let lower = relative.to_ascii_lowercase();
    lower == "readme.md"
        || lower == "skill.md"
        || lower == "quickstart.md"
        || lower == "cargo.toml"
        || lower.starts_with("docs/")
        || lower.starts_with("references/")
        || lower.starts_with("config/")
        || lower.starts_with("agents/")
        || lower.ends_with("cargo.toml")
        || lower.ends_with(".rs")
}

fn read_optional_input(
    root: &Path,
    prompt: Option<&str>,
    prompt_file: Option<&PathBuf>,
) -> Result<Option<String>> {
    let mut parts = Vec::new();
    if let Some(prompt) = prompt.filter(|value| !value.trim().is_empty()) {
        parts.push(prompt.to_string());
    }
    if let Some(path) = prompt_file {
        let path = ensure_within(path, root, "prompt file escaped workspace")?;
        parts.push(read_text(&path)?);
    }
    Ok((!parts.is_empty()).then(|| parts.join("\n\n")))
}

fn read_optional_file(root: &Path, path: Option<&PathBuf>) -> Result<Option<String>> {
    let Some(path) = path else {
        return Ok(None);
    };
    let path = ensure_within(path, root, "model output file escaped workspace")?;
    Ok(Some(read_text(&path)?))
}

fn push_section(html: &mut String, sections: &mut Vec<String>, title: &str, body: &str) {
    sections.push(title.to_string());
    html.push_str(&format!(
        "<section id=\"{}\">\n<h2>{}</h2>\n{}\n</section>\n",
        slugify(title),
        escape_html(title),
        body
    ));
}

fn project_overview(evidence: &[DocEvidenceFile]) -> String {
    let readme = evidence
        .iter()
        .find(|file| file.relative == "README.md")
        .map(|file| compact_markdown_text(&file.text, 2_000))
        .unwrap_or_else(|| "No README.md was found.".into());
    format!(
        "<div class=\"grid\"><div class=\"box\"><h3>Current Purpose</h3>{}</div><div class=\"box\"><h3>Evidence Count</h3><p>{} current files were scanned as documentation evidence.</p></div></div>",
        markdown_to_html_fragment(&readme),
        evidence.len()
    )
}

fn customer_docs_section(report: &CustomerDocsReport) -> String {
    let class = if report.status == "current" {
        "ok"
    } else {
        "warn"
    };
    let mut html = format!(
        "<div class=\"{}\"><p><strong>Status:</strong> {}</p><p><strong>Managed guide:</strong> <code>{}</code></p></div>",
        class,
        escape_html(&report.status),
        escape_html(&display_path(&report.managed_path))
    );
    html.push_str("<h3>Required Topics</h3><ul>");
    for topic in &report.required_topics {
        html.push_str(&format!("<li><code>{}</code></li>", escape_html(topic)));
    }
    html.push_str("</ul><h3>Covered Topics</h3><ul>");
    for topic in &report.covered_topics {
        html.push_str(&format!("<li><code>{}</code></li>", escape_html(topic)));
    }
    html.push_str("</ul>");
    if !report.missing_topics.is_empty() {
        html.push_str("<h3>Missing Topics</h3><ul>");
        for topic in &report.missing_topics {
            html.push_str(&format!("<li><code>{}</code></li>", escape_html(topic)));
        }
        html.push_str("</ul>");
    }
    html
}

fn developer_architecture_summary(evidence: &[DocEvidenceFile]) -> String {
    let modules = evidence
        .iter()
        .find(|file| file.relative == "crates/rayman-core/src/lib.rs")
        .map(|file| {
            file.text
                .lines()
                .filter_map(|line| line.trim().strip_prefix("pub mod "))
                .map(|value| value.trim_end_matches(';').to_string())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let crates = evidence
        .iter()
        .filter(|file| file.relative.ends_with("Cargo.toml"))
        .map(|file| file.relative.clone())
        .collect::<Vec<_>>();
    let mut html = String::new();
    html.push_str("<h3>Crates And Manifests</h3>\n<ul>");
    for item in crates {
        html.push_str(&format!("<li><code>{}</code></li>", escape_html(&item)));
    }
    html.push_str("</ul>\n<h3>Core Public Modules</h3>\n<ul>");
    for module in modules {
        html.push_str(&format!("<li><code>{}</code></li>", escape_html(&module)));
    }
    html.push_str("</ul>");
    html
}

fn prompt_summary(evidence: &[DocEvidenceFile]) -> String {
    let prompt_file = evidence
        .iter()
        .find(|file| file.relative == "config/prompts.yaml");
    let Some(prompt_file) = prompt_file else {
        return "<p>No prompt configuration file was found.</p>".into();
    };
    let keys = prompt_file
        .text
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            (trimmed.ends_with(':') && !trimmed.starts_with('#') && !trimmed.starts_with('-'))
                .then(|| trimmed.trim_end_matches(':').to_string())
        })
        .take(40)
        .collect::<Vec<_>>();
    let mut html = String::from(
        "<p>Prompt configuration is treated as developer-facing project understanding input.</p><ul>",
    );
    for key in keys {
        html.push_str(&format!("<li><code>{}</code></li>", escape_html(&key)));
    }
    html.push_str("</ul>");
    html
}

fn developer_understanding_section(
    prompt_text: Option<&str>,
    model_output: Option<&str>,
) -> String {
    let mut html = String::new();
    if let Some(prompt_text) = prompt_text.filter(|value| !value.trim().is_empty()) {
        html.push_str("<h3>Prompt Context</h3>");
        html.push_str(&format!("<pre>{}</pre>", escape_html(prompt_text)));
    }
    if let Some(model_output) = model_output.filter(|value| !value.trim().is_empty()) {
        html.push_str("<h3>Model Output For Developers</h3>");
        html.push_str("<p>This model output is preserved as developer understanding material and rendered as escaped documentation content.</p>");
        html.push_str(&format!("<pre>{}</pre>", escape_html(model_output)));
    }
    if html.is_empty() {
        html.push_str("<p>No prompt or model-output material was supplied for this run.</p>");
    }
    html
}

fn auxiliary_contribution_section(report: &Value) -> String {
    let total = report.get("project_total").unwrap_or(&Value::Null);
    let productions = total
        .get("production_count")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let contributions = total
        .get("contribution_count")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let percentage = total
        .get("contribution_percentage")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    let state_path = report
        .get("state_path")
        .and_then(Value::as_str)
        .unwrap_or("<unavailable>");
    let contribution_summary = if productions == 0 {
        "No implementation-validation correction sample yet".to_string()
    } else {
        format!("{contributions}/{productions} ({percentage:.1}%)")
    };
    let mut html = format!(
        "<div class=\"box\"><p><strong>Implementation-validation correction contribution:</strong> {}</p><p><strong>State:</strong> <code>{}</code></p></div>",
        contribution_summary,
        escape_html(state_path)
    );
    html.push_str("<p>Auxiliary usage stats show advisory and analysis value separately. A correction contribution is counted only when auxiliary AI participates in implementation validation and the final validator output actually corrects the primary model result.</p>");
    if let Some(last_event) = report.get("last_event").filter(|value| !value.is_null()) {
        html.push_str("<h3>Last Contribution Event</h3>");
        html.push_str(&format!(
            "<pre>{}</pre>",
            escape_html(&serde_json::to_string_pretty(last_event).unwrap_or_default())
        ));
    }
    html
}

fn obsolete_asset_section(report: &AssetRetirementReport) -> String {
    if report.blockers.is_empty() {
        return "<p class=\"ok\">No obsolete asset blockers were reported by the asset retirement scan.</p>".into();
    }
    let mut html = String::from(
        "<div class=\"warn\"><p>Obsolete asset cleanup is required before success.</p><ul>",
    );
    for blocker in &report.blockers {
        html.push_str(&format!("<li>{}</li>", escape_html(blocker)));
    }
    html.push_str("</ul></div>");
    html
}

fn evidence_section(evidence: &[DocEvidenceFile]) -> String {
    let mut html = String::from("<ul>");
    for file in evidence {
        html.push_str(&format!(
            "<li><code>{}</code></li>",
            escape_html(&file.relative)
        ));
    }
    html.push_str("</ul>");
    html
}

fn developer_sources(
    root: &Path,
    prompt_text: Option<&str>,
    model_output: Option<&str>,
) -> Vec<PathBuf> {
    let mut sources = Vec::new();
    if prompt_text.is_some() {
        sources.push(root.join("<inline-or-file-prompt>"));
    }
    if model_output.is_some() {
        sources.push(root.join("<model-output>"));
    }
    sources
}

fn section_from_markdown(
    evidence: &[DocEvidenceFile],
    relative: &str,
    title: &str,
) -> Option<String> {
    let file = evidence.iter().find(|file| file.relative == relative)?;
    let mut in_section = false;
    let mut section = Vec::new();
    for line in file.text.lines() {
        let heading = line.trim_start();
        if heading.starts_with("## ") {
            let heading_title = heading.trim_start_matches('#').trim();
            if in_section {
                break;
            }
            in_section = heading_title.eq_ignore_ascii_case(title);
        }
        if in_section {
            section.push(line);
        }
    }
    (!section.is_empty()).then(|| section.join("\n"))
}

fn compact_markdown_text(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut compact = text.chars().take(max_chars).collect::<String>();
    let boundary = readable_truncation_boundary(&compact, max_chars);
    if let Some(boundary) = boundary {
        compact.truncate(boundary);
    }
    compact = compact.trim_end().to_string();
    compact.push_str("\n\n[truncated for generated HTML documentation]");
    compact
}

fn readable_truncation_boundary(text: &str, max_chars: usize) -> Option<usize> {
    let min_boundary = max_chars / 2;
    let mut candidates = Vec::new();
    candidates.extend(
        text.match_indices("\n\n")
            .map(|(index, pattern)| index + pattern.len()),
    );
    candidates.extend(text.match_indices('\n').map(|(index, _)| index + 1));
    candidates.extend(text.match_indices(". ").map(|(index, _)| index + 1));
    candidates.extend(["。", "；"].into_iter().flat_map(|pattern| {
        text.match_indices(pattern)
            .map(move |(index, _)| index + pattern.len())
    }));
    candidates
        .into_iter()
        .filter(|index| *index >= min_boundary && text.is_char_boundary(*index))
        .max()
}

fn markdown_to_html_fragment(markdown: &str) -> String {
    let mut html = String::new();
    let mut in_list = false;
    let mut in_code = false;
    let mut code = String::new();
    for line in markdown.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            if in_code {
                html.push_str(&format!("<pre><code>{}</code></pre>\n", escape_html(&code)));
                code.clear();
                in_code = false;
            } else {
                in_code = true;
            }
            continue;
        }
        if in_code {
            code.push_str(line);
            code.push('\n');
            continue;
        }
        if let Some(title) = trimmed.strip_prefix("### ") {
            close_list(&mut html, &mut in_list);
            html.push_str(&format!("<h3>{}</h3>\n", escape_html(title)));
        } else if trimmed.starts_with("## ") || trimmed.starts_with("# ") {
            close_list(&mut html, &mut in_list);
        } else if let Some(item) = trimmed.strip_prefix("- ") {
            if !in_list {
                html.push_str("<ul>\n");
                in_list = true;
            }
            html.push_str(&format!("<li>{}</li>\n", escape_html(item)));
        } else if trimmed.is_empty() {
            close_list(&mut html, &mut in_list);
        } else {
            close_list(&mut html, &mut in_list);
            html.push_str(&format!("<p>{}</p>\n", escape_html(trimmed)));
        }
    }
    if in_code {
        html.push_str(&format!("<pre><code>{}</code></pre>\n", escape_html(&code)));
    }
    close_list(&mut html, &mut in_list);
    html
}

fn close_list(html: &mut String, in_list: &mut bool) {
    if *in_list {
        html.push_str("</ul>\n");
        *in_list = false;
    }
}

fn prune_stale_generated_docs(root: &Path, output: &Path) -> Result<Vec<PathBuf>> {
    let docs_dir = root.join("docs");
    if !docs_dir.exists() {
        return Ok(Vec::new());
    }
    let mut pruned = Vec::new();
    for entry in WalkDir::new(&docs_dir)
        .into_iter()
        .filter_map(|entry| entry.ok())
    {
        if !entry.file_type().is_file() || entry.path() == output {
            continue;
        }
        if !entry
            .path()
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("html"))
        {
            continue;
        }
        let text = fs::read_to_string(entry.path()).unwrap_or_default();
        if text.contains("name=\"rayman-generated-doc\" content=\"true\"") {
            fs::remove_file(entry.path())
                .with_context(|| format!("无法清理过时生成文档: {}", entry.path().display()))?;
            pruned.push(entry.path().to_path_buf());
        }
    }
    Ok(pruned)
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skill_rule_at_trigger_size_is_not_split() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("SKILL.md");
        let text = "# Title\n".to_string() + &"a".repeat(SKILL_RULE_TRIGGER_CHARS - 8);
        assert_eq!(text.chars().count(), SKILL_RULE_TRIGGER_CHARS);
        fs::write(&path, &text).unwrap();

        let summary = compact_skill_rules(temp.path(), false).unwrap();

        assert_eq!(summary.scanned_files, 1);
        assert_eq!(summary.split_files, 0);
        assert_eq!(summary.skipped_files, 1);
        assert_eq!(fs::read_to_string(path).unwrap(), text);
        assert!(!temp.path().join("references").exists());
    }

    #[test]
    fn skill_rule_above_trigger_is_split_below_target() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("SKILL.md");
        let moved = "## Long Details\n".to_string() + &"Detailed rule line.\n".repeat(1_200);
        let text = "# Skill\n\n## Core\nKeep this core rule.\n\n".to_string() + &moved;
        assert!(text.chars().count() > SKILL_RULE_TRIGGER_CHARS);
        fs::write(&path, &text).unwrap();

        let summary = compact_skill_rules(temp.path(), false).unwrap();
        let source = fs::read_to_string(&path).unwrap();

        assert_eq!(summary.split_files, 1);
        assert!(source.chars().count() < SKILL_RULE_TARGET_CHARS);
        assert!(source.contains("See [Long Details](references/skill-long-details.md)"));
        let reference = temp.path().join("references").join("skill-long-details.md");
        let reference_text = fs::read_to_string(reference).unwrap();
        assert!(reference_text.contains(&moved));
    }

    #[test]
    fn skill_main_above_line_trigger_is_split_even_below_char_trigger() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("SKILL.md");
        let moved = "## Long Details\n".to_string() + &"Detailed rule line.\n".repeat(116);
        let text = "# Skill\n\n## Core\nKeep this core rule.\n\n".to_string()
            + &"core\n".repeat(10)
            + &moved;
        assert!(text.chars().count() < SKILL_RULE_TRIGGER_CHARS);
        assert!(skill_main_line_count(&text) > SKILL_MAIN_COMPACT_TRIGGER_LINES);
        fs::write(&path, &text).unwrap();

        let summary = compact_skill_rules(temp.path(), false).unwrap();
        let source = fs::read_to_string(&path).unwrap();

        assert_eq!(summary.split_files, 1);
        assert!(skill_main_line_count(&source) <= SKILL_MAIN_TARGET_LINES);
        assert!(
            source.chars().count() * 100
                <= text.chars().count() * (100 - SKILL_RULE_MIN_REDUCTION_PERCENT)
        );
        let reference = temp.path().join("references").join("skill-long-details.md");
        let reference_text = fs::read_to_string(reference).unwrap();
        assert!(reference_text.contains(&moved));
    }

    #[test]
    fn dry_run_does_not_write_split_files() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("SKILL.md");
        let text = "# Skill\n\n## Details\n".to_string() + &"x\n".repeat(12_000);
        fs::write(&path, &text).unwrap();

        let summary = compact_skill_rules(temp.path(), true).unwrap();

        assert_eq!(summary.reports[0].action, "would_split");
        assert_eq!(fs::read_to_string(&path).unwrap(), text);
        assert!(!temp.path().join("references").exists());
    }

    #[test]
    fn related_nested_sections_move_with_parent_heading() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("SKILL.md");
        let details = "## Alpha Details\nIntro.\n### Alpha Edge Cases\n".to_string()
            + &"alpha detail\n".repeat(1_700);
        let beta = "## Beta Details\n".to_string() + &"beta detail\n".repeat(100);
        let text = "# Skill\n\n## Core\nKeep this core rule.\n\n".to_string() + &details + &beta;
        assert!(text.chars().count() > SKILL_RULE_TRIGGER_CHARS);
        fs::write(&path, &text).unwrap();

        let summary = compact_skill_rules(temp.path(), false).unwrap();
        let source = fs::read_to_string(&path).unwrap();

        assert!(!summary.reports[0].references.is_empty());
        assert!(source.contains("references/skill-alpha-details.md"));
        assert!(!source.contains("references/skill-alpha-edge-cases.md"));
        let reference =
            fs::read_to_string(temp.path().join("references/skill-alpha-details.md")).unwrap();
        assert!(reference.contains(&details));
        assert!(reference.contains("### Alpha Edge Cases"));
    }

    #[test]
    fn fenced_code_headings_do_not_split_sections() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("SKILL.md");
        let example = "## Examples\n```markdown\n## Not A Real Heading\n```\n".to_string()
            + &"example detail\n".repeat(1_600);
        let text = "# Skill\n\n## Core\nKeep this core rule.\n\n".to_string() + &example;
        assert!(text.chars().count() > SKILL_RULE_TRIGGER_CHARS);
        fs::write(&path, &text).unwrap();

        let summary = compact_skill_rules(temp.path(), false).unwrap();
        let source = fs::read_to_string(&path).unwrap();
        let reference =
            fs::read_to_string(temp.path().join("references/skill-examples.md")).unwrap();

        assert_eq!(summary.reports[0].references.len(), 1);
        assert!(source.contains("references/skill-examples.md"));
        assert!(!source.contains("not-a-real-heading"));
        assert!(reference.contains("```markdown\n## Not A Real Heading\n```"));
    }

    #[test]
    fn skill_main_line_budget_accepts_target_plus_buffer() {
        let text = "line\n".repeat(SKILL_MAIN_COMPACT_TRIGGER_LINES);
        assert!(assert_skill_main_line_budget(&text).is_ok());
    }

    #[test]
    fn skill_main_line_budget_rejects_above_compaction_trigger() {
        let text = "line\n".repeat(SKILL_MAIN_COMPACT_TRIGGER_LINES + 1);
        assert!(assert_skill_main_line_budget(&text).is_err());
    }

    #[test]
    fn compact_markdown_text_truncates_at_readable_boundary() {
        let text = "## CLI\n\n`rayman assets status` reports state.\n\n`rayman assets scan` rereads current files, recomputes stale docs/config/tests/CLI/API references, and writes refreshed state.\n\n`rayman assets cleanup --apply` deletes only registered files.";

        let compact = compact_markdown_text(text, 125);

        assert!(compact.contains("[truncated for generated HTML documentation]"));
        assert!(!compact.contains("recomputes sta\n\n[truncated"));
        assert!(compact.contains("reports state."));
    }

    #[test]
    fn compact_markdown_text_truncates_chinese_at_char_boundary() {
        let text = "## 说明\n\n第一段说明当前行为已经验证。第二段说明生成文档必须在中文标点后安全截断；继续补充更多上下文以触发压缩逻辑。\n\n第三段不应该成为必需内容。";

        let compact = compact_markdown_text(text, 64);

        assert!(compact.contains("[truncated for generated HTML documentation]"));
        assert!(!compact.contains("�"));
        assert!(compact.is_char_boundary(compact.len()));
    }

    #[test]
    fn maintain_html_docs_escapes_model_output_for_developer_docs() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("README.md"), "# Demo\n").unwrap();
        fs::create_dir_all(temp.path().join("docs")).unwrap();
        fs::write(temp.path().join("docs").join("CLI.md"), "# CLI\n").unwrap();
        fs::write(temp.path().join("model.txt"), "<script>alert(1)</script>").unwrap();
        let output = temp.path().join("docs").join("project-docs.html");

        let report = maintain_html_docs(DocsMaintainOptions {
            root: temp.path().to_path_buf(),
            output: Some(output.clone()),
            prompt: Some("explain architecture".into()),
            prompt_file: None,
            model_output: Some(temp.path().join("model.txt")),
            dry_run: false,
            check: false,
            apply_prune: false,
        })
        .unwrap();

        assert_eq!(report.status, "current");
        assert!(
            report
                .sections
                .contains(&"Auxiliary AI Contribution".to_string())
        );
        assert!(report.auxiliary_ai_contribution.is_object());
        let html = fs::read_to_string(output).unwrap();
        assert!(html.contains("Developer Understanding From Model Output"));
        assert!(html.contains("Auxiliary AI Contribution"));
        assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
        assert!(!html.contains("<script>alert(1)</script>"));
    }

    #[test]
    fn maintain_html_docs_includes_agent_yaml_evidence() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("README.md"), "# Demo\n").unwrap();
        fs::create_dir_all(temp.path().join("docs")).unwrap();
        fs::write(temp.path().join("docs").join("CLI.md"), "# CLI\n").unwrap();
        fs::create_dir_all(temp.path().join("agents")).unwrap();
        fs::write(
            temp.path().join("agents").join("openai.yaml"),
            "interface:\n  display_name: Test Agent\n",
        )
        .unwrap();
        let output = temp.path().join("docs").join("project-docs.html");

        let report = maintain_html_docs(DocsMaintainOptions {
            root: temp.path().to_path_buf(),
            output: Some(output),
            prompt: None,
            prompt_file: None,
            model_output: None,
            dry_run: false,
            check: false,
            apply_prune: false,
        })
        .unwrap();

        assert!(report.evidence_files.iter().any(|path| {
            path.strip_prefix(&report.root)
                .ok()
                .is_some_and(|relative| relative == Path::new("agents").join("openai.yaml"))
        }));
    }

    #[test]
    fn maintain_html_docs_excludes_stale_generated_html_evidence() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("README.md"), "# Demo\n").unwrap();
        fs::create_dir_all(temp.path().join("docs")).unwrap();
        fs::write(temp.path().join("docs").join("CLI.md"), "# CLI\n").unwrap();
        let stale = temp.path().join("docs").join("old-project-docs.html");
        fs::write(
            &stale,
            r#"<!doctype html><meta name="rayman-generated-doc" content="true">stale"#,
        )
        .unwrap();
        let output = temp.path().join("docs").join("project-docs.html");

        let report = maintain_html_docs(DocsMaintainOptions {
            root: temp.path().to_path_buf(),
            output: Some(output),
            prompt: None,
            prompt_file: None,
            model_output: None,
            dry_run: false,
            check: false,
            apply_prune: false,
        })
        .unwrap();

        assert!(!report.evidence_files.iter().any(|path| {
            path.strip_prefix(&report.root)
                .ok()
                .is_some_and(|relative| relative == Path::new("docs").join("old-project-docs.html"))
        }));
    }

    #[test]
    fn maintain_html_docs_excludes_compatibility_exempt_assets_from_evidence() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("README.md"), "# Demo\n").unwrap();
        fs::create_dir_all(temp.path().join("docs")).unwrap();
        let obsolete = temp.path().join("docs").join("old-guide.md");
        fs::write(&obsolete, "# Old\n\nObsoleteEvidenceMarker\n").unwrap();
        AssetRetirementManager::new(temp.path())
            .unwrap()
            .exempt(crate::assets::AssetExemptRequest {
                path: obsolete,
                retention_reason: "temporary audit retention".into(),
                expires_at: "2999-01-01".into(),
            })
            .unwrap();
        let output = temp.path().join("docs").join("project-docs.html");

        let report = maintain_html_docs(DocsMaintainOptions {
            root: temp.path().to_path_buf(),
            output: Some(output.clone()),
            prompt: None,
            prompt_file: None,
            model_output: None,
            dry_run: false,
            check: false,
            apply_prune: false,
        })
        .unwrap();

        assert_eq!(report.status, "current");
        assert!(report.asset_retirement.blockers.is_empty());
        assert!(
            report
                .asset_retirement
                .exemptions
                .iter()
                .any(|record| record.path == "docs/old-guide.md")
        );
        assert!(!report.evidence_files.iter().any(|path| {
            path.strip_prefix(&report.root)
                .ok()
                .is_some_and(|relative| relative == Path::new("docs").join("old-guide.md"))
        }));
        let html = fs::read_to_string(output).unwrap();
        assert!(!html.contains("ObsoleteEvidenceMarker"));
    }

    #[test]
    fn maintain_html_docs_auto_completes_customer_project_docs() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(
            temp.path().join("package.json"),
            r#"{"scripts":{"test":"vitest"}}"#,
        )
        .unwrap();
        fs::create_dir_all(temp.path().join("src")).unwrap();
        fs::write(
            temp.path().join("src").join("index.ts"),
            "export function run() { return 1; }\n",
        )
        .unwrap();
        let output = temp.path().join("docs").join("project-docs.html");

        let check_report = maintain_html_docs(DocsMaintainOptions {
            root: temp.path().to_path_buf(),
            output: Some(output.clone()),
            prompt: None,
            prompt_file: None,
            model_output: None,
            dry_run: false,
            check: true,
            apply_prune: false,
        })
        .unwrap();

        assert_eq!(check_report.status, "stale");
        assert_eq!(check_report.customer_docs.status, "stale");
        assert!(!temp.path().join(CUSTOMER_PROJECT_GUIDE_RELATIVE).exists());

        let report = maintain_html_docs(DocsMaintainOptions {
            root: temp.path().to_path_buf(),
            output: Some(output.clone()),
            prompt: None,
            prompt_file: None,
            model_output: None,
            dry_run: false,
            check: false,
            apply_prune: false,
        })
        .unwrap();

        let guide = fs::read_to_string(temp.path().join(CUSTOMER_PROJECT_GUIDE_RELATIVE)).unwrap();
        let readme = fs::read_to_string(temp.path().join("README.md")).unwrap();
        let html = fs::read_to_string(output).unwrap();
        assert_eq!(report.status, "current");
        assert_eq!(report.customer_docs.status, "current");
        assert!(guide.contains(CUSTOMER_PROJECT_GUIDE_MARKER));
        assert!(guide.contains("## Setup"));
        assert!(guide.contains("## Usage"));
        assert!(guide.contains("## Architecture"));
        assert!(guide.contains("## Configuration"));
        assert!(guide.contains("## Testing And Validation"));
        assert!(readme.contains(CUSTOMER_README_MARKER));
        assert!(html.contains("Customer Documentation Completeness"));

        let second_check = maintain_html_docs(DocsMaintainOptions {
            root: temp.path().to_path_buf(),
            output: Some(temp.path().join("docs").join("project-docs.html")),
            prompt: None,
            prompt_file: None,
            model_output: None,
            dry_run: false,
            check: true,
            apply_prune: false,
        })
        .unwrap();
        assert_eq!(second_check.status, "current");
    }

    #[test]
    fn maintain_html_docs_preserves_existing_customer_readme() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("Cargo.toml"), "[package]\nname='demo'\n").unwrap();
        fs::create_dir_all(temp.path().join("src")).unwrap();
        fs::write(temp.path().join("src").join("lib.rs"), "pub fn demo() {}\n").unwrap();
        fs::write(
            temp.path().join("README.md"),
            "# Custom README\n\nOverview only.\n",
        )
        .unwrap();
        let output = temp.path().join("docs").join("project-docs.html");

        let report = maintain_html_docs(DocsMaintainOptions {
            root: temp.path().to_path_buf(),
            output: Some(output),
            prompt: None,
            prompt_file: None,
            model_output: None,
            dry_run: false,
            check: false,
            apply_prune: false,
        })
        .unwrap();

        assert_eq!(
            fs::read_to_string(temp.path().join("README.md")).unwrap(),
            "# Custom README\n\nOverview only.\n"
        );
        assert!(temp.path().join(CUSTOMER_PROJECT_GUIDE_RELATIVE).exists());
        assert!(
            !report
                .customer_docs
                .generated_files
                .contains(&temp.path().join("README.md"))
        );
    }

    #[test]
    fn html_docs_ui_contract_preserves_chinese_and_layout_markers() {
        // @ui:html_docs
        let temp = tempfile::tempdir().unwrap();
        fs::write(
            temp.path().join("README.md"),
            "# 示例项目\n支持中文目录名。\n",
        )
        .unwrap();
        fs::create_dir_all(temp.path().join("docs")).unwrap();
        fs::write(temp.path().join("docs").join("CLI.md"), "# CLI\n").unwrap();
        fs::write(
            temp.path().join("model.txt"),
            "模型说明：支持中文目录名 <script>alert('中文')</script>",
        )
        .unwrap();
        let output = temp.path().join("docs").join("project-docs.html");

        maintain_html_docs(DocsMaintainOptions {
            root: temp.path().to_path_buf(),
            output: Some(output.clone()),
            prompt: Some("说明中文 UI 输出".into()),
            prompt_file: None,
            model_output: Some(temp.path().join("model.txt")),
            dry_run: false,
            check: false,
            apply_prune: false,
        })
        .unwrap();

        let html = fs::read_to_string(output).unwrap();
        assert!(html.contains("<meta charset=\"utf-8\">"));
        assert!(
            html.contains(
                "<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">"
            )
        );
        assert!(html.contains("grid-template-columns:repeat(auto-fit,minmax(240px,1fr))"));
        assert!(html.contains("说明中文 UI 输出"));
        assert!(html.contains("模型说明：支持中文目录名"));
        assert!(html.contains("&lt;script&gt;alert(&#39;中文&#39;)&lt;/script&gt;"));
        assert!(!html.contains("<script>alert('中文')</script>"));
    }

    #[test]
    fn maintain_html_docs_dry_run_does_not_write_output() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("README.md"), "# Demo\n").unwrap();
        let output = temp.path().join("docs").join("project-docs.html");

        let report = maintain_html_docs(DocsMaintainOptions {
            root: temp.path().to_path_buf(),
            output: Some(output.clone()),
            prompt: None,
            prompt_file: None,
            model_output: None,
            dry_run: true,
            check: false,
            apply_prune: false,
        })
        .unwrap();

        assert_eq!(report.status, "would_update");
        assert!(!output.exists());
    }

    #[test]
    fn maintain_html_docs_check_reports_stale_without_writing() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("README.md"), "# Demo\n").unwrap();
        fs::create_dir_all(temp.path().join("docs")).unwrap();
        let output = temp.path().join("docs").join("project-docs.html");

        let report = maintain_html_docs(DocsMaintainOptions {
            root: temp.path().to_path_buf(),
            output: Some(output.clone()),
            prompt: None,
            prompt_file: None,
            model_output: None,
            dry_run: false,
            check: true,
            apply_prune: false,
        })
        .unwrap();

        assert_eq!(report.status, "stale");
        assert!(!output.exists());
    }

    #[test]
    fn maintain_html_docs_check_blocks_obsolete_asset_candidates() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("README.md"), "# Demo\n").unwrap();
        let obsolete = temp.path().join("old-entry.md");
        fs::write(&obsolete, "obsolete entry\n").unwrap();
        AssetRetirementManager::new(temp.path())
            .unwrap()
            .retire(crate::assets::AssetRetireRequest {
                path: obsolete,
                replacement_behavior: "current docs cover the replacement behavior".into(),
                deletion_reason: "old entry is no longer part of the current surface".into(),
                validation_command: "rayman docs maintain --check".into(),
                apply_delete: false,
            })
            .unwrap();
        let output = temp.path().join("docs").join("project-docs.html");

        let report = maintain_html_docs(DocsMaintainOptions {
            root: temp.path().to_path_buf(),
            output: Some(output),
            prompt: None,
            prompt_file: None,
            model_output: None,
            dry_run: false,
            check: true,
            apply_prune: false,
        })
        .unwrap();

        assert_eq!(report.status, "blocked");
        assert!(!report.obsolete_asset_blockers.is_empty());
        assert!(
            report
                .required_actions
                .iter()
                .any(|action| action.contains("rayman assets"))
        );
    }

    #[test]
    fn maintain_html_docs_prunes_only_marked_generated_html() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("README.md"), "# Demo\n").unwrap();
        let docs = temp.path().join("docs");
        fs::create_dir_all(&docs).unwrap();
        let stale = docs.join("old.html");
        let manual = docs.join("manual.html");
        fs::write(
            &stale,
            r#"<meta name="rayman-generated-doc" content="true">old"#,
        )
        .unwrap();
        fs::write(&manual, "<html>manual</html>").unwrap();

        let report = maintain_html_docs(DocsMaintainOptions {
            root: temp.path().to_path_buf(),
            output: Some(docs.join("project-docs.html")),
            prompt: None,
            prompt_file: None,
            model_output: None,
            dry_run: false,
            check: false,
            apply_prune: true,
        })
        .unwrap();

        assert_eq!(report.pruned_assets.len(), 1);
        assert!(!stale.exists());
        assert!(manual.exists());
    }
}
