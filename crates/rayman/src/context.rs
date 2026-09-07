//! 工作区上下文索引：单次遍历 + 每文件指纹缓存。
//!
//! `refresh` 对每个文件重算内容摘要，并在遍历或读取失败时拒绝写出索引；缓存仅用于
//! 报告复用统计，不能作为跳过内容证明的依据。

use std::collections::BTreeMap;
use std::path::{Component, Path};
use std::time::UNIX_EPOCH;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::file_io::is_link_or_reparse;
use crate::file_io::{read_json, write_json};
use crate::hash::{sha256_bytes, sha256_file};
use crate::pathfmt::display_path;
use crate::state_paths;
use crate::timefmt::now_iso;
use crate::walk::{relative_key, workspace_files_checked};

#[cfg(test)]
const INDEX_RELATIVE_PATH: &str = ".RaymanCodingSkill/context/index.json";
const INDEX_STATE_RELATIVE: &str = "context/index.json";
pub const CONTEXT_SCHEMA_VERSION: u32 = 2;
const SOURCE_EXTENSIONS: &[&str] = &[
    "rs", "js", "jsx", "ts", "tsx", "py", "go", "java", "cs", "cpp", "c", "h", "hpp", "rb", "php",
    "swift", "kt", "scala",
];
const TEST_MARKERS: &[&str] = &["test", "tests", "spec"];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileEntry {
    pub path: String,
    pub size: u64,
    pub mtime_ns: u128,
    pub sha256: String,
    pub kind: String,
    pub lines: usize,
    #[serde(default)]
    pub symbols: Vec<Symbol>,
    /// 本进程从不写入这个字段：`build_entry` 在读取/哈希失败时直接返回 `Err`，
    /// `refresh` 把错误向上抛，索引根本不会落盘。字段只用于**读取**外部写入或
    /// 被篡改的索引文件——发现它有值就说明那份索引不可信，必须拒绝。
    #[serde(default)]
    pub read_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Symbol {
    pub name: String,
    pub kind: String,
    pub line: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ContextIndex {
    #[serde(default)]
    pub schema_version: u32,
    pub generated_at: String,
    pub workspace: String,
    #[serde(default)]
    pub workspace_identity: String,
    pub files: Vec<FileEntry>,
}

pub const CONTEXT_DELIVERY_SCHEMA: &str = "rayman.context-delivery.v1";
const CONTEXT_DELIVERY_CURSOR_PREFIX: &str = "rayman-context-cursor-v1";
pub use crate::build_context_delivery_page;
/// Machine contract shared by future budgeted context-navigation commands.
///
/// This type deliberately carries only navigation authority.  A successfully
/// decoded envelope can help select source to inspect, but it can never satisfy
/// a goal validation, completion, release, or installation requirement.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ContextDeliveryV1 {
    pub schema: String,
    pub authority: ContextDeliveryAuthority,
    pub snapshot_sha256: String,
    pub query_sha256: String,
    pub sort_sha256: String,
    /// Zero-based offset of the first item in this page.
    pub offset: usize,
    /// Exact UTF-8 byte ceiling for the canonical compact JSON document.
    pub budget_bytes: usize,
    /// Exact UTF-8 byte count of the canonical compact JSON document.
    pub output_bytes: usize,
    /// Total deterministic matches for the bound query and snapshot.
    pub total: usize,
    /// Complete records carried by this page.
    pub returned: usize,
    /// Matches remaining after this page.
    pub omitted: usize,
    pub truncated: bool,
    pub next_cursor: Option<String>,
    /// One deterministic page containing both resolved and unresolved records.
    /// A single cursor therefore budgets every record; there is no unbounded
    /// unresolved side channel.
    pub records: Vec<ContextDeliveryRecord>,
    pub coverage: ContextDeliveryCoverage,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ContextDeliveryAuthority {
    NavigationOnly,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "state", content = "record", rename_all = "snake_case")]
pub enum ContextDeliveryRecord {
    Resolved(ContextDeliveryItem),
    Unresolved(ContextDeliveryUnresolved),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ContextDeliveryItem {
    /// Normalized workspace-relative path using `/` separators.
    pub path: String,
    pub sha256: String,
    pub line_range: ContextDeliveryLineRange,
    pub reason: String,
    pub provenance: Vec<ContextDeliveryProvenance>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub attributes: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContextDeliveryLineRange {
    /// One-based inclusive start line.
    pub start: usize,
    /// One-based inclusive end line.
    pub end: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContextDeliveryProvenance {
    pub kind: String,
    pub evidence: String,
    /// When provenance names a file, path and hash are an all-or-nothing pair.
    pub source_path: Option<String>,
    pub source_sha256: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContextDeliveryUnresolved {
    pub reference: String,
    pub reason: String,
    pub provenance: Vec<ContextDeliveryProvenance>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContextDeliveryCoverage {
    pub status: ContextDeliveryCoverageStatus,
    /// Totals cover the complete bound query, while returned counts cover only
    /// this page's tagged records.
    pub resolved_total: usize,
    pub resolved_returned: usize,
    pub unresolved_total: usize,
    pub unresolved_returned: usize,
    pub omitted_items: usize,
    /// Required for `unknown`; optional explanatory text for other states.
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ContextDeliveryCoverageStatus {
    Complete,
    Partial,
    Unknown,
}

/// Identity that every continuation cursor is deterministically content-bound to.
/// Changing the schema, snapshot, normalized query, or deterministic sort
/// specification produces a different binding and invalidates the old cursor.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContextDeliveryCursorBinding {
    pub schema: String,
    pub snapshot_sha256: String,
    pub query_sha256: String,
    pub sort_sha256: String,
}

impl ContextDeliveryCursorBinding {
    pub fn v1(snapshot_sha256: String, query_sha256: String, sort_sha256: String) -> Self {
        Self {
            schema: CONTEXT_DELIVERY_SCHEMA.into(),
            snapshot_sha256,
            query_sha256,
            sort_sha256,
        }
    }

    pub(crate) fn validate(&self) -> Result<()> {
        if self.schema != CONTEXT_DELIVERY_SCHEMA {
            bail!("unsupported context delivery schema: {}", self.schema);
        }
        for (label, value) in [
            ("snapshot_sha256", self.snapshot_sha256.as_str()),
            ("query_sha256", self.query_sha256.as_str()),
            ("sort_sha256", self.sort_sha256.as_str()),
        ] {
            if !is_lower_sha256(value) {
                bail!("context delivery {label} must be a lowercase SHA-256 digest");
            }
        }
        Ok(())
    }

    fn cursor_digest(&self, position: usize) -> Result<String> {
        self.validate()?;
        Ok(sha256_bytes(&serde_json::to_vec(&(self, position))?))
    }

    pub fn encode_cursor(&self, position: usize) -> Result<String> {
        Ok(format!(
            "{CONTEXT_DELIVERY_CURSOR_PREFIX}:{}:{position}",
            self.cursor_digest(position)?
        ))
    }

    pub fn decode_cursor(&self, cursor: &str) -> Result<usize> {
        let parts = cursor.split(':').collect::<Vec<_>>();
        if parts.len() != 3 || parts[0] != CONTEXT_DELIVERY_CURSOR_PREFIX {
            bail!("malformed context delivery cursor");
        }
        let position = parts[2]
            .parse::<usize>()
            .context("malformed context delivery cursor position")?;
        if position.to_string() != parts[2] {
            bail!("context delivery cursor position is not canonical");
        }
        let expected = self.cursor_digest(position)?;
        if parts[1] != expected {
            bail!(
                "context delivery cursor was invalidated by schema, snapshot, query, sort, or position drift"
            );
        }
        Ok(position)
    }
}

impl ContextDeliveryV1 {
    pub fn cursor_binding(&self) -> ContextDeliveryCursorBinding {
        ContextDeliveryCursorBinding {
            schema: self.schema.clone(),
            snapshot_sha256: self.snapshot_sha256.clone(),
            query_sha256: self.query_sha256.clone(),
            sort_sha256: self.sort_sha256.clone(),
        }
    }

    /// Serialize the exact machine contract.  Budget enforcement never drops
    /// a partial record: callers must construct truthful counts and a bound
    /// cursor first, then this method either emits the complete document or
    /// rejects it as over budget.
    pub fn encode_json(&mut self) -> Result<Vec<u8>> {
        let bytes = self.stabilized_json()?;
        if bytes.len() > self.budget_bytes {
            bail!(
                "context delivery output exceeds UTF-8 budget: {} > {}",
                bytes.len(),
                self.budget_bytes
            );
        }
        Ok(bytes)
    }

    pub(crate) fn stabilized_json(&mut self) -> Result<Vec<u8>> {
        self.validate_semantics()?;
        for _ in 0..8 {
            let bytes = serde_json::to_vec(self)?;
            if self.output_bytes == bytes.len() {
                return Ok(bytes);
            }
            self.output_bytes = bytes.len();
        }
        bail!("context delivery output byte count did not stabilize");
    }

    pub fn decode_json(bytes: &[u8]) -> Result<Self> {
        let envelope =
            serde_json::from_slice::<Self>(bytes).context("invalid context delivery v1 JSON")?;
        envelope.validate()?;
        if serde_json::to_vec(&envelope)? != bytes {
            bail!("context delivery v1 JSON is not in canonical compact form");
        }
        Ok(envelope)
    }

    pub fn validate(&self) -> Result<()> {
        self.validate_semantics()?;
        let actual = serde_json::to_vec(self)?.len();
        if self.output_bytes != actual {
            bail!(
                "context delivery output_bytes mismatch: declared {} actual {actual}",
                self.output_bytes
            );
        }
        if actual > self.budget_bytes {
            bail!(
                "context delivery output exceeds UTF-8 budget: {actual} > {}",
                self.budget_bytes
            );
        }
        Ok(())
    }

    fn validate_semantics(&self) -> Result<()> {
        self.cursor_binding().validate()?;
        if self.authority != ContextDeliveryAuthority::NavigationOnly {
            bail!("context delivery authority must be navigation_only");
        }
        if self.budget_bytes == 0 {
            bail!("context delivery budget_bytes must be positive");
        }
        if self.returned != self.records.len() {
            bail!("context delivery returned count does not match records");
        }
        let consumed = self
            .offset
            .checked_add(self.returned)
            .and_then(|value| value.checked_add(self.omitted))
            .context("context delivery counts overflow")?;
        if self.total != consumed {
            bail!("context delivery total must equal offset + returned + omitted");
        }
        if self.truncated != (self.omitted > 0) {
            bail!("context delivery truncated must exactly reflect omitted items");
        }
        if self.truncated && self.returned == 0 {
            bail!("context delivery cannot issue a non-advancing cursor");
        }
        match (&self.next_cursor, self.truncated) {
            (Some(cursor), true) => {
                let expected = self
                    .offset
                    .checked_add(self.returned)
                    .context("context delivery cursor position overflow")?;
                if self.cursor_binding().decode_cursor(cursor)? != expected {
                    bail!("context delivery next_cursor position is inconsistent");
                }
            }
            (None, false) => {}
            _ => bail!("context delivery next_cursor presence must exactly reflect truncation"),
        }
        let mut resolved_returned = 0usize;
        let mut unresolved_returned = 0usize;
        for record in &self.records {
            match record {
                ContextDeliveryRecord::Resolved(item) => {
                    resolved_returned += 1;
                    validate_delivery_item(item)?;
                }
                ContextDeliveryRecord::Unresolved(unresolved) => {
                    unresolved_returned += 1;
                    if unresolved.reference.trim().is_empty()
                        || unresolved.reason.trim().is_empty()
                        || unresolved.provenance.is_empty()
                    {
                        bail!("context delivery unresolved entry is incomplete");
                    }
                    for provenance in &unresolved.provenance {
                        validate_delivery_provenance(provenance)?;
                    }
                }
            }
        }
        let coverage_total = self
            .coverage
            .resolved_total
            .checked_add(self.coverage.unresolved_total)
            .context("context delivery coverage total overflow")?;
        if coverage_total != self.total
            || resolved_returned > self.coverage.resolved_total
            || unresolved_returned > self.coverage.unresolved_total
            || self.coverage.resolved_returned != resolved_returned
            || self.coverage.unresolved_returned != unresolved_returned
            || resolved_returned + unresolved_returned != self.returned
            || self.coverage.omitted_items != self.omitted
        {
            bail!("context delivery coverage counts are inconsistent");
        }
        let incomplete = self.omitted > 0 || self.coverage.unresolved_total > 0;
        match self.coverage.status {
            ContextDeliveryCoverageStatus::Complete if incomplete => {
                bail!("complete context delivery coverage cannot omit or leave unresolved items")
            }
            ContextDeliveryCoverageStatus::Partial if !incomplete => {
                bail!("partial context delivery coverage requires omitted or unresolved items")
            }
            ContextDeliveryCoverageStatus::Unknown
                if self
                    .coverage
                    .detail
                    .as_deref()
                    .is_none_or(|detail| detail.trim().is_empty()) =>
            {
                bail!("unknown context delivery coverage requires detail")
            }
            _ => {}
        }
        Ok(())
    }
}

/// Hash a canonical JSON identity used for snapshot, normalized-query, and
/// deterministic-sort bindings. This is an identity helper, never authority.
pub fn context_delivery_identity<T: Serialize>(value: &T) -> Result<String> {
    Ok(sha256_bytes(&serde_json::to_vec(value)?))
}

const CONTEXT_FILE_KINDS: &[&str] = &["source", "test", "docs", "config", "script", "asset"];
const CONTEXT_DELIVERY_FIELDS: &[&str] = &[
    "path",
    "sha256",
    "line_range",
    "reason",
    "provenance",
    "record_type",
    "kind",
    "size",
    "lines",
    "symbol_count",
    "symbols",
    "name",
    "symbol_kind",
    "matched",
    "match_line",
    "text",
    "encoding",
    "start_line",
    "end_line",
    "requested_path",
];

#[derive(Debug, Clone, Serialize)]
pub struct ContextOverviewOptions {
    pub kinds: Vec<String>,
    pub path_prefix: Option<String>,
    pub fields: Vec<String>,
    #[serde(skip)]
    pub limit: usize,
    #[serde(skip)]
    pub cursor: Option<String>,
    #[serde(skip)]
    pub budget_bytes: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct ContextQueryOptions {
    pub kinds: Vec<String>,
    pub path_prefix: Option<String>,
    pub fields: Vec<String>,
    pub include_path: bool,
    pub include_symbols: bool,
    pub include_content: bool,
    #[serde(skip)]
    pub limit: usize,
    #[serde(skip)]
    pub cursor: Option<String>,
    #[serde(skip)]
    pub budget_bytes: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct ContextTextOptions {
    pub fields: Vec<String>,
    #[serde(skip)]
    pub limit: usize,
    #[serde(skip)]
    pub cursor: Option<String>,
    #[serde(skip)]
    pub budget_bytes: usize,
}

pub fn overview_delivery(
    index: &ContextIndex,
    raw_index_sha256: &str,
    options: ContextOverviewOptions,
) -> Result<ContextDeliveryV1> {
    let options = normalize_overview_options(options)?;
    let mut records = Vec::new();
    for entry in index
        .files
        .iter()
        .filter(|entry| entry_matches_scope(entry, &options.kinds, options.path_prefix.as_deref()))
    {
        records.push(entry_overview_record(entry, &options.fields)?);
    }
    finish_context_delivery(
        context_delivery_snapshot(index, raw_index_sha256)?,
        json!({"command":"context overview","options":options}),
        "context-overview:context-record-canonical-json-asc:v1",
        records,
        options.limit,
        options.cursor.as_deref(),
        options.budget_bytes,
    )
}

pub fn query_delivery(
    root: &Path,
    index: &ContextIndex,
    raw_index_sha256: &str,
    term: &str,
    options: ContextQueryOptions,
) -> Result<ContextDeliveryV1> {
    let term = normalized_query_term(term)?;
    let options = normalize_query_options(options)?;
    let needle = term.to_lowercase();
    let mut records = Vec::new();
    for entry in index
        .files
        .iter()
        .filter(|entry| entry_matches_scope(entry, &options.kinds, options.path_prefix.as_deref()))
    {
        if options.include_path && entry.path.to_lowercase().contains(&needle) {
            records.push(entry_path_match_record(entry, &term, &options.fields)?);
        }
        if options.include_symbols {
            for symbol in entry
                .symbols
                .iter()
                .filter(|symbol| symbol.name.to_lowercase().contains(&needle))
            {
                records.push(entry_symbol_match_record(
                    entry,
                    symbol,
                    &term,
                    &options.fields,
                )?);
            }
        }
        if options.include_content && is_text_search_kind(entry) {
            match read_verified_utf8_text(root, entry)? {
                Some(text) => {
                    for (line_index, line) in text.lines().enumerate() {
                        if line.to_lowercase().contains(&needle) {
                            records.push(entry_content_match_record(
                                entry,
                                line_index + 1,
                                line,
                                &term,
                                &options.fields,
                            )?);
                        }
                    }
                }
                None => records.push(unresolved_for_entry(
                    entry,
                    "file content is not valid UTF-8",
                    "verified_context_content",
                    format!("content query `{term}`"),
                )),
            }
        }
    }
    finish_context_delivery(
        context_delivery_snapshot(index, raw_index_sha256)?,
        json!({"command":"context query","term":term,"options":options}),
        "context-query:context-record-canonical-json-asc:v1",
        records,
        options.limit,
        options.cursor.as_deref(),
        options.budget_bytes,
    )
}

pub fn excerpt_delivery(
    root: &Path,
    index: &ContextIndex,
    raw_index_sha256: &str,
    path: &str,
    start: usize,
    end: usize,
    options: ContextTextOptions,
) -> Result<ContextDeliveryV1> {
    let path = normalized_context_path(path, "path")?;
    let options = normalize_text_options(options)?;
    if start == 0 || end < start {
        bail!("context excerpt line range must be one-based and inclusive");
    }
    let record = if let Some(entry) = entry_for_path(index, &path) {
        validate_line_range(entry, start, end)?;
        match read_verified_utf8_text(root, entry)? {
            Some(text) => entry_excerpt_record(entry, start, end, &text, &options.fields)?,
            None => unresolved_for_entry(
                entry,
                "file content is not valid UTF-8",
                "context_excerpt",
                format!("{path}:{start}-{end}"),
            ),
        }
    } else {
        unresolved_reference(
            path.clone(),
            "path is absent from the verified context snapshot",
            "context_excerpt",
            format!("{path}:{start}-{end}"),
        )
    };
    finish_context_delivery(
        context_delivery_snapshot(index, raw_index_sha256)?,
        json!({"command":"context excerpt","path":path,"start":start,"end":end,"options":options}),
        "context-excerpt:context-record-canonical-json-asc:v1",
        vec![record],
        options.limit,
        options.cursor.as_deref(),
        options.budget_bytes,
    )
}

pub fn pack_delivery(
    root: &Path,
    index: &ContextIndex,
    raw_index_sha256: &str,
    paths: &[String],
    options: ContextTextOptions,
) -> Result<ContextDeliveryV1> {
    let paths = normalized_context_paths(paths)?;
    let options = normalize_text_options(options)?;
    let mut records = Vec::new();
    for path in &paths {
        if let Some(entry) = entry_for_path(index, path) {
            match read_verified_utf8_text(root, entry)? {
                Some(text) => records.push(entry_pack_record(entry, &text, &options.fields)?),
                None => records.push(unresolved_for_entry(
                    entry,
                    "file content is not valid UTF-8",
                    "context_pack",
                    path.clone(),
                )),
            }
        } else {
            records.push(unresolved_reference(
                path.clone(),
                "path is absent from the verified context snapshot",
                "context_pack",
                path.clone(),
            ));
        }
    }
    finish_context_delivery(
        context_delivery_snapshot(index, raw_index_sha256)?,
        json!({"command":"context pack","paths":paths,"options":options}),
        "context-pack:context-record-canonical-json-asc:v1",
        records,
        options.limit,
        options.cursor.as_deref(),
        options.budget_bytes,
    )
}

fn normalize_overview_options(
    mut options: ContextOverviewOptions,
) -> Result<ContextOverviewOptions> {
    normalize_page_controls(options.limit, options.budget_bytes, "context overview")?;
    options.kinds = normalized_kinds(options.kinds)?;
    options.path_prefix = options
        .path_prefix
        .map(|prefix| normalized_context_prefix(&prefix))
        .transpose()?;
    options.fields = normalized_fields(options.fields)?;
    Ok(options)
}

fn normalize_query_options(mut options: ContextQueryOptions) -> Result<ContextQueryOptions> {
    normalize_page_controls(options.limit, options.budget_bytes, "context query")?;
    options.kinds = normalized_kinds(options.kinds)?;
    options.path_prefix = options
        .path_prefix
        .map(|prefix| normalized_context_prefix(&prefix))
        .transpose()?;
    options.fields = normalized_fields(options.fields)?;
    if !options.include_path && !options.include_symbols && !options.include_content {
        options.include_path = true;
        options.include_symbols = true;
    }
    Ok(options)
}

fn normalize_text_options(mut options: ContextTextOptions) -> Result<ContextTextOptions> {
    normalize_page_controls(options.limit, options.budget_bytes, "context text")?;
    options.fields = normalized_fields(options.fields)?;
    Ok(options)
}

fn normalize_page_controls(limit: usize, budget_bytes: usize, command: &str) -> Result<()> {
    if limit == 0 {
        bail!("{command} --limit must be positive");
    }
    if budget_bytes == 0 {
        bail!("{command} --budget-bytes must be positive");
    }
    Ok(())
}

fn normalized_kinds(kinds: Vec<String>) -> Result<Vec<String>> {
    let mut normalized = kinds
        .into_iter()
        .map(|kind| kind.trim().to_ascii_lowercase())
        .filter(|kind| !kind.is_empty())
        .collect::<Vec<_>>();
    normalized.sort();
    normalized.dedup();
    if let Some(kind) = normalized
        .iter()
        .find(|kind| !CONTEXT_FILE_KINDS.contains(&kind.as_str()))
    {
        bail!(
            "unknown context kind `{kind}`; available kinds: {}",
            CONTEXT_FILE_KINDS.join(",")
        );
    }
    Ok(normalized)
}

fn normalized_fields(fields: Vec<String>) -> Result<Vec<String>> {
    let mut normalized = fields
        .into_iter()
        .map(|field| field.trim().to_string())
        .filter(|field| !field.is_empty())
        .collect::<Vec<_>>();
    normalized.sort();
    normalized.dedup();
    if let Some(field) = normalized
        .iter()
        .find(|field| !CONTEXT_DELIVERY_FIELDS.contains(&field.as_str()))
    {
        bail!(
            "unknown context delivery field `{field}`; available fields: {}",
            CONTEXT_DELIVERY_FIELDS.join(",")
        );
    }
    Ok(normalized)
}

fn normalized_context_prefix(raw: &str) -> Result<String> {
    let path = raw.trim().trim_end_matches('/');
    if path.is_empty() {
        bail!("context delivery --path-prefix must be normalized and workspace-relative: {raw}");
    }
    normalized_context_path(path, "--path-prefix").map_err(|_| {
        anyhow::anyhow!(
            "context delivery --path-prefix must be normalized and workspace-relative: {raw}"
        )
    })
}

fn normalized_context_path(raw: &str, label: &str) -> Result<String> {
    let path = raw.trim();
    validate_delivery_path(path).with_context(|| {
        format!("context delivery {label} must be normalized and workspace-relative: {raw}")
    })?;
    Ok(path.to_string())
}

fn normalized_context_paths(paths: &[String]) -> Result<Vec<String>> {
    if paths.is_empty() {
        bail!("context pack requires at least one path");
    }
    let mut normalized = paths
        .iter()
        .map(|path| normalized_context_path(path, "path"))
        .collect::<Result<Vec<_>>>()?;
    normalized.sort();
    normalized.dedup();
    Ok(normalized)
}

fn normalized_query_term(term: &str) -> Result<String> {
    let term = term.trim();
    if term.is_empty() {
        bail!("context query term must not be empty");
    }
    Ok(term.to_string())
}

fn entry_matches_scope(entry: &FileEntry, kinds: &[String], path_prefix: Option<&str>) -> bool {
    (kinds.is_empty() || kinds.binary_search(&entry.kind).is_ok())
        && path_prefix.is_none_or(|prefix| path_matches_prefix(&entry.path, prefix))
}

fn path_matches_prefix(path: &str, prefix: &str) -> bool {
    path == prefix
        || path
            .strip_prefix(prefix)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

fn is_text_search_kind(entry: &FileEntry) -> bool {
    matches!(
        entry.kind.as_str(),
        "source" | "test" | "docs" | "config" | "script"
    )
}

fn entry_for_path<'a>(index: &'a ContextIndex, path: &str) -> Option<&'a FileEntry> {
    index.files.iter().find(|entry| entry.path == path)
}

fn validate_line_range(entry: &FileEntry, start: usize, end: usize) -> Result<()> {
    let max_line = entry.lines.max(1);
    if start > max_line || end > max_line {
        bail!(
            "context excerpt line range exceeds indexed file length: {} has {} line(s)",
            entry.path,
            entry.lines
        );
    }
    Ok(())
}

fn read_verified_utf8_text(root: &Path, entry: &FileEntry) -> Result<Option<String>> {
    let bytes = read_verified_file(root, entry)?;
    Ok(String::from_utf8(bytes).ok())
}

fn entry_line_range(entry: &FileEntry) -> ContextDeliveryLineRange {
    ContextDeliveryLineRange {
        start: 1,
        end: entry.lines.max(1),
    }
}

fn entry_overview_record(entry: &FileEntry, fields: &[String]) -> Result<ContextDeliveryRecord> {
    let mut attributes = base_file_attributes(entry);
    if fields
        .binary_search_by(|candidate| candidate.as_str().cmp("symbols"))
        .is_ok()
    {
        attributes.insert("symbols".into(), json!(&entry.symbols));
    }
    project_fields(fields, &mut attributes);
    Ok(resolved_for_entry(
        entry,
        entry_line_range(entry),
        "indexed context file",
        "context_index",
        entry.path.clone(),
        attributes,
    ))
}

fn entry_path_match_record(
    entry: &FileEntry,
    term: &str,
    fields: &[String],
) -> Result<ContextDeliveryRecord> {
    let mut attributes = base_file_attributes(entry);
    attributes.insert("matched".into(), json!("path"));
    project_fields(fields, &mut attributes);
    Ok(resolved_for_entry(
        entry,
        entry_line_range(entry),
        "path contains query term",
        "context_query_path",
        term.to_string(),
        attributes,
    ))
}

fn entry_symbol_match_record(
    entry: &FileEntry,
    symbol: &Symbol,
    term: &str,
    fields: &[String],
) -> Result<ContextDeliveryRecord> {
    let mut attributes = base_file_attributes(entry);
    attributes.insert("record_type".into(), json!("symbol_match"));
    attributes.insert("matched".into(), json!("symbol"));
    attributes.insert("name".into(), json!(symbol.name));
    attributes.insert("symbol_kind".into(), json!(symbol.kind));
    attributes.insert("match_line".into(), json!(symbol.line));
    project_fields(fields, &mut attributes);
    Ok(resolved_for_entry(
        entry,
        ContextDeliveryLineRange {
            start: symbol.line,
            end: symbol.line,
        },
        "symbol name contains query term",
        "context_query_symbol",
        term.to_string(),
        attributes,
    ))
}

fn entry_content_match_record(
    entry: &FileEntry,
    line: usize,
    text: &str,
    term: &str,
    fields: &[String],
) -> Result<ContextDeliveryRecord> {
    let mut attributes = base_file_attributes(entry);
    attributes.insert("record_type".into(), json!("content_match"));
    attributes.insert("matched".into(), json!("content"));
    attributes.insert("match_line".into(), json!(line));
    attributes.insert("text".into(), json!(text));
    attributes.insert("encoding".into(), json!("utf-8"));
    project_fields(fields, &mut attributes);
    Ok(resolved_for_entry(
        entry,
        ContextDeliveryLineRange {
            start: line,
            end: line,
        },
        "content line contains query term",
        "context_query_content",
        format!("{}:{line}:{term}", entry.path),
        attributes,
    ))
}

fn entry_excerpt_record(
    entry: &FileEntry,
    start: usize,
    end: usize,
    text: &str,
    fields: &[String],
) -> Result<ContextDeliveryRecord> {
    let mut attributes = base_file_attributes(entry);
    attributes.insert("record_type".into(), json!("excerpt"));
    attributes.insert("start_line".into(), json!(start));
    attributes.insert("end_line".into(), json!(end));
    attributes.insert("text".into(), json!(line_slice(text, start, end)));
    attributes.insert("encoding".into(), json!("utf-8"));
    project_fields(fields, &mut attributes);
    Ok(resolved_for_entry(
        entry,
        ContextDeliveryLineRange { start, end },
        "explicit verified line excerpt",
        "context_excerpt",
        format!("{}:{start}-{end}", entry.path),
        attributes,
    ))
}

fn entry_pack_record(
    entry: &FileEntry,
    text: &str,
    fields: &[String],
) -> Result<ContextDeliveryRecord> {
    let mut attributes = base_file_attributes(entry);
    attributes.insert("record_type".into(), json!("pack_file"));
    attributes.insert("start_line".into(), json!(1));
    attributes.insert("end_line".into(), json!(entry.lines.max(1)));
    attributes.insert("text".into(), json!(text));
    attributes.insert("encoding".into(), json!("utf-8"));
    project_fields(fields, &mut attributes);
    Ok(resolved_for_entry(
        entry,
        entry_line_range(entry),
        "explicit verified file pack",
        "context_pack",
        entry.path.clone(),
        attributes,
    ))
}

fn base_file_attributes(entry: &FileEntry) -> BTreeMap<String, Value> {
    let mut attributes = BTreeMap::new();
    attributes.insert("record_type".into(), json!("file_overview"));
    attributes.insert("kind".into(), json!(entry.kind));
    attributes.insert("size".into(), json!(entry.size));
    attributes.insert("lines".into(), json!(entry.lines));
    attributes.insert("symbol_count".into(), json!(entry.symbols.len()));
    attributes
}

fn line_slice(text: &str, start: usize, end: usize) -> String {
    if text.is_empty() {
        return String::new();
    }
    text.split_inclusive('\n')
        .skip(start - 1)
        .take(end - start + 1)
        .collect::<String>()
}

fn project_fields(fields: &[String], attributes: &mut BTreeMap<String, Value>) {
    if fields.is_empty() {
        return;
    }
    attributes.retain(|field, _| fields.binary_search(field).is_ok());
}

fn resolved_for_entry(
    entry: &FileEntry,
    line_range: ContextDeliveryLineRange,
    reason: impl Into<String>,
    provenance_kind: &str,
    evidence: impl Into<String>,
    attributes: BTreeMap<String, Value>,
) -> ContextDeliveryRecord {
    ContextDeliveryRecord::Resolved(ContextDeliveryItem {
        path: entry.path.clone(),
        sha256: entry.sha256.clone(),
        line_range,
        reason: reason.into(),
        provenance: vec![ContextDeliveryProvenance {
            kind: provenance_kind.into(),
            evidence: evidence.into(),
            source_path: Some(entry.path.clone()),
            source_sha256: Some(entry.sha256.clone()),
        }],
        attributes,
    })
}

fn unresolved_for_entry(
    entry: &FileEntry,
    reason: impl Into<String>,
    provenance_kind: &str,
    evidence: impl Into<String>,
) -> ContextDeliveryRecord {
    ContextDeliveryRecord::Unresolved(ContextDeliveryUnresolved {
        reference: entry.path.clone(),
        reason: reason.into(),
        provenance: vec![ContextDeliveryProvenance {
            kind: provenance_kind.into(),
            evidence: evidence.into(),
            source_path: Some(entry.path.clone()),
            source_sha256: Some(entry.sha256.clone()),
        }],
    })
}

fn unresolved_reference(
    reference: String,
    reason: impl Into<String>,
    provenance_kind: &str,
    evidence: impl Into<String>,
) -> ContextDeliveryRecord {
    ContextDeliveryRecord::Unresolved(ContextDeliveryUnresolved {
        reference,
        reason: reason.into(),
        provenance: vec![ContextDeliveryProvenance {
            kind: provenance_kind.into(),
            evidence: evidence.into(),
            source_path: None,
            source_sha256: None,
        }],
    })
}

fn finish_context_delivery(
    snapshot_sha256: String,
    query: Value,
    sort: &str,
    mut records: Vec<ContextDeliveryRecord>,
    limit: usize,
    cursor: Option<&str>,
    budget_bytes: usize,
) -> Result<ContextDeliveryV1> {
    let mut keyed_records = records
        .drain(..)
        .map(|record| Ok((serde_json::to_vec(&record)?, record)))
        .collect::<Result<Vec<_>>>()?;
    keyed_records.sort_by(|left, right| left.0.cmp(&right.0));
    let records = keyed_records
        .into_iter()
        .map(|(_, record)| record)
        .collect::<Vec<_>>();
    build_context_delivery_page(
        &records,
        snapshot_sha256,
        context_delivery_identity(&query)?,
        context_delivery_identity(&sort)?,
        cursor,
        limit,
        budget_bytes,
    )
}

fn context_delivery_snapshot(index: &ContextIndex, raw_index_sha256: &str) -> Result<String> {
    context_delivery_identity(&(
        index.schema_version,
        &index.workspace_identity,
        &index.files,
        raw_index_sha256,
    ))
}

fn is_lower_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn validate_delivery_path(path: &str) -> Result<()> {
    let bytes = path.as_bytes();
    let windows_drive = bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':';
    if path.is_empty()
        || path.starts_with('/')
        || path.contains('\\')
        || path.contains(':')
        || windows_drive
        || std::path::Path::new(path)
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
        || path
            .split('/')
            .any(|component| component.is_empty() || component == "." || component == "..")
    {
        bail!("context delivery path must be normalized and workspace-relative: {path}");
    }
    Ok(())
}

fn validate_delivery_item(item: &ContextDeliveryItem) -> Result<()> {
    validate_delivery_path(&item.path)?;
    if !is_lower_sha256(&item.sha256) {
        bail!("context delivery item sha256 is invalid: {}", item.path);
    }
    if item.line_range.start == 0 || item.line_range.end < item.line_range.start {
        bail!("context delivery item line range is invalid: {}", item.path);
    }
    if item.reason.trim().is_empty() || item.provenance.is_empty() {
        bail!(
            "context delivery item requires reason and provenance: {}",
            item.path
        );
    }
    for provenance in &item.provenance {
        validate_delivery_provenance(provenance)?;
    }
    Ok(())
}

fn validate_delivery_provenance(provenance: &ContextDeliveryProvenance) -> Result<()> {
    if provenance.kind.trim().is_empty() || provenance.evidence.trim().is_empty() {
        bail!("context delivery provenance requires kind and evidence");
    }
    match (&provenance.source_path, &provenance.source_sha256) {
        (Some(path), Some(sha256)) => {
            validate_delivery_path(path)?;
            if !is_lower_sha256(sha256) {
                bail!("context delivery provenance source_sha256 is invalid");
            }
        }
        (None, None) => {}
        _ => bail!("context delivery provenance source path and hash must be paired"),
    }
    Ok(())
}

/// 一次刷新的统计，用来向用户报告到底做了多少实际工作。
#[derive(Debug, Clone, Serialize)]
pub struct RefreshReport {
    pub total: usize,
    /// Every current file was read through the strong hashing path.
    pub files_hashed: usize,
    pub bytes_hashed: u64,
    pub content_unchanged: usize,
    pub content_changed: usize,
    /// Legacy JSON compatibility alias for `content_unchanged`.
    pub reused: usize,
    /// Legacy JSON compatibility alias for `content_changed`; despite this old
    /// name, all `files_hashed` files were hashed.
    pub rehashed: usize,
    pub removed: usize,
    pub errors: Vec<String>,
}

fn refresh_report(
    files: &[FileEntry],
    content_unchanged: usize,
    removed: usize,
    files_hashed: usize,
    bytes_hashed: u64,
) -> Result<RefreshReport> {
    let content_changed = files
        .len()
        .checked_sub(content_unchanged)
        .context("context refresh unchanged count exceeds current files")?;
    Ok(RefreshReport {
        total: files.len(),
        files_hashed,
        bytes_hashed,
        content_unchanged,
        content_changed,
        reused: content_unchanged,
        rehashed: content_changed,
        removed,
        errors: Vec::new(),
    })
}

fn hash_work(files: &[FileEntry]) -> Result<(usize, u64)> {
    let bytes = files.iter().try_fold(0u64, |total, entry| {
        total
            .checked_add(entry.size)
            .context("context refresh byte count overflow")
    })?;
    Ok((files.len(), bytes))
}

/// 相对当前工作区状态的新鲜度，stat-only 计算，不做整树哈希。
#[derive(Debug, Clone, Serialize)]
pub struct FreshnessReport {
    pub status: String, // ready | stale | missing | incomplete
    pub changed: Vec<String>,
    pub removed: Vec<String>,
    pub added: Vec<String>,
    pub errors: Vec<String>,
}

fn index_path(root: &Path, create_parents: bool) -> Result<std::path::PathBuf> {
    state_paths::managed_state_file(root, Path::new(INDEX_STATE_RELATIVE), create_parents)
}

pub fn workspace_identity(root: &Path) -> String {
    let canonical = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    workspace_identity_from_canonical_root(&canonical)
}

/// Hash a root already canonicalized by a caller that owns a stable capture.
/// This avoids reopening that path in capture-only readiness decisions.
pub(crate) fn workspace_identity_from_canonical_root(root: &Path) -> String {
    sha256_bytes(root.to_string_lossy().as_bytes())
}

pub fn load(root: &Path) -> Result<Option<ContextIndex>> {
    read_json::<ContextIndex>(&index_path(root, false)?)
}

fn mtime_ns(metadata: &std::fs::Metadata) -> u128 {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos())
        .unwrap_or(0)
}

/// 载入缓存索引；只有缺失返回 `None`，损坏或 I/O 错误必须向上传播。
fn load_cached(root: &Path) -> Result<Option<ContextIndex>> {
    // 只有文件确实不存在才视为首次运行。损坏、权限或其它 I/O 错误
    // 必须继续向上传递，避免 refresh 静默覆盖取证状态。
    read_json::<ContextIndex>(&index_path(root, false)?)
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn validate_cached_entry(entry: &FileEntry) -> Result<()> {
    let bytes = entry.path.as_bytes();
    let windows_drive = bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':';
    if entry.path.is_empty()
        || entry.path.contains('\\')
        || windows_drive
        || Path::new(&entry.path)
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        bail!("context 索引包含不安全路径: {}", entry.path);
    }
    if !valid_sha256(&entry.sha256) {
        bail!("context 索引包含无效 sha256: {}", entry.path);
    }
    if entry.read_error.is_some() {
        bail!("context 索引包含读取失败条目: {}", entry.path);
    }
    if !matches!(
        entry.kind.as_str(),
        "source" | "test" | "docs" | "config" | "script" | "asset"
    ) {
        bail!("context 索引包含无效文件类型: {}", entry.path);
    }
    let extension = Path::new(&entry.path)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if entry.kind != classify(&entry.path, &extension) {
        bail!("context 索引文件类型与路径不一致: {}", entry.path);
    }
    Ok(())
}

fn validated_entries(entries: Vec<FileEntry>, label: &str) -> Result<BTreeMap<String, FileEntry>> {
    let mut validated = BTreeMap::new();
    for entry in entries {
        validate_cached_entry(&entry)?;
        let path = entry.path.clone();
        if validated.insert(path.clone(), entry).is_some() {
            bail!("{label} 包含重复路径: {path}");
        }
    }
    Ok(validated)
}

fn cached_entries_for_refresh(
    root: &Path,
    cached: Option<ContextIndex>,
) -> Result<BTreeMap<String, FileEntry>> {
    let Some(index) = cached else {
        return Ok(BTreeMap::new());
    };
    let compatible = index.schema_version == CONTEXT_SCHEMA_VERSION
        && index.workspace_identity == workspace_identity(root);
    let entries = validated_entries(index.files, "context 索引")?;
    if !compatible {
        return Ok(BTreeMap::new());
    }
    Ok(entries)
}

fn validate_unchanged_cached_derivations(
    cached: &BTreeMap<String, FileEntry>,
    current: &[FileEntry],
) -> Result<()> {
    for entry in current {
        let Some(old) = cached.get(&entry.path) else {
            continue;
        };
        if old.sha256 == entry.sha256
            && (old.size != entry.size
                || old.kind != entry.kind
                || old.lines != entry.lines
                || old.symbols != entry.symbols
                || old.read_error != entry.read_error)
        {
            bail!("context 索引与当前相同内容的派生字段不一致: {}", entry.path);
        }
    }
    Ok(())
}

/// 按工作区相对路径对文件分类；只吃相对路径，避免祖先目录含 "test" 造成整清单误判。
fn classify(rel: &str, extension: &str) -> String {
    let in_test_dir = rel
        .split('/')
        .any(|component| component == "test" || component == "tests" || component == "__tests__");
    let file_name = rel.rsplit('/').next().unwrap_or(rel);
    let is_source = SOURCE_EXTENSIONS.contains(&extension);
    let name_marks_test = is_source && file_name_marks_test(file_name);
    if is_source && (in_test_dir || name_marks_test) {
        "test".into()
    } else if ["md", "mdx", "rst", "txt"].contains(&extension) {
        "docs".into()
    } else if ["yaml", "yml", "json", "toml", "ini", "env"].contains(&extension) {
        "config".into()
    } else if ["sh", "ps1", "bat", "cmd"].contains(&extension) {
        "script".into()
    } else if is_source {
        "source".into()
    } else {
        "asset".into()
    }
}

/// 文件名级测试判定：整词/词缀匹配 TEST_MARKERS，不用裸子串——
/// latest.rs、inspect.rs、attest.rs 都含 "test"/"spec" 子串但不是测试。
fn file_name_marks_test(file_name: &str) -> bool {
    let lower = file_name.to_ascii_lowercase();
    let stem = lower.split('.').next().unwrap_or(&lower);
    TEST_MARKERS.iter().any(|marker| {
        stem == *marker
            || stem.starts_with(&format!("{marker}_"))
            || stem.ends_with(&format!("_{marker}"))
            || lower.contains(&format!(".{marker}."))
    })
}

fn extract_symbols(text: &str) -> Vec<Symbol> {
    let mut symbols = Vec::new();
    for (index, raw) in text.lines().enumerate() {
        let line = raw.trim_start();
        if line.starts_with("//") || line.starts_with('#') || line.starts_with('*') {
            continue;
        }
        if let Some(route) = between(line, ".route(\"", "\"") {
            symbols.push(Symbol {
                name: route,
                kind: "route".into(),
                line: index + 1,
            });
            continue;
        }
        if let Some(name) = reexport_name(line) {
            symbols.push(Symbol {
                name,
                kind: "reexport".into(),
                line: index + 1,
            });
            continue;
        }
        let mut rest = line;
        // 可见性前缀：pub、pub(crate)、pub(super)、pub(in path) 统一剥掉。
        if let Some(after) = rest.strip_prefix("pub ") {
            rest = after;
        } else if let Some(after) = rest.strip_prefix("pub(")
            && let Some(close) = after.find(')')
        {
            rest = after[close + 1..].trim_start();
        }
        for prefix in ["async ", "unsafe ", "default "] {
            if let Some(stripped) = rest.strip_prefix(prefix) {
                rest = stripped;
            }
        }
        for (prefix, kind) in [
            ("fn ", "function"),
            ("struct ", "type"),
            ("enum ", "type"),
            ("trait ", "type"),
            ("mod ", "module"),
            ("class ", "type"),
            ("def ", "function"),
        ] {
            if let Some(tail) = rest.strip_prefix(prefix) {
                let name = tail
                    .split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_'))
                    .next()
                    .unwrap_or("")
                    .to_string();
                if !name.is_empty() {
                    symbols.push(Symbol {
                        name,
                        kind: kind.into(),
                        line: index + 1,
                    });
                }
                break;
            }
        }
    }
    symbols
}

fn reexport_name(line: &str) -> Option<String> {
    let tail = line.strip_prefix("pub use ")?;
    let tail = tail.trim_end_matches(';').trim();
    let name = tail
        .rsplit("::")
        .next()
        .unwrap_or(tail)
        .trim_matches(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_'));
    (!name.is_empty()).then(|| name.to_string())
}

fn between(text: &str, start: &str, end: &str) -> Option<String> {
    let begin = text.find(start)? + start.len();
    let rest = &text[begin..];
    let stop = rest.find(end)?;
    Some(rest[..stop].to_string())
}

/// 超过此大小的文件只做流式 hash 与流式行数统计，不进内存做符号解析。
const MAX_TEXT_BYTES: u64 = 8 * 1024 * 1024;

/// Count lines without holding the file in memory. Matches `str::lines()`:
/// a trailing newline does not add an empty final line.
fn count_lines_streaming(path: &Path) -> Result<usize> {
    use std::io::{BufRead, BufReader};

    let mut reader = BufReader::new(std::fs::File::open(path)?);
    let mut buffer = Vec::new();
    let mut lines = 0usize;
    loop {
        buffer.clear();
        if reader.read_until(b'\n', &mut buffer)? == 0 {
            break;
        }
        lines += 1;
    }
    Ok(lines)
}

fn build_entry(root: &Path, path: &Path, size: u64, mtime: u128) -> Result<FileEntry> {
    ensure_source_file(root, path)?;
    let rel = relative_key(root, path);
    let extension = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    // 读取或哈希失败不能归约为空内容：refresh 必须拒绝写出可能被误判为完整的索引。
    let (bytes, streamed_hash, streamed_lines) = if size > MAX_TEXT_BYTES {
        let hash = sha256_file(path)
            .with_context(|| format!("上下文索引无法哈希文件: {}", display_path(path)))?;
        // Reporting 0 lines here inverted the `large_file` quality gate: the
        // rule blocks above a line threshold, so the very largest files were
        // the only ones it could never fire on. Line count is streamed instead;
        // symbols stay empty because that needs the whole text in memory.
        let lines = count_lines_streaming(path)
            .with_context(|| format!("上下文索引无法统计文件行数: {}", display_path(path)))?;
        (Vec::new(), Some(hash), Some(lines))
    } else {
        let bytes = std::fs::read(path)
            .with_context(|| format!("上下文索引无法读取文件: {}", display_path(path)))?;
        (bytes, None, None)
    };
    let after = std::fs::metadata(path)
        .with_context(|| format!("上下文索引无法复查文件元数据: {}", display_path(path)))?;
    ensure_source_file(root, path)?;
    if after.len() != size || mtime_ns(&after) != mtime {
        bail!("上下文索引读取期间文件发生变化: {}", display_path(path));
    }
    let sha256 = streamed_hash.unwrap_or_else(|| sha256_bytes(&bytes));
    let text = String::from_utf8_lossy(&bytes);
    let lines = streamed_lines.unwrap_or_else(|| text.lines().count());
    let kind = classify(&rel, &extension);
    let symbols = if kind == "source" || kind == "test" {
        extract_symbols(&text)
    } else {
        Vec::new()
    };
    Ok(FileEntry {
        path: rel,
        size,
        mtime_ns: mtime,
        sha256,
        kind,
        lines,
        symbols,
        read_error: None,
    })
}

/// Build the exact context entry for bytes already captured through a stable
/// no-follow handle. Readiness uses this to project one workspace capture into
/// context freshness, assets, maps, and goal baselines without another walk.
pub(crate) fn build_entry_from_captured_bytes(
    root: &Path,
    path: &Path,
    size: u64,
    mtime: u128,
    bytes: &[u8],
) -> Result<FileEntry> {
    if bytes.len() as u64 != size {
        bail!(
            "captured context file size mismatch: {} ({} != {})",
            display_path(path),
            bytes.len(),
            size
        );
    }
    let rel = relative_key(root, path);
    let extension = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let kind = classify(&rel, &extension);
    let lines = if bytes.is_empty() {
        0
    } else {
        bytes.iter().filter(|byte| **byte == b'\n').count()
            + usize::from(bytes.last() != Some(&b'\n'))
    };
    let symbols = if (kind == "source" || kind == "test") && size <= MAX_TEXT_BYTES {
        extract_symbols(&String::from_utf8_lossy(bytes))
    } else {
        Vec::new()
    };
    Ok(FileEntry {
        path: rel,
        size,
        mtime_ns: mtime,
        sha256: sha256_bytes(bytes),
        kind,
        lines,
        symbols,
        read_error: None,
    })
}

fn incomplete_freshness(error: impl Into<String>) -> FreshnessReport {
    FreshnessReport {
        status: "incomplete".into(),
        changed: Vec::new(),
        removed: Vec::new(),
        added: Vec::new(),
        errors: vec![error.into()],
    }
}

/// Validate a cached index against caller-captured complete entries. The exact
/// cached object is returned only when every persisted and derived field
/// matches; callers must not reopen the cache after this decision.
pub(crate) fn verify_index_from_capture(
    root: &Path,
    cached: Result<Option<ContextIndex>>,
    current: &[FileEntry],
) -> (FreshnessReport, Option<ContextIndex>) {
    let index = match cached {
        Ok(Some(index)) => index,
        Ok(None) => {
            return (
                FreshnessReport {
                    status: "missing".into(),
                    changed: Vec::new(),
                    removed: Vec::new(),
                    added: Vec::new(),
                    errors: Vec::new(),
                },
                None,
            );
        }
        Err(error) => {
            return (
                incomplete_freshness(format!("无法安全读取 context 状态: {error:#}")),
                None,
            );
        }
    };
    if index.schema_version != CONTEXT_SCHEMA_VERSION
        || index.workspace_identity != workspace_identity(root)
    {
        return (
            FreshnessReport {
                status: "missing".into(),
                changed: Vec::new(),
                removed: Vec::new(),
                added: Vec::new(),
                errors: vec!["context schema/workspace identity 不匹配".into()],
            },
            None,
        );
    }

    let mut errors = Vec::new();
    let mut cached_by_path = BTreeMap::new();
    for entry in &index.files {
        if cached_by_path.insert(entry.path.as_str(), entry).is_some() {
            errors.push(format!("context 索引包含重复路径: {}", entry.path));
        }
        if entry.read_error.is_some() {
            errors.push(format!("context 索引包含读取失败条目: {}", entry.path));
        }
    }
    let mut current_by_path = BTreeMap::new();
    for entry in current {
        if current_by_path.insert(entry.path.as_str(), entry).is_some() {
            errors.push(format!(
                "当前 workspace capture 包含重复路径: {}",
                entry.path
            ));
        }
    }

    let mut changed = Vec::new();
    let mut added = Vec::new();
    for (path, entry) in &current_by_path {
        match cached_by_path.get(path) {
            Some(cached) if **cached == **entry => {}
            Some(_) => changed.push((*path).to_string()),
            None => added.push((*path).to_string()),
        }
    }
    let mut removed = cached_by_path
        .keys()
        .filter(|path| !current_by_path.contains_key(**path))
        .map(|path| (*path).to_string())
        .collect::<Vec<_>>();
    changed.sort();
    added.sort();
    removed.sort();
    let status = if !errors.is_empty() {
        "incomplete"
    } else if changed.is_empty() && added.is_empty() && removed.is_empty() {
        "ready"
    } else {
        "stale"
    };
    let ready = status == "ready";
    (
        FreshnessReport {
            status: status.into(),
            changed,
            removed,
            added,
            errors,
        },
        ready.then_some(index),
    )
}

/// Publish a context index from an already complete workspace capture. This is
/// the readiness `--refresh-context` path: refresh itself is the first decision
/// walk rather than an extra fifth walk before a two-round finish.
pub(crate) fn refresh_from_capture(
    root: &Path,
    files: Vec<FileEntry>,
    captured_cached: Result<Option<ContextIndex>>,
) -> Result<(ContextIndex, RefreshReport)> {
    let identity = workspace_identity(root);
    let cached = cached_entries_for_refresh(root, captured_cached?)?;
    let _current = validated_entries(files.clone(), "当前 workspace capture")?;
    validate_unchanged_cached_derivations(&cached, &files)?;
    let current_paths = files
        .iter()
        .map(|entry| entry.path.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    let reused = files
        .iter()
        .filter(|entry| {
            cached.get(&entry.path).is_some_and(|old| {
                old.sha256 == entry.sha256 && old.read_error.is_none() && entry.read_error.is_none()
            })
        })
        .count();
    let removed = cached
        .keys()
        .filter(|path| !current_paths.contains(path.as_str()))
        .count();
    let (files_hashed, bytes_hashed) = hash_work(&files)?;
    let report = refresh_report(&files, reused, removed, files_hashed, bytes_hashed)?;
    let index = ContextIndex {
        schema_version: CONTEXT_SCHEMA_VERSION,
        generated_at: now_iso(),
        workspace: display_path(root),
        workspace_identity: identity,
        files,
    };
    write_json(&index_path(root, true)?, &index)?;
    Ok((index, report))
}

/// 在读取/哈希前后确认候选文件仍是工作区内的普通文件。遍历器不跟随
/// 链接，但路径可能在遍历完成后被替换；拒绝链接/reparse，避免把工作区外
/// 内容纳入上下文索引。
pub(crate) fn ensure_source_file(root: &Path, path: &Path) -> Result<()> {
    let relative = path.strip_prefix(root).with_context(|| {
        format!(
            "上下文索引文件不属于工作区: {} under {}",
            display_path(path),
            display_path(root)
        )
    })?;
    let components = relative.components().collect::<Vec<_>>();
    if components.is_empty()
        || components
            .iter()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        bail!("上下文索引拒绝不安全相对路径: {}", display_path(path));
    }
    let mut current = root.to_path_buf();
    for (index, component) in components.iter().enumerate() {
        let Component::Normal(name) = component else {
            bail!("上下文索引拒绝不安全相对路径: {}", display_path(path));
        };
        current.push(name);
        let metadata = std::fs::symlink_metadata(&current)
            .with_context(|| format!("上下文索引无法读取文件元数据: {}", display_path(&current)))?;
        if is_link_or_reparse(&metadata) {
            bail!(
                "上下文索引拒绝链接/reparse 路径: {}",
                display_path(&current)
            );
        }
        if index + 1 == components.len() {
            if !metadata.file_type().is_file() {
                bail!("上下文索引拒绝非普通文件: {}", display_path(&current));
            }
        } else if !metadata.file_type().is_dir() {
            bail!("上下文索引路径组件不是目录: {}", display_path(&current));
        }
    }
    let workspace = root
        .canonicalize()
        .with_context(|| format!("无法规范化工作区根: {}", display_path(root)))?;
    let canonical = path
        .canonicalize()
        .with_context(|| format!("无法规范化上下文文件: {}", display_path(path)))?;
    if !canonical.starts_with(&workspace) {
        bail!(
            "上下文索引文件逃逸工作区: {} -> {}",
            display_path(path),
            display_path(&canonical)
        );
    }
    Ok(())
}

/// Read a file named by an already strongly verified index and bind the exact
/// bytes consumed by a downstream map to that same [`FileEntry`].  This closes
/// the gap where a caller validated the cache, then reopened a changed source or
/// followed a newly substituted parent link while deriving trusted conclusions.
pub(crate) fn read_verified_file(root: &Path, entry: &FileEntry) -> Result<Vec<u8>> {
    let relative = Path::new(&entry.path);
    if relative.as_os_str().is_empty()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        bail!("上下文索引条目含不安全路径: {}", entry.path);
    }
    let path = root.join(relative);
    ensure_source_file(root, &path)?;
    let before = std::fs::symlink_metadata(&path)
        .with_context(|| format!("无法读取已验证上下文文件元数据: {}", entry.path))?;
    let bytes = std::fs::read(&path)
        .with_context(|| format!("无法读取已验证上下文文件: {}", entry.path))?;
    ensure_source_file(root, &path)?;
    let after = std::fs::symlink_metadata(&path)
        .with_context(|| format!("无法复查已验证上下文文件元数据: {}", entry.path))?;
    let actual_hash = sha256_bytes(&bytes);
    if before.len() != after.len()
        || bytes.len() as u64 != entry.size
        || actual_hash != entry.sha256
    {
        bail!(
            "上下文文件在索引验证后发生变化: {} (size {} != {} or sha256 {} != {})",
            entry.path,
            bytes.len(),
            entry.size,
            actual_hash,
            entry.sha256
        );
    }
    Ok(bytes)
}

fn capture_current_entries(root: &Path) -> Result<Vec<FileEntry>> {
    let mut files = Vec::new();
    for path in workspace_files_checked(root)? {
        let metadata = std::fs::metadata(&path)
            .with_context(|| format!("上下文索引无法读取文件元数据: {}", display_path(&path)))?;
        files.push(build_entry(
            root,
            &path,
            metadata.len(),
            mtime_ns(&metadata),
        )?);
    }
    Ok(files)
}

/// 刷新索引：对每个当前文件完成两个连续且相等的强内容捕获，再发布第二轮结果。
/// 缓存只用于报告内容未变/变化数量；它从不跳过哈希。
pub fn refresh(root: &Path) -> Result<(ContextIndex, RefreshReport)> {
    refresh_with_between_capture(root, || {})
}

fn refresh_with_between_capture<F>(root: &Path, between: F) -> Result<(ContextIndex, RefreshReport)>
where
    F: FnOnce(),
{
    let identity = workspace_identity(root);
    let cached = cached_entries_for_refresh(root, load_cached(root)?)?;

    let first = capture_current_entries(root)?;
    between();
    let files = capture_current_entries(root)?;
    if first != files {
        bail!("context refresh 的两轮强内容捕获不一致；工作区在刷新期间发生变化");
    }
    validate_unchanged_cached_derivations(&cached, &files)?;

    let mut content_unchanged = 0usize;
    let mut present = std::collections::BTreeSet::new();
    for entry in &files {
        present.insert(entry.path.clone());
        if cached.get(&entry.path).is_some_and(|old| {
            old.sha256 == entry.sha256 && old.read_error.is_none() && entry.read_error.is_none()
        }) {
            content_unchanged += 1;
        }
    }

    let removed = cached.keys().filter(|key| !present.contains(*key)).count();
    let (first_files, first_bytes) = hash_work(&first)?;
    let (terminal_files, terminal_bytes) = hash_work(&files)?;
    let files_hashed = first_files
        .checked_add(terminal_files)
        .context("context refresh file count overflow")?;
    let bytes_hashed = first_bytes
        .checked_add(terminal_bytes)
        .context("context refresh byte count overflow")?;
    let report = refresh_report(
        &files,
        content_unchanged,
        removed,
        files_hashed,
        bytes_hashed,
    )?;
    let index = ContextIndex {
        schema_version: CONTEXT_SCHEMA_VERSION,
        generated_at: now_iso(),
        workspace: display_path(root),
        workspace_identity: identity,
        files,
    };
    write_json(&index_path(root, true)?, &index)?;
    Ok((index, report))
}

/// 只做 stat-only 新鲜度检查，不重建、不整树哈希。缓存损坏或缺失时报 `missing`。
pub fn freshness(root: &Path) -> FreshnessReport {
    let cached = match load_cached(root) {
        Ok(Some(cached)) => cached,
        Ok(None) => {
            return FreshnessReport {
                status: "missing".into(),
                changed: Vec::new(),
                removed: Vec::new(),
                added: Vec::new(),
                errors: Vec::new(),
            };
        }
        Err(error) => {
            return FreshnessReport {
                status: "incomplete".into(),
                changed: Vec::new(),
                removed: Vec::new(),
                added: Vec::new(),
                errors: vec![format!("无法安全读取 context 状态: {error:#}")],
            };
        }
    };
    if cached.schema_version != CONTEXT_SCHEMA_VERSION
        || cached.workspace_identity != workspace_identity(root)
    {
        return FreshnessReport {
            status: "missing".into(),
            changed: Vec::new(),
            removed: Vec::new(),
            added: Vec::new(),
            errors: vec!["context schema/workspace identity 不匹配".into()],
        };
    }
    let cached_map: BTreeMap<_, _> = cached
        .files
        .into_iter()
        .map(|entry| (entry.path.clone(), entry))
        .collect();

    let mut changed = Vec::new();
    let mut added = Vec::new();
    let mut present = std::collections::BTreeSet::new();
    let mut errors = Vec::new();
    let paths = match workspace_files_checked(root) {
        Ok(paths) => paths,
        Err(error) => {
            return FreshnessReport {
                status: "incomplete".into(),
                changed: Vec::new(),
                removed: Vec::new(),
                added: Vec::new(),
                errors: vec![format!("工作区遍历失败: {error:#}")],
            };
        }
    };
    for path in paths {
        let metadata = match std::fs::metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) => {
                errors.push(format!("{}: {error}", display_path(&path)));
                continue;
            }
        };
        let rel = relative_key(root, &path);
        present.insert(rel.clone());
        match cached_map.get(&rel) {
            Some(entry) => {
                if entry.read_error.is_some()
                    || entry.size != metadata.len()
                    || entry.mtime_ns != mtime_ns(&metadata)
                {
                    changed.push(rel);
                }
            }
            None => added.push(rel),
        }
    }
    let removed: Vec<String> = cached_map
        .keys()
        .filter(|key| !present.contains(*key))
        .cloned()
        .collect();

    let status =
        if !errors.is_empty() || cached_map.values().any(|entry| entry.read_error.is_some()) {
            "incomplete"
        } else if changed.is_empty() && added.is_empty() && removed.is_empty() {
            "ready"
        } else {
            "stale"
        };
    FreshnessReport {
        status: status.into(),
        changed,
        removed,
        added,
        errors,
    }
}

/// 强新鲜度：用于 map、standard/release 检查。它从当前工作区遍历结果重新构造
/// 每个完整 [`FileEntry`]，同时复核内容摘要和 kind/lines/symbols 等派生字段。
///
/// 缓存里的 `path` 只是待比较的数据，绝不能反过来驱动文件读取：状态文件可被
/// 手工编辑，直接 `root.join(cached.path)` 会形成工作区外读取，也会让伪造的派生
/// 字段在 hash 不变时进入 map/quality 信任链。
/// Return the exact cached index object that passed the strong content check.
///
/// Callers that derive trusted conclusions from the index must use this API
/// instead of checking [`strong_freshness`] and then reopening the state file:
/// the latter creates a validate-then-reopen window in which a different cache
/// can be substituted after validation.
pub fn verified_index(root: &Path) -> Result<ContextIndex> {
    let (report, index) = strong_freshness_with_index(root);
    if report.status != "ready" {
        let mut details = Vec::new();
        if !report.changed.is_empty() {
            details.push(format!("changed={:?}", report.changed));
        }
        if !report.added.is_empty() {
            details.push(format!("added={:?}", report.added));
        }
        if !report.removed.is_empty() {
            details.push(format!("removed={:?}", report.removed));
        }
        if !report.errors.is_empty() {
            details.push(format!("errors={:?}", report.errors));
        }
        bail!(
            "上下文索引不是 ready（当前: {}）。先运行 `rayman context refresh`。{}",
            report.status,
            if details.is_empty() {
                String::new()
            } else {
                format!(" {}", details.join(" "))
            }
        );
    }
    index.ok_or_else(|| anyhow::anyhow!("上下文索引缺失。先运行 `rayman context refresh`。"))
}

pub fn strong_freshness(root: &Path) -> FreshnessReport {
    strong_freshness_with_index(root).0
}

fn strong_freshness_with_index(root: &Path) -> (FreshnessReport, Option<ContextIndex>) {
    let index = match load_cached(root) {
        Ok(Some(index)) => index,
        Ok(None) => {
            return (
                FreshnessReport {
                    status: "missing".into(),
                    changed: Vec::new(),
                    removed: Vec::new(),
                    added: Vec::new(),
                    errors: Vec::new(),
                },
                None,
            );
        }
        Err(error) => {
            return (
                FreshnessReport {
                    status: "incomplete".into(),
                    changed: Vec::new(),
                    removed: Vec::new(),
                    added: Vec::new(),
                    errors: vec![format!("无法安全读取 context 状态: {error:#}")],
                },
                None,
            );
        }
    };
    if index.schema_version != CONTEXT_SCHEMA_VERSION
        || index.workspace_identity != workspace_identity(root)
    {
        return (
            FreshnessReport {
                status: "missing".into(),
                changed: Vec::new(),
                removed: Vec::new(),
                added: Vec::new(),
                errors: vec!["context schema/workspace identity 不匹配".into()],
            },
            None,
        );
    }

    let mut errors = Vec::new();
    let mut cached = BTreeMap::new();
    for entry in &index.files {
        if cached.insert(entry.path.as_str(), entry).is_some() {
            errors.push(format!("context 索引包含重复路径: {}", entry.path));
        }
        if entry.read_error.is_some() {
            errors.push(format!("context 索引包含读取失败条目: {}", entry.path));
        }
    }
    let paths = match workspace_files_checked(root) {
        Ok(paths) => paths,
        Err(error) => {
            errors.push(format!("工作区遍历失败: {error:#}"));
            return (
                FreshnessReport {
                    status: "incomplete".into(),
                    changed: Vec::new(),
                    removed: Vec::new(),
                    added: Vec::new(),
                    errors,
                },
                None,
            );
        }
    };
    let mut changed = Vec::new();
    let mut added = Vec::new();
    let mut present = std::collections::BTreeSet::new();
    for path in paths {
        let metadata = match std::fs::metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) => {
                errors.push(format!("{}: {error}", display_path(&path)));
                continue;
            }
        };
        let rel = relative_key(root, &path);
        present.insert(rel.clone());
        match build_entry(root, &path, metadata.len(), mtime_ns(&metadata)) {
            Ok(current)
                if cached
                    .get(rel.as_str())
                    .is_some_and(|entry| **entry == current) => {}
            Ok(_) if cached.contains_key(rel.as_str()) => changed.push(rel),
            Ok(_) => added.push(rel),
            Err(error) => errors.push(format!("{}: {error:#}", display_path(&path))),
        }
    }
    let removed = cached
        .keys()
        .filter(|path| !present.contains(**path))
        .map(|path| (*path).to_string())
        .collect::<Vec<_>>();
    changed.sort();
    changed.dedup();
    added.sort();
    added.dedup();
    let status = if !errors.is_empty() {
        "incomplete"
    } else if changed.is_empty() && added.is_empty() && removed.is_empty() {
        "ready"
    } else {
        "stale"
    };
    let ready = status == "ready";
    // `cached` borrows `index`; end that borrow before returning the exact
    // validated object to the caller.
    drop(cached);
    (
        FreshnessReport {
            status: status.into(),
            changed,
            removed,
            added,
            errors,
        },
        ready.then_some(index),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn touch(path: &Path, body: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
    }

    /// Files above `MAX_TEXT_BYTES` are hashed without being read into memory.
    /// Reporting them as 0 lines inverted the `large_file` quality gate, which
    /// blocks above a line threshold: the very largest files were the only
    /// ones it could never fire on.
    #[test]
    fn oversized_files_report_their_real_line_count() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let line = "pub fn filler() {} // padding to exceed the in-memory limit\n";
        let repeats = (MAX_TEXT_BYTES as usize / line.len()) + 64;
        touch(&root.join("src/huge.rs"), &line.repeat(repeats));
        touch(&root.join("src/small.rs"), "pub fn a() {}\npub fn b() {}\n");

        let (index, _) = refresh(root).unwrap();
        let huge = index
            .files
            .iter()
            .find(|file| file.path == "src/huge.rs")
            .unwrap();
        assert!(huge.size > MAX_TEXT_BYTES, "fixture must exceed the limit");
        assert_eq!(huge.lines, repeats, "line count must be streamed, not zero");
        // Symbols still need the whole text in memory, so they stay empty.
        assert!(huge.symbols.is_empty());

        let small = index
            .files
            .iter()
            .find(|file| file.path == "src/small.rs")
            .unwrap();
        assert_eq!(small.lines, 2, "the in-memory path is unchanged");
    }

    #[test]
    fn refresh_hashes_every_file_and_reports_content_deltas_honestly() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        touch(&root.join("src/a.rs"), "pub fn a() {}");
        touch(&root.join("src/b.rs"), "pub fn b() {}");

        let (_, first) = refresh(root).unwrap();
        assert_eq!(first.total, 2);
        assert_eq!(first.files_hashed, 4);
        assert_eq!(first.bytes_hashed, 52);
        assert_eq!(first.content_changed, 2);
        assert_eq!(first.content_unchanged, 0);
        assert_eq!(first.rehashed, first.content_changed);
        assert_eq!(first.reused, first.content_unchanged);
        let first_json = serde_json::to_value(&first).unwrap();
        assert_eq!(first_json["files_hashed"], 4);
        assert_eq!(first_json["bytes_hashed"], 52);
        assert_eq!(first_json["content_changed"], 2);
        assert_eq!(first_json["content_unchanged"], 0);
        assert_eq!(first_json["rehashed"], first_json["content_changed"]);
        assert_eq!(first_json["reused"], first_json["content_unchanged"]);

        // 不改任何文件：第二次仍完成两轮强哈希，但内容分类为全部未变。
        let (_, second) = refresh(root).unwrap();
        assert_eq!(second.files_hashed, 4);
        assert_eq!(second.bytes_hashed, 52);
        assert_eq!(second.content_unchanged, 2);
        assert_eq!(second.content_changed, 0);

        // 改一个文件：仍完成两轮全文件强哈希，内容分类只有一个变化。
        touch(&root.join("src/a.rs"), "pub fn a() { /* changed */ }");
        let (_, third) = refresh(root).unwrap();
        assert_eq!(third.files_hashed, 4);
        assert_eq!(third.content_changed, 1);
        assert_eq!(third.content_unchanged, 1);
        assert_eq!(third.rehashed, third.content_changed);
        assert_eq!(third.reused, third.content_unchanged);
    }

    #[test]
    fn compatible_cache_validation_rejects_semantic_corruption() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        touch(&root.join("src/a.rs"), "pub fn a() {}");
        let index = refresh(root).unwrap().0;

        let mut duplicate = index.files.clone();
        duplicate.push(duplicate[0].clone());
        assert!(validated_entries(duplicate, "cache").is_err());

        let mut read_error = index.files.clone();
        read_error[0].read_error = Some("forged failure".into());
        assert!(validated_entries(read_error, "cache").is_err());

        let mut unsafe_path = index.files.clone();
        unsafe_path[0].path = "../outside.rs".into();
        assert!(validated_entries(unsafe_path, "cache").is_err());

        let mut bad_hash = index.files;
        bad_hash[0].sha256 = "not-a-hash".into();
        assert!(validated_entries(bad_hash, "cache").is_err());

        let mut bad_kind = refresh(root).unwrap().0.files;
        bad_kind[0].kind = "asset".into();
        assert!(validated_entries(bad_kind, "cache").is_err());

        let mut incompatible = refresh(root).unwrap().0;
        incompatible.schema_version = 0;
        incompatible.files[0].read_error = Some("must not be washed away".into());
        assert!(cached_entries_for_refresh(root, Some(incompatible)).is_err());
    }

    #[test]
    fn refresh_entrypoints_preserve_a_semantically_invalid_cache() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        touch(&root.join("src/a.rs"), "pub fn a() {}");
        let (valid, _) = refresh(root).unwrap();
        let mut forged = valid.clone();
        forged.files.push(forged.files[0].clone());
        let path = root.join(INDEX_RELATIVE_PATH);
        write_json(&path, &forged).unwrap();
        let before = fs::read(&path).unwrap();

        assert!(refresh(root).is_err());
        assert_eq!(fs::read(&path).unwrap(), before);

        let current = capture_current_entries(root).unwrap();
        assert!(refresh_from_capture(root, current, Ok(Some(forged))).is_err());
        assert_eq!(fs::read(&path).unwrap(), before);

        let mut forged_derivations = valid;
        forged_derivations.files[0].lines = 0;
        forged_derivations.files[0].symbols.clear();
        write_json(&path, &forged_derivations).unwrap();
        let before = fs::read(&path).unwrap();

        assert!(refresh(root).is_err());
        assert_eq!(fs::read(&path).unwrap(), before);

        let current = capture_current_entries(root).unwrap();
        assert!(refresh_from_capture(root, current, Ok(Some(forged_derivations))).is_err());
        assert_eq!(fs::read(&path).unwrap(), before);
    }

    #[test]
    fn refresh_rejects_same_stat_drift_between_strong_capture_rounds() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let path = root.join("src/a.rs");
        touch(&path, "fn alpha() {}\n");
        refresh(root).unwrap();
        let index_path = root.join(INDEX_RELATIVE_PATH);
        let before = fs::read(&index_path).unwrap();
        let original = fs::metadata(&path).unwrap().modified().unwrap();

        let error = refresh_with_between_capture(root, || {
            fs::write(&path, "fn bravo() {}\n").unwrap();
            let file = fs::File::options().write(true).open(&path).unwrap();
            file.set_times(fs::FileTimes::new().set_modified(original))
                .unwrap();
        })
        .unwrap_err()
        .to_string();
        assert!(error.contains("两轮强内容捕获不一致"), "{error}");
        assert_eq!(fs::read(&index_path).unwrap(), before);
    }

    #[test]
    fn classify_uses_relative_path_and_extracts_symbols() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        touch(&root.join("src/main.rs"), "pub fn main() {}\nstruct Foo;");
        touch(&root.join("tests/it.rs"), "fn check() {}");
        touch(&root.join("README.md"), "# doc");

        let (index, _) = refresh(root).unwrap();
        let by_path: BTreeMap<_, _> = index
            .files
            .iter()
            .map(|entry| (entry.path.as_str(), entry))
            .collect();
        assert_eq!(by_path["src/main.rs"].kind, "source");
        assert_eq!(by_path["tests/it.rs"].kind, "test");
        assert_eq!(by_path["README.md"].kind, "docs");
        assert!(
            by_path["src/main.rs"]
                .symbols
                .iter()
                .any(|symbol| symbol.name == "main" && symbol.kind == "function")
        );
    }

    #[test]
    fn classify_extracts_reexport_symbols() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        touch(
            &root.join("src/lib.rs"),
            "pub use crate::exports::display_path;\n",
        );

        let (index, _) = refresh(root).unwrap();
        let file = index
            .files
            .iter()
            .find(|entry| entry.path == "src/lib.rs")
            .unwrap();
        assert!(
            file.symbols
                .iter()
                .any(|symbol| { symbol.name == "display_path" && symbol.kind == "reexport" })
        );
    }

    #[test]
    fn classify_does_not_treat_substring_names_as_tests() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        // 含 test/spec 子串但不是测试的普通源文件。
        touch(&root.join("src/latest.rs"), "pub fn latest() {}");
        touch(&root.join("src/inspect.rs"), "pub fn inspect() {}");
        touch(&root.join("src/attest.rs"), "pub fn attest() {}");
        // 真正的测试命名习惯仍要识别。
        touch(&root.join("src/parser_test.rs"), "#[test]\nfn t() {}");
        touch(&root.join("src/test_utils.rs"), "pub fn helper() {}");
        touch(&root.join("src/app.spec.ts"), "it('x', () => {});");

        let (index, _) = refresh(root).unwrap();
        let kind = |path: &str| {
            index
                .files
                .iter()
                .find(|entry| entry.path == path)
                .unwrap()
                .kind
                .clone()
        };
        assert_eq!(kind("src/latest.rs"), "source");
        assert_eq!(kind("src/inspect.rs"), "source");
        assert_eq!(kind("src/attest.rs"), "source");
        assert_eq!(kind("src/parser_test.rs"), "test");
        assert_eq!(kind("src/test_utils.rs"), "test");
        assert_eq!(kind("src/app.spec.ts"), "test");
    }

    #[test]
    fn docs_inside_tests_directory_do_not_create_a_test_anchor() {
        assert_eq!(classify("tests/README.md", "md"), "docs");
        assert_eq!(classify("tests/case.rs", "rs"), "test");
    }

    #[test]
    fn extract_symbols_handles_scoped_visibility() {
        let symbols = extract_symbols(
            "pub(crate) fn helper() {}\npub(super) struct Inner;\npub(in crate::x) enum E {}\n",
        );
        let names: Vec<_> = symbols.iter().map(|symbol| symbol.name.as_str()).collect();
        assert!(names.contains(&"helper"));
        assert!(names.contains(&"Inner"));
        assert!(names.contains(&"E"));
    }

    #[test]
    fn refresh_tolerates_non_utf8_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        touch(&root.join("src/a.rs"), "pub fn a() {}");
        // GBK 编码字节：非法 UTF-8，不得让 refresh 整体失败。
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/gbk.rs"), [0xD6u8, 0xD0, 0xCE, 0xC4, b'\n']).unwrap();

        let (index, _) = refresh(root).unwrap();
        assert!(index.files.iter().any(|entry| entry.path == "src/gbk.rs"));
    }

    #[test]
    fn freshness_is_missing_then_ready_then_stale() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        touch(&root.join("src/a.rs"), "fn a() {}");
        assert_eq!(freshness(root).status, "missing");
        refresh(root).unwrap();
        assert_eq!(freshness(root).status, "ready");
        touch(&root.join("src/b.rs"), "fn b() {}");
        let report = freshness(root);
        assert_eq!(report.status, "stale");
        assert_eq!(report.added, vec!["src/b.rs".to_string()]);
    }

    #[test]
    fn strong_freshness_detects_same_stat_content_replacement() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let path = root.join("src/a.rs");
        touch(&path, "fn alpha() {}\n");
        refresh(root).unwrap();
        let original = fs::metadata(&path).unwrap().modified().unwrap();
        // Same byte length, then restore mtime: stat-only UI cannot prove identity.
        fs::write(&path, "fn bravo() {}\n").unwrap();
        let file = fs::File::options().write(true).open(&path).unwrap();
        file.set_times(fs::FileTimes::new().set_modified(original))
            .unwrap();
        assert_eq!(freshness(root).status, "ready");
        assert_eq!(strong_freshness(root).status, "stale");
    }

    #[test]
    fn strong_freshness_rejects_forged_derived_fields() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        touch(&root.join("src/lib.rs"), "pub fn answer() -> u32 { 42 }\n");
        let (mut index, _) = refresh(root).unwrap();
        let entry = index
            .files
            .iter_mut()
            .find(|entry| entry.path == "src/lib.rs")
            .unwrap();
        entry.kind = "asset".into();
        entry.lines = 0;
        entry.symbols.clear();
        write_json(&root.join(INDEX_RELATIVE_PATH), &index).unwrap();

        let report = strong_freshness(root);
        assert_eq!(report.status, "stale", "errors={:?}", report.errors);
        assert_eq!(report.changed, vec!["src/lib.rs".to_string()]);
    }

    #[test]
    fn strong_freshness_never_reads_paths_supplied_only_by_cache() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        touch(&root.join("src/lib.rs"), "pub fn answer() -> u32 { 42 }\n");
        let (mut index, _) = refresh(root).unwrap();
        index.files.push(FileEntry {
            path: "../outside-do-not-read.rs".into(),
            size: 1,
            mtime_ns: 1,
            sha256: "0".repeat(64),
            kind: "source".into(),
            lines: 1,
            symbols: Vec::new(),
            read_error: None,
        });
        write_json(&root.join(INDEX_RELATIVE_PATH), &index).unwrap();

        let report = strong_freshness(root);
        assert_eq!(report.status, "stale");
        assert!(report.errors.is_empty(), "errors={:?}", report.errors);
        assert_eq!(
            report.removed,
            vec!["../outside-do-not-read.rs".to_string()]
        );
    }

    #[test]
    fn strong_freshness_rejects_duplicate_cached_paths() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        touch(&root.join("src/lib.rs"), "pub fn answer() -> u32 { 42 }\n");
        let (mut index, _) = refresh(root).unwrap();
        index.files.push(index.files[0].clone());
        write_json(&root.join(INDEX_RELATIVE_PATH), &index).unwrap();

        let report = strong_freshness(root);
        assert_eq!(report.status, "incomplete");
        assert!(
            report.errors.iter().any(|error| error.contains("重复路径")),
            "errors={:?}",
            report.errors
        );
    }

    #[test]
    fn verified_index_is_the_same_object_that_passed_validation() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        touch(&root.join("src/lib.rs"), "pub fn answer() -> u32 { 42 }\n");
        refresh(root).unwrap();

        let validated = verified_index(root).unwrap();
        let mut replacement = load(root).unwrap().unwrap();
        replacement.files[0].path = "../outside.rs".into();
        write_json(&root.join(INDEX_RELATIVE_PATH), &replacement).unwrap();

        assert_eq!(validated.files.len(), 1);
        assert_eq!(validated.files[0].path, "src/lib.rs");
        assert_ne!(strong_freshness(root).status, "ready");
    }

    #[test]
    fn verified_index_stale_error_names_the_changed_paths() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        touch(&root.join("src/lib.rs"), "pub fn answer() -> u32 { 42 }\n");
        refresh(root).unwrap();
        touch(&root.join("src/new.rs"), "pub fn new_item() {}\n");

        let error = verified_index(root).unwrap_err().to_string();
        assert!(error.contains("added"), "{error}");
        assert!(error.contains("src/new.rs"), "{error}");
    }

    #[test]
    fn verified_file_read_rejects_post_validation_byte_drift() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        touch(&root.join("src/lib.rs"), "pub fn alpha() {}\n");
        let index = refresh(root).unwrap().0;
        let entry = index
            .files
            .iter()
            .find(|entry| entry.path == "src/lib.rs")
            .unwrap();
        touch(&root.join("src/lib.rs"), "pub fn bravo() {}\n");

        let error = read_verified_file(root, entry).unwrap_err().to_string();
        assert!(error.contains("索引验证后发生变化"), "{error}");
    }

    #[cfg(unix)]
    #[test]
    fn verified_file_read_rejects_a_substituted_parent_symlink() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let root = dir.path();
        touch(&root.join("src/lib.rs"), "pub fn answer() {}\n");
        let index = refresh(root).unwrap().0;
        let entry = index
            .files
            .iter()
            .find(|entry| entry.path == "src/lib.rs")
            .unwrap();
        fs::remove_file(root.join("src/lib.rs")).unwrap();
        fs::remove_dir(root.join("src")).unwrap();
        touch(&outside.path().join("lib.rs"), "pub fn answer() {}\n");
        symlink(outside.path(), root.join("src")).unwrap();

        let error = read_verified_file(root, entry).unwrap_err().to_string();
        assert!(error.contains("链接/reparse"), "{error}");
    }

    #[test]
    fn corrupt_cache_is_preserved_as_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        touch(&root.join("src/a.rs"), "fn a() {}");
        refresh(root).unwrap();
        fs::write(root.join(INDEX_RELATIVE_PATH), "{ corrupt").unwrap();
        // 损坏状态不能被静默降级并覆盖；只读状态报告为 incomplete，刷新返回错误。
        assert_eq!(freshness(root).status, "incomplete");
        assert!(refresh(root).is_err());
        assert_eq!(
            fs::read_to_string(root.join(INDEX_RELATIVE_PATH)).unwrap(),
            "{ corrupt"
        );
    }

    #[test]
    fn refresh_refuses_incomplete_workspace_walk() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing-workspace");
        assert!(refresh(&missing).is_err());
    }

    #[test]
    fn build_entry_refuses_a_non_file_read_target() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let target = root.join("not-a-file");
        fs::create_dir_all(&target).unwrap();
        let metadata = fs::metadata(&target).unwrap();
        assert!(build_entry(root, &target, metadata.len(), mtime_ns(&metadata)).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn build_entry_refuses_a_symlink_to_a_file_outside_workspace() {
        use std::os::unix::fs::symlink;

        let workspace = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let outside_file = outside.path().join("secret.rs");
        fs::write(&outside_file, "fn secret() {}").unwrap();
        let target = workspace.path().join("secret.rs");
        symlink(&outside_file, &target).unwrap();
        let metadata = fs::metadata(&target).unwrap();
        assert!(
            build_entry(
                workspace.path(),
                &target,
                metadata.len(),
                mtime_ns(&metadata)
            )
            .is_err()
        );
    }
}
