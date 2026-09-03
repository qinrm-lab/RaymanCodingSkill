//! rayman：RaymanCodingSkill 的 load-bearing v2 核心。
//! 模块覆盖 activation、context/map、goal/authority、recovery、host integration、
//! managed temp 与 trusted update；各能力的完成边界由共享 workflow contract 定义。

pub mod assets;
pub const CLI_CONTRACT: &str = "rayman-cli-contract-v18";
pub const CLI_VERSION: &str = env!("CARGO_PKG_VERSION");

pub mod autosave;
pub mod checkpoint;
pub mod codex_hook;
pub mod codex_host;
pub mod context;
pub mod execution_context;
pub mod goal;
pub mod hash;
pub mod map;
pub mod readiness_state;
pub mod source_state;
pub mod state_lock;
pub mod state_paths;
pub mod temp;
pub mod toolchain;
pub mod update;
pub mod walk;
pub mod workspace;

mod file_io;
pub mod pathfmt;
pub mod timefmt;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// 工作区根：从当前目录向上查找含 `.RaymanCodingSkill/` 或 `.git` 的最近祖先，
/// 找不到则回退到当前目录。修复“从子目录运行会另建一份状态、分裂工作区”的问题。
/// `.git` 用 exists()：git worktree / submodule 的 `.git` 是文件而非目录。
pub fn workspace_root() -> Result<PathBuf> {
    let cwd = std::env::current_dir().context("无法读取当前目录")?;
    let mut dir = cwd.as_path();
    loop {
        if is_workspace_marker(dir)? {
            return Ok(dir.to_path_buf());
        }
        match dir.parent() {
            Some(parent) => dir = parent,
            None => return Ok(cwd),
        }
    }
}

fn is_workspace_marker(dir: &Path) -> Result<bool> {
    // A linked state root must not become the authority boundary that selects
    // a workspace.  `managed_state_root` verifies both symlinks and Windows
    // reparse points before reporting a marker.
    Ok(state_paths::managed_state_root(dir, false)?.is_some() || dir.join(".git").exists())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::*;
    use std::collections::BTreeMap;

    #[test]
    fn workspace_marker_rejects_an_invalid_state_root() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".RaymanCodingSkill"), "not a directory").unwrap();
        assert!(is_workspace_marker(dir.path()).is_err());
    }
    #[test]
    fn context_delivery_v1_serializes_exact_utf8_budget_and_required_fields() {
        let snapshot = "a".repeat(64);
        let query = "b".repeat(64);
        let sort = "c".repeat(64);
        let mut attributes = BTreeMap::new();
        attributes.insert("symbol".into(), serde_json::json!("函数"));
        let mut envelope = ContextDeliveryV1 {
            schema: CONTEXT_DELIVERY_SCHEMA.into(),
            authority: ContextDeliveryAuthority::NavigationOnly,
            snapshot_sha256: snapshot,
            query_sha256: query,
            sort_sha256: sort,
            offset: 0,
            budget_bytes: 4096,
            output_bytes: 0,
            total: 1,
            returned: 1,
            omitted: 0,
            truncated: false,
            next_cursor: None,
            records: vec![ContextDeliveryRecord::Resolved(ContextDeliveryItem {
                path: "src/lib.rs".into(),
                sha256: "d".repeat(64),
                line_range: ContextDeliveryLineRange { start: 3, end: 8 },
                reason: "查询命中".into(),
                provenance: vec![ContextDeliveryProvenance {
                    kind: "symbol_index".into(),
                    evidence: "exact symbol match".into(),
                    source_path: Some("src/lib.rs".into()),
                    source_sha256: Some("d".repeat(64)),
                }],
                attributes,
            })],
            coverage: ContextDeliveryCoverage {
                status: ContextDeliveryCoverageStatus::Complete,
                resolved_total: 1,
                resolved_returned: 1,
                unresolved_total: 0,
                unresolved_returned: 0,
                omitted_items: 0,
                detail: None,
            },
        };

        let encoded = envelope.encode_json().unwrap();
        assert_eq!(encoded.len(), envelope.output_bytes);
        assert!(encoded.len() <= envelope.budget_bytes);
        let json: serde_json::Value = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(json["schema"], CONTEXT_DELIVERY_SCHEMA);
        assert_eq!(json["authority"], "navigation_only");
        assert_eq!(json["records"][0]["state"], "resolved");
        assert_eq!(
            json["records"][0]["record"]["line_range"],
            serde_json::json!({"start": 3, "end": 8})
        );
        assert_eq!(ContextDeliveryV1::decode_json(&encoded).unwrap(), envelope);
        assert!(
            ContextDeliveryV1::decode_json(&serde_json::to_vec_pretty(&envelope).unwrap()).is_err(),
            "non-canonical JSON must not share a budget/cursor contract"
        );
    }

    #[test]
    fn context_delivery_cursor_invalidates_schema_snapshot_query_or_sort_drift() {
        let binding =
            ContextDeliveryCursorBinding::v1("a".repeat(64), "b".repeat(64), "c".repeat(64));
        let cursor = binding.encode_cursor(17).unwrap();
        assert_eq!(binding.decode_cursor(&cursor).unwrap(), 17);
        let (sealed_prefix, _old_position) = cursor.rsplit_once(':').unwrap();
        for malformed in [
            cursor.replacen("rayman-context-cursor-v1", "rayman-context-cursor-v2", 1),
            "rayman-context-cursor-v1".into(),
            format!("{sealed_prefix}:not-a-number"),
            format!("{sealed_prefix}:017"),
            format!("{cursor}:extra"),
        ] {
            assert!(
                binding.decode_cursor(&malformed).is_err(),
                "accepted malformed cursor {malformed}"
            );
        }
        assert!(
            binding
                .decode_cursor(&format!("{sealed_prefix}:18"))
                .is_err(),
            "the cursor seal must bind its next position"
        );

        for drifted in [
            ContextDeliveryCursorBinding {
                schema: "rayman.context-delivery.v2".into(),
                ..binding.clone()
            },
            ContextDeliveryCursorBinding {
                snapshot_sha256: "d".repeat(64),
                ..binding.clone()
            },
            ContextDeliveryCursorBinding {
                query_sha256: "d".repeat(64),
                ..binding.clone()
            },
            ContextDeliveryCursorBinding {
                sort_sha256: "d".repeat(64),
                ..binding.clone()
            },
        ] {
            assert!(drifted.decode_cursor(&cursor).is_err());
        }
    }

    #[test]
    fn context_delivery_rejects_silent_truncation_and_budget_overrun() {
        let binding =
            ContextDeliveryCursorBinding::v1("a".repeat(64), "b".repeat(64), "c".repeat(64));
        let mut envelope = ContextDeliveryV1 {
            schema: CONTEXT_DELIVERY_SCHEMA.into(),
            authority: ContextDeliveryAuthority::NavigationOnly,
            snapshot_sha256: binding.snapshot_sha256.clone(),
            query_sha256: binding.query_sha256.clone(),
            sort_sha256: binding.sort_sha256.clone(),
            offset: 0,
            budget_bytes: 4096,
            output_bytes: 0,
            total: 2,
            returned: 1,
            omitted: 1,
            truncated: false,
            next_cursor: None,
            records: vec![ContextDeliveryRecord::Resolved(ContextDeliveryItem {
                path: "src/lib.rs".into(),
                sha256: "d".repeat(64),
                line_range: ContextDeliveryLineRange { start: 1, end: 1 },
                reason: "exact path".into(),
                provenance: vec![ContextDeliveryProvenance {
                    kind: "query".into(),
                    evidence: "path equality".into(),
                    source_path: None,
                    source_sha256: None,
                }],
                attributes: BTreeMap::new(),
            })],
            coverage: ContextDeliveryCoverage {
                status: ContextDeliveryCoverageStatus::Partial,
                resolved_total: 2,
                resolved_returned: 1,
                unresolved_total: 0,
                unresolved_returned: 0,
                omitted_items: 1,
                detail: Some("output budget page".into()),
            },
        };

        assert!(
            envelope.encode_json().is_err(),
            "truncation must be explicit"
        );
        envelope.truncated = true;
        assert!(
            envelope.encode_json().is_err(),
            "truncation requires a cursor"
        );
        envelope.next_cursor = Some(binding.encode_cursor(1).unwrap());
        let mut over_budget = envelope.clone();
        over_budget.budget_bytes = 1;
        assert!(
            over_budget
                .encode_json()
                .unwrap_err()
                .to_string()
                .contains("exceeds UTF-8 budget")
        );

        envelope.encode_json().unwrap();
        for path in [
            "C:/outside/file.rs",
            "C:relative.rs",
            "/absolute.rs",
            "../escape.rs",
            r"src\windows.rs",
            r"\\server\share.rs",
            "src/lib.rs:secret",
        ] {
            let mut invalid = envelope.clone();
            let ContextDeliveryRecord::Resolved(item) = &mut invalid.records[0] else {
                unreachable!()
            };
            item.path = path.into();
            assert!(
                invalid.encode_json().is_err(),
                "accepted unsafe path {path}"
            );
        }
        let mut invalid = envelope.clone();
        let ContextDeliveryRecord::Resolved(item) = &mut invalid.records[0] else {
            unreachable!()
        };
        item.sha256 = "not-a-hash".into();
        assert!(invalid.encode_json().is_err());

        let mut invalid = envelope.clone();
        let ContextDeliveryRecord::Resolved(item) = &mut invalid.records[0] else {
            unreachable!()
        };
        item.line_range.start = 0;
        assert!(invalid.encode_json().is_err());

        let mut invalid = envelope.clone();
        let ContextDeliveryRecord::Resolved(item) = &mut invalid.records[0] else {
            unreachable!()
        };
        item.reason.clear();
        assert!(invalid.encode_json().is_err());

        let mut invalid = envelope.clone();
        let ContextDeliveryRecord::Resolved(item) = &mut invalid.records[0] else {
            unreachable!()
        };
        item.provenance.clear();
        assert!(invalid.encode_json().is_err());

        let mut invalid = envelope.clone();
        invalid.coverage.resolved_total = 1;
        assert!(invalid.encode_json().is_err());

        let mut invalid = envelope.clone();
        invalid.coverage.resolved_total = 0;
        invalid.coverage.unresolved_total = 2;
        assert!(
            invalid.encode_json().is_err(),
            "a returned resolved record cannot exceed the complete-query resolved total"
        );

        let mut invalid = envelope.clone();
        invalid.returned = 2;
        assert!(invalid.encode_json().is_err());
    }

    #[test]
    fn context_delivery_pages_unresolved_records_inside_the_same_budgeted_stream() {
        let binding =
            ContextDeliveryCursorBinding::v1("a".repeat(64), "b".repeat(64), "c".repeat(64));
        let unresolved = || {
            ContextDeliveryRecord::Unresolved(ContextDeliveryUnresolved {
                reference: "crate::missing".into(),
                reason: "dependency target absent from the verified snapshot".into(),
                provenance: vec![ContextDeliveryProvenance {
                    kind: "dependency_edge".into(),
                    evidence: "use crate::missing".into(),
                    source_path: Some("src/lib.rs".into()),
                    source_sha256: Some("d".repeat(64)),
                }],
            })
        };
        let mut first = ContextDeliveryV1 {
            schema: CONTEXT_DELIVERY_SCHEMA.into(),
            authority: ContextDeliveryAuthority::NavigationOnly,
            snapshot_sha256: binding.snapshot_sha256.clone(),
            query_sha256: binding.query_sha256.clone(),
            sort_sha256: binding.sort_sha256.clone(),
            offset: 0,
            budget_bytes: 4096,
            output_bytes: 0,
            total: 2,
            returned: 1,
            omitted: 1,
            truncated: true,
            next_cursor: Some(binding.encode_cursor(1).unwrap()),
            records: vec![unresolved()],
            coverage: ContextDeliveryCoverage {
                status: ContextDeliveryCoverageStatus::Partial,
                resolved_total: 0,
                resolved_returned: 0,
                unresolved_total: 2,
                unresolved_returned: 1,
                omitted_items: 1,
                detail: Some("one unresolved record remains".into()),
            },
        };
        let encoded = first.encode_json().unwrap();
        let json: serde_json::Value = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(json["records"][0]["state"], "unresolved");
        assert_eq!(
            binding
                .decode_cursor(first.next_cursor.as_deref().unwrap())
                .unwrap(),
            1
        );

        let mut second = ContextDeliveryV1 {
            offset: 1,
            omitted: 0,
            truncated: false,
            next_cursor: None,
            records: vec![unresolved()],
            coverage: ContextDeliveryCoverage {
                omitted_items: 0,
                ..first.coverage.clone()
            },
            output_bytes: 0,
            ..first
        };
        second.encode_json().unwrap();
    }
}
