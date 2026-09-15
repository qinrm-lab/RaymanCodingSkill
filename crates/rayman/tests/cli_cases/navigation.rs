use super::*;

#[test]
fn budgeted_goal_navigation_pages_without_baselines_and_preserves_legacy_shapes() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "src/lib.rs", "pub fn answer() -> i32 { 42 }\n");
    let started = run_json(
        root,
        &[
            "goal",
            "start",
            "budgeted goal fixture",
            "--must-proof",
            "test::first requirement",
            "--must-proof",
            "test::second requirement",
        ],
    );
    let goal_id = started["id"].as_str().unwrap().to_string();
    run_json(
        root,
        &[
            "goal",
            "package",
            "add",
            &goal_id,
            "parent",
            "parent package",
            "--req",
            "req_1",
        ],
    );
    run_json(
        root,
        &[
            "goal",
            "package",
            "add",
            &goal_id,
            "child",
            "child package",
            "--parent",
            "parent",
            "--req",
            "req_2",
        ],
    );

    let legacy = run_json(root, &["goal", "list"]);
    assert!(legacy.is_array());
    assert!(legacy[0].get("baseline").is_none());

    let before = state_snapshot(root);
    let brief = run(
        root,
        &[
            "--format",
            "json",
            "goal",
            "brief",
            &goal_id,
            "--limit",
            "1",
            "--budget-bytes",
            "4096",
        ],
    );
    assert_eq!(brief.status, 0, "stderr={}", brief.stderr);
    assert!(!brief.stdout.ends_with('\n'));
    let brief_json: Value = serde_json::from_str(&brief.stdout).unwrap();
    assert_eq!(brief_json["schema"], "rayman.context-delivery.v1");
    assert_eq!(brief_json["authority"], "navigation_only");
    assert_eq!(
        brief_json["sort_sha256"],
        rayman::context::context_delivery_identity(
            &"goal-brief:overview;open-requirement-goal-order;package-vector-order;delta-summary-then-path-order;recent-progress-recorded-at-desc-id-desc;missing-validation-goal-order;frontier;next-command:v1"
        )
        .unwrap()
    );
    let raw_goal = std::fs::read(
        root.join(".RaymanCodingSkill/goals")
            .join(format!("{goal_id}.json")),
    )
    .unwrap();
    assert_eq!(
        brief_json["records"][0]["record"]["line_range"]["end"],
        String::from_utf8(raw_goal).unwrap().lines().count().max(1)
    );
    assert_eq!(
        brief.stdout.len() as u64,
        brief_json["output_bytes"].as_u64().unwrap()
    );
    assert!(brief_json["truncated"].as_bool().unwrap());
    assert!(!brief.stdout.contains("\"baseline\":"));
    let brief_cursor = brief_json["next_cursor"].as_str().unwrap();
    let english = run(
        root,
        &[
            "--language",
            "en",
            "--format",
            "json",
            "goal",
            "brief",
            &goal_id,
            "--limit",
            "1",
            "--budget-bytes",
            "4096",
        ],
    );
    let chinese = run(
        root,
        &[
            "--language",
            "zh-CN",
            "--format",
            "json",
            "goal",
            "brief",
            &goal_id,
            "--limit",
            "1",
            "--budget-bytes",
            "4096",
        ],
    );
    assert_eq!(english.stdout, chinese.stdout);
    let next_brief = run_json(
        root,
        &[
            "goal",
            "brief",
            &goal_id,
            "--cursor",
            brief_cursor,
            "--limit",
            "2",
            "--budget-bytes",
            "8192",
        ],
    );
    assert_eq!(next_brief["offset"], 1);

    let listed = run_json(
        root,
        &[
            "goal",
            "list",
            "--lifecycle",
            "current",
            "--status",
            "active",
            "--limit",
            "1",
            "--budget-bytes",
            "4096",
        ],
    );
    assert_eq!(listed["records"][0]["record"]["attributes"]["id"], goal_id);
    let packages = run_json(
        root,
        &[
            "goal",
            "package",
            "list",
            &goal_id,
            "--limit",
            "1",
            "--budget-bytes",
            "4096",
        ],
    );
    assert_eq!(packages["total"], 2);
    assert_eq!(packages["returned"], 1);
    let package_cursor = packages["next_cursor"].as_str().unwrap().to_string();
    let shown = run_json(
        root,
        &[
            "goal",
            "package",
            "show",
            &goal_id,
            "parent",
            "--limit",
            "1",
            "--budget-bytes",
            "4096",
        ],
    );
    assert_eq!(
        shown["records"][0]["record"]["attributes"]["package_id"],
        "parent"
    );
    assert_eq!(
        state_snapshot(root),
        before,
        "navigation queries must be read-only"
    );

    let cross_goal_page = run_json(
        root,
        &[
            "goal",
            "brief",
            &goal_id,
            "--limit",
            "1",
            "--budget-bytes",
            "4096",
        ],
    );
    let cross_goal_cursor = cross_goal_page["next_cursor"].as_str().unwrap();
    run_json(
        root,
        &["goal", "start", "other goal", "--must", "other work"],
    );
    let cross_goal_stale = run(
        root,
        &[
            "--format",
            "json",
            "goal",
            "brief",
            &goal_id,
            "--cursor",
            cross_goal_cursor,
            "--budget-bytes",
            "8192",
        ],
    );
    assert_ne!(cross_goal_stale.status, 0);
    assert!(cross_goal_stale.stderr.contains("invalidated"));

    let progress_goal = run_json(
        root,
        &[
            "goal",
            "start",
            "progress next-command fixture",
            "--must",
            "finish stage",
        ],
    );
    let progress_goal_id = progress_goal["id"].as_str().unwrap();
    run_json(
        root,
        &[
            "goal",
            "package",
            "add",
            progress_goal_id,
            "stage",
            "stage package",
            "--req",
            "req_1",
        ],
    );
    let progress = run_json(
        root,
        &[
            "goal",
            "progress",
            progress_goal_id,
            "--package",
            "stage",
            "--message",
            "stage passed",
            "--command",
            "rustc --version",
        ],
    );
    let progress_brief = run_json(
        root,
        &["goal", "brief", progress_goal_id, "--budget-bytes", "16384"],
    );
    let progress_next = progress_brief["records"]
        .as_array()
        .unwrap()
        .iter()
        .find(|record| record["record"]["attributes"]["record_type"] == "next_command")
        .unwrap();
    assert_eq!(
        progress_next["record"]["attributes"]["command"],
        format!(
            "rayman goal package complete {progress_goal_id} stage --progress {}",
            progress["id"].as_str().unwrap()
        )
    );

    run_json(
        root,
        &[
            "goal",
            "package",
            "add",
            &goal_id,
            "later",
            "later package",
            "--optional",
        ],
    );
    let stale = run(
        root,
        &[
            "--format",
            "json",
            "goal",
            "package",
            "list",
            &goal_id,
            "--cursor",
            &package_cursor,
            "--budget-bytes",
            "8192",
        ],
    );
    assert_ne!(stale.status, 0);
    assert!(stale.stderr.contains("invalidated"), "{}", stale.stderr);

    write(root, ".RaymanCodingSkill/goals/corrupt.json", "{ broken");
    let mut invalid_contract: Value = serde_json::from_slice(
        &std::fs::read(
            root.join(".RaymanCodingSkill/goals")
                .join(format!("{goal_id}.json")),
        )
        .unwrap(),
    )
    .unwrap();
    invalid_contract["id"] = Value::String("invalid_contract".into());
    invalid_contract["requirements"][1]["id"] = Value::String("req_1".into());
    std::fs::write(
        root.join(".RaymanCodingSkill/goals/invalid_contract.json"),
        serde_json::to_vec_pretty(&invalid_contract).unwrap(),
    )
    .unwrap();
    let mut invalid_legacy: Value = serde_json::from_slice(
        &std::fs::read(
            root.join(".RaymanCodingSkill/goals")
                .join(format!("{goal_id}.json")),
        )
        .unwrap(),
    )
    .unwrap();
    invalid_legacy["schema_version"] = Value::from(0);
    invalid_legacy["id"] = Value::String("invalid_legacy".into());
    let duplicate_package = invalid_legacy["work_packages"][0].clone();
    invalid_legacy["work_packages"]
        .as_array_mut()
        .unwrap()
        .push(duplicate_package);
    std::fs::write(
        root.join(".RaymanCodingSkill/goals/invalid_legacy.json"),
        serde_json::to_vec_pretty(&invalid_legacy).unwrap(),
    )
    .unwrap();
    let unresolved = run_json(
        root,
        &[
            "goal",
            "list",
            "--lifecycle",
            "current",
            "--limit",
            "20",
            "--budget-bytes",
            "16384",
        ],
    );
    assert_eq!(unresolved["coverage"]["unresolved_total"], 3);
    assert!(
        unresolved["records"]
            .as_array()
            .unwrap()
            .iter()
            .any(|record| record["state"] == "unresolved")
    );

    let evidence_only = run_json(
        root,
        &[
            "goal",
            "start",
            "evidence-only brief fixture",
            "--must",
            "must still validate",
        ],
    );
    let evidence_id = evidence_only["id"].as_str().unwrap();
    run_json(
        root,
        &[
            "goal",
            "evidence",
            evidence_id,
            "--req",
            "req_1",
            "--message",
            "caller attestation only",
            "--validated",
            "cargo test --locked",
        ],
    );
    let evidence_brief = run_json(
        root,
        &["goal", "brief", evidence_id, "--budget-bytes", "16384"],
    );
    let next = evidence_brief["records"]
        .as_array()
        .unwrap()
        .iter()
        .find(|record| record["record"]["attributes"]["record_type"] == "next_command")
        .unwrap();
    assert!(
        next["record"]["attributes"]["command"]
            .as_str()
            .unwrap()
            .contains("goal validate"),
        "next={next}"
    );
}

#[test]
fn budgeted_map_queries_scope_fields_depth_and_cursor_drift_fail_closed() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(
        root,
        "Cargo.toml",
        "[package]\nname = \"budget-map\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[dependencies]\nbudget-dep = { path = \"crates/budget-dep\" }\n\n[workspace]\nmembers = [\"crates/budget-dep\"]\nresolver = \"3\"\n",
    );
    write(
        root,
        "src/lib.rs",
        "mod dep;\npub fn alpha() -> i32 { dep::value() }\npub fn alphabet() -> i32 { alpha() }\n",
    );
    write(
        root,
        "src/dep.rs",
        "use crate::deep;\npub fn value() -> i32 { deep::value() }\n",
    );
    write(root, "src/deep.rs", "pub fn value() -> i32 { 1 }\n");
    write(
        root,
        "crates/budget-dep/Cargo.toml",
        "[package]\nname = \"budget-dep\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    );
    write(
        root,
        "crates/budget-dep/src/lib.rs",
        "pub fn dep_value() -> i32 { 1 }\n",
    );
    write(
        root,
        "tests/api.rs",
        "#[test]\nfn alpha_works() { assert_eq!(budget_map::alpha(), 1); }\n",
    );
    generate_lockfile(root);
    run_json(root, &["context", "refresh"]);

    let legacy = run_json(root, &["map", "symbol", "alpha"]);
    assert!(legacy["matches"].is_array());
    let legacy_topology = run_json(root, &["map", "topology"]);
    assert!(legacy_topology.get("schema").is_none());
    assert!(legacy_topology["packages"].is_array());
    assert!(
        legacy_topology["packages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|package| package["name"] == "budget-map" && package["root_path"] == "."),
        "legacy_topology={legacy_topology}"
    );
    assert!(
        legacy_topology["package_dependencies"]
            .as_array()
            .unwrap()
            .iter()
            .any(|dependency| {
                dependency["from_package"] == "budget-map"
                    && dependency["to_package"] == "budget-dep"
                    && dependency["dependency_name"] == "budget-dep"
            }),
        "legacy_topology={legacy_topology}"
    );
    let before = state_snapshot(root);
    let exact = run(
        root,
        &[
            "--format",
            "json",
            "map",
            "symbol",
            "alpha",
            "--exact",
            "--path-prefix",
            "src",
            "--package",
            "budget-map",
            "--fields",
            "name,kind,module,package",
            "--limit",
            "1",
            "--budget-bytes",
            "4096",
        ],
    );
    assert_eq!(exact.status, 0, "stderr={}", exact.stderr);
    assert!(!exact.stdout.ends_with('\n'));
    let exact_json: Value = serde_json::from_str(&exact.stdout).unwrap();
    assert_eq!(exact_json["total"], 1);
    assert_eq!(exact_json["authority"], "navigation_only");
    assert_eq!(
        exact_json["sort_sha256"],
        rayman::context::context_delivery_identity(&"symbol:context-record-canonical-json-asc:v1")
            .unwrap()
    );
    assert_eq!(
        exact_json["records"][0]["record"]["attributes"]["name"],
        "alpha"
    );
    assert_eq!(
        exact.stdout.len() as u64,
        exact_json["output_bytes"].as_u64().unwrap()
    );

    let file = run_json(
        root,
        &[
            "map",
            "file",
            "src/lib.rs",
            "--max-depth",
            "2",
            "--fields",
            "record_type,direction,depth,name",
            "--limit",
            "1",
            "--budget-bytes",
            "4096",
        ],
    );
    assert!(file["truncated"].as_bool().unwrap());
    assert_eq!(
        file["sort_sha256"],
        rayman::context::context_delivery_identity(&"file:context-record-canonical-json-asc:v1")
            .unwrap()
    );
    let cursor = file["next_cursor"].as_str().unwrap().to_string();
    let depth = run_json(
        root,
        &[
            "map",
            "file",
            "src/lib.rs",
            "--max-depth",
            "2",
            "--fields",
            "record_type,direction,depth",
            "--limit",
            "100",
            "--budget-bytes",
            "32768",
        ],
    );
    assert!(
        depth["records"]
            .as_array()
            .unwrap()
            .iter()
            .any(|record| record["record"]["attributes"]["depth"] == 2),
        "depth={depth}"
    );
    let canonical_records = depth["records"]
        .as_array()
        .unwrap()
        .iter()
        .map(|record| {
            let record =
                serde_json::from_value::<rayman::context::ContextDeliveryRecord>(record.clone())
                    .unwrap();
            serde_json::to_vec(&record).unwrap()
        })
        .collect::<Vec<_>>();
    assert!(canonical_records.windows(2).all(|pair| pair[0] <= pair[1]));
    let impact = run_json(
        root,
        &[
            "map",
            "impact",
            "src/lib.rs",
            "--package",
            "budget-map",
            "--max-depth",
            "2",
            "--limit",
            "2",
            "--budget-bytes",
            "4096",
        ],
    );
    assert!(impact["total"].as_u64().unwrap() >= 1);
    assert_eq!(
        impact["sort_sha256"],
        rayman::context::context_delivery_identity(&"impact:context-record-canonical-json-asc:v1")
            .unwrap()
    );
    let plan = run_json(
        root,
        &[
            "map",
            "plan",
            "src/lib.rs",
            "src/dep.rs",
            "--path-prefix",
            "src",
            "--max-depth",
            "2",
            "--limit",
            "2",
            "--budget-bytes",
            "4096",
        ],
    );
    assert!(plan["total"].as_u64().unwrap() >= 2);
    assert_eq!(
        plan["sort_sha256"],
        rayman::context::context_delivery_identity(&"plan:context-record-canonical-json-asc:v1")
            .unwrap()
    );
    let topology_fields = "record_type,name,package,root_path,manifest_path,workspace_member,source_files,test_files,from_package,from_root_path,to_package,to_root_path,dependency_name,kind,evidence";
    let topology_page = run(
        root,
        &[
            "--format",
            "json",
            "map",
            "topology",
            "--fields",
            topology_fields,
            "--limit",
            "1",
            "--budget-bytes",
            "8192",
        ],
    );
    assert_eq!(topology_page.status, 0, "stderr={}", topology_page.stderr);
    assert!(!topology_page.stdout.ends_with('\n'));
    let topology_page_json: Value = serde_json::from_str(&topology_page.stdout).unwrap();
    assert_eq!(topology_page_json["schema"], "rayman.context-delivery.v1");
    assert_eq!(topology_page_json["authority"], "navigation_only");
    assert!(topology_page_json["truncated"].as_bool().unwrap());
    assert_eq!(topology_page_json["returned"], 1);
    assert_eq!(
        topology_page_json["sort_sha256"],
        rayman::context::context_delivery_identity(
            &"topology:context-record-canonical-json-asc:v1"
        )
        .unwrap()
    );
    assert_eq!(
        topology_page.stdout.len() as u64,
        topology_page_json["output_bytes"].as_u64().unwrap()
    );
    let topology_cursor = topology_page_json["next_cursor"]
        .as_str()
        .unwrap()
        .to_string();
    let topology = run_json(
        root,
        &[
            "map",
            "topology",
            "--fields",
            topology_fields,
            "--cursor",
            &topology_cursor,
            "--limit",
            "100",
            "--budget-bytes",
            "32768",
        ],
    );
    assert_eq!(topology["offset"], 1);
    let topology_records = topology["records"].as_array().unwrap();
    assert!(
        topology_records
            .iter()
            .any(
                |record| record["record"]["attributes"]["record_type"] == "package"
                    && record["record"]["attributes"]["package"] == "budget-dep"
                    && record["record"]["attributes"]["manifest_path"]
                        == "crates/budget-dep/Cargo.toml"
            ),
        "topology={topology}"
    );
    assert!(
        topology_records.iter().any(|record| {
            record["record"]["attributes"]["record_type"] == "package_dependency"
                && record["record"]["attributes"]["from_package"] == "budget-map"
                && record["record"]["attributes"]["to_package"] == "budget-dep"
                && record["record"]["attributes"]["dependency_name"] == "budget-dep"
                && record["record"]["attributes"]["kind"] == "normal"
        }),
        "topology={topology}"
    );
    for record in topology_records {
        let attributes = record["record"]["attributes"].as_object().unwrap();
        assert!(
            attributes
                .keys()
                .all(|field| topology_fields.split(',').any(|allowed| allowed == field)),
            "unexpected projected attributes={attributes:?}"
        );
    }
    assert_eq!(
        state_snapshot(root),
        before,
        "map queries must be read-only"
    );

    let field_drift = run(
        root,
        &[
            "--format",
            "json",
            "map",
            "file",
            "src/lib.rs",
            "--max-depth",
            "2",
            "--fields",
            "record_type,direction,depth",
            "--cursor",
            &cursor,
            "--budget-bytes",
            "8192",
        ],
    );
    assert_ne!(field_drift.status, 0);
    assert!(field_drift.stderr.contains("invalidated"));
    let filter_drift = run(
        root,
        &[
            "--format",
            "json",
            "map",
            "file",
            "src/lib.rs",
            "--max-depth",
            "1",
            "--fields",
            "record_type,direction,depth,name",
            "--cursor",
            &cursor,
            "--budget-bytes",
            "8192",
        ],
    );
    assert_ne!(filter_drift.status, 0);
    assert!(filter_drift.stderr.contains("invalidated"));

    let index_path = root.join(".RaymanCodingSkill/context/index.json");
    let index_value: Value = serde_json::from_slice(&std::fs::read(&index_path).unwrap()).unwrap();
    std::fs::write(&index_path, serde_json::to_vec(&index_value).unwrap()).unwrap();
    let raw_snapshot_stale = run(
        root,
        &[
            "--format",
            "json",
            "map",
            "file",
            "src/lib.rs",
            "--max-depth",
            "2",
            "--fields",
            "record_type,direction,depth,name",
            "--cursor",
            &cursor,
            "--budget-bytes",
            "8192",
        ],
    );
    assert_ne!(raw_snapshot_stale.status, 0);
    assert!(raw_snapshot_stale.stderr.contains("invalidated"));
    let refreshed_page = run_json(
        root,
        &[
            "map",
            "file",
            "src/lib.rs",
            "--max-depth",
            "2",
            "--fields",
            "record_type,direction,depth,name",
            "--limit",
            "1",
            "--budget-bytes",
            "4096",
        ],
    );
    let content_cursor = refreshed_page["next_cursor"].as_str().unwrap().to_string();

    let unknown = run(
        root,
        &["map", "symbol", "alpha", "--fields", "unknown-field"],
    );
    assert_ne!(unknown.status, 0);
    assert!(unknown.stderr.contains("unknown map delivery field"));
    let unknown_topology = run(root, &["map", "topology", "--fields", "unknown-field"]);
    assert_ne!(unknown_topology.status, 0);
    assert!(
        unknown_topology
            .stderr
            .contains("unknown map delivery field")
    );
    let unsafe_prefix = run(
        root,
        &["map", "symbol", "alpha", "--path-prefix", "../escape"],
    );
    assert_ne!(unsafe_prefix.status, 0);
    assert!(unsafe_prefix.stderr.contains("workspace-relative"));
    let tiny = run(
        root,
        &["map", "symbol", "alpha", "--exact", "--budget-bytes", "32"],
    );
    assert_ne!(tiny.status, 0);
    assert!(tiny.stderr.contains("budget_too_small"));
    assert!(tiny.stderr.contains("split_required"));

    write(
        root,
        "src/lib.rs",
        "mod dep;\npub fn alpha() -> i32 { dep::value() + 1 }\npub fn alphabet() -> i32 { alpha() }\n",
    );
    run_json(root, &["context", "refresh"]);
    let stale = run(
        root,
        &[
            "--format",
            "json",
            "map",
            "file",
            "src/lib.rs",
            "--max-depth",
            "2",
            "--fields",
            "record_type,direction,depth,name",
            "--cursor",
            &content_cursor,
            "--budget-bytes",
            "8192",
        ],
    );
    assert_ne!(stale.status, 0);
    assert!(stale.stderr.contains("invalidated"), "{}", stale.stderr);

    let duplicate = tempfile::tempdir().unwrap();
    for package in ["one", "two"] {
        write(
            duplicate.path(),
            &format!("crates/{package}/Cargo.toml"),
            "[package]\nname = \"duplicate\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        );
        write(
            duplicate.path(),
            &format!("crates/{package}/src/lib.rs"),
            "pub fn duplicated() {}\n",
        );
    }
    run_json(duplicate.path(), &["context", "refresh"]);
    let ambiguous = run(
        duplicate.path(),
        &[
            "map",
            "symbol",
            "duplicated",
            "--package",
            "duplicate",
            "--limit",
            "1",
        ],
    );
    assert_ne!(ambiguous.status, 0);
    assert!(
        ambiguous.stderr.contains("ambiguous"),
        "{}",
        ambiguous.stderr
    );
}

#[test]
fn budgeted_context_retrieval_commands_are_navigation_only_and_stale_safe() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(
        root,
        "src/lib.rs",
        "pub fn alpha() -> i32 {\n    helper()\n}\nfn helper() -> i32 { 1 }\n",
    );
    write(
        root,
        "src/parser.rs",
        "pub fn parse_alpha(input: &str) -> bool {\n    input.contains(\"alpha\")\n}\n",
    );
    write(root, "README.md", "# fixture\nalpha docs\n");
    run_json(root, &["context", "refresh"]);

    let before = state_snapshot(root);
    let overview = run(
        root,
        &[
            "--format",
            "json",
            "context",
            "overview",
            "--kind",
            "source",
            "--path-prefix",
            "src",
            "--fields",
            "record_type,kind,lines,symbol_count",
            "--limit",
            "1",
            "--budget-bytes",
            "4096",
        ],
    );
    assert_eq!(overview.status, 0, "stderr={}", overview.stderr);
    assert!(!overview.stdout.ends_with('\n'));
    let overview_json: Value = serde_json::from_str(&overview.stdout).unwrap();
    assert_eq!(overview_json["schema"], "rayman.context-delivery.v1");
    assert_eq!(overview_json["authority"], "navigation_only");
    assert_eq!(
        overview_json["sort_sha256"],
        rayman::context::context_delivery_identity(
            &"context-overview:context-record-canonical-json-asc:v1"
        )
        .unwrap()
    );
    assert_eq!(overview_json["total"], 2);
    assert_eq!(overview_json["returned"], 1);
    assert!(overview_json["truncated"].as_bool().unwrap());
    assert_eq!(
        overview.stdout.len() as u64,
        overview_json["output_bytes"].as_u64().unwrap()
    );
    let overview_cursor = overview_json["next_cursor"].as_str().unwrap().to_string();
    let next_overview = run_json(
        root,
        &[
            "context",
            "overview",
            "--kind",
            "source",
            "--path-prefix",
            "src",
            "--fields",
            "record_type,kind,lines,symbol_count",
            "--cursor",
            &overview_cursor,
            "--limit",
            "100",
            "--budget-bytes",
            "8192",
        ],
    );
    assert_eq!(next_overview["offset"], 1);

    let query = run_json(
        root,
        &[
            "context",
            "query",
            "alpha",
            "--symbols",
            "--content",
            "--path-prefix",
            "src",
            "--fields",
            "record_type,matched,name,symbol_kind,match_line,text,encoding",
            "--budget-bytes",
            "16384",
        ],
    );
    let query_records = query["records"].as_array().unwrap();
    assert!(query_records.iter().any(|record| {
        record["record"]["attributes"]["record_type"] == "symbol_match"
            && record["record"]["attributes"]["name"] == "alpha"
    }));
    assert!(query_records.iter().any(|record| {
        record["record"]["attributes"]["record_type"] == "content_match"
            && record["record"]["attributes"]["text"]
                .as_str()
                .unwrap()
                .contains("alpha")
    }));

    let excerpt = run_json(
        root,
        &[
            "context",
            "excerpt",
            "src/lib.rs",
            "--start",
            "1",
            "--end",
            "2",
            "--budget-bytes",
            "8192",
        ],
    );
    assert_eq!(excerpt["total"], 1);
    assert_eq!(excerpt["records"][0]["record"]["path"], "src/lib.rs");
    assert_eq!(excerpt["records"][0]["record"]["line_range"]["start"], 1);
    assert_eq!(excerpt["records"][0]["record"]["line_range"]["end"], 2);
    assert!(
        excerpt["records"][0]["record"]["sha256"]
            .as_str()
            .unwrap()
            .len()
            == 64
    );
    assert!(
        excerpt["records"][0]["record"]["attributes"]["text"]
            .as_str()
            .unwrap()
            .contains("helper()")
    );

    let pack = run_json(
        root,
        &[
            "context",
            "pack",
            "src/lib.rs",
            "README.md",
            "missing.rs",
            "--fields",
            "record_type,text,encoding,kind",
            "--budget-bytes",
            "16384",
        ],
    );
    assert_eq!(pack["total"], 3);
    assert_eq!(pack["coverage"]["unresolved_total"], 1);
    assert!(pack["records"].as_array().unwrap().iter().any(|record| {
        record["state"] == "resolved"
            && record["record"]["attributes"]["record_type"] == "pack_file"
            && record["record"]["attributes"]["text"]
                .as_str()
                .unwrap()
                .contains("alpha")
    }));
    assert!(pack["records"].as_array().unwrap().iter().any(|record| {
        record["state"] == "unresolved" && record["record"]["reference"] == "missing.rs"
    }));
    assert_eq!(
        state_snapshot(root),
        before,
        "context retrieval commands must be read-only"
    );

    let unknown_field = run(root, &["context", "overview", "--fields", "unknown-field"]);
    assert_ne!(unknown_field.status, 0);
    assert!(
        unknown_field
            .stderr
            .contains("unknown context delivery field"),
        "{}",
        unknown_field.stderr
    );
    let unsafe_path = run(
        root,
        &[
            "context",
            "excerpt",
            "../escape.rs",
            "--start",
            "1",
            "--end",
            "1",
        ],
    );
    assert_ne!(unsafe_path.status, 0);
    assert!(
        unsafe_path.stderr.contains("workspace-relative"),
        "{}",
        unsafe_path.stderr
    );
    let empty_query = run(root, &["context", "query", "   "]);
    assert_ne!(empty_query.status, 0);
    assert!(
        empty_query.stderr.contains("term must not be empty"),
        "{}",
        empty_query.stderr
    );
    let tiny_budget = run(
        root,
        &["context", "pack", "src/lib.rs", "--budget-bytes", "32"],
    );
    assert_ne!(tiny_budget.status, 0);
    assert!(tiny_budget.stderr.contains("budget_too_small"));
    assert!(tiny_budget.stderr.contains("split_required"));

    write(
        root,
        "src/parser.rs",
        "pub fn parse_alpha(input: &str) -> bool {\n    input.starts_with(\"alpha\")\n}\n",
    );
    run_json(root, &["context", "refresh"]);
    let stale_cursor = run(
        root,
        &[
            "--format",
            "json",
            "context",
            "overview",
            "--kind",
            "source",
            "--path-prefix",
            "src",
            "--fields",
            "record_type,kind,lines,symbol_count",
            "--cursor",
            &overview_cursor,
            "--budget-bytes",
            "8192",
        ],
    );
    assert_ne!(stale_cursor.status, 0);
    assert!(stale_cursor.stderr.contains("invalidated"));

    write(root, "src/new.rs", "pub fn beta() {}\n");
    let stale_context = run(root, &["context", "overview", "--kind", "source"]);
    assert_ne!(stale_context.status, 0);
    assert!(
        stale_context.stderr.contains("上下文索引不是 ready")
            || stale_context.stderr.contains("context refresh"),
        "{}",
        stale_context.stderr
    );
}
