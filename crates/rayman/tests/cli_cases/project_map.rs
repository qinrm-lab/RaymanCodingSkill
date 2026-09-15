use super::*;

#[test]
fn assets_scan_reports_obsolete_and_markers_without_deleting() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(
        root,
        "src/main.rs",
        "fn main() {} // TODO: 未完成 wire up\n",
    );
    write(root, "src/old.rs.bak", "dead");

    let report = run_json(root, &["assets"]);
    assert!(
        report["obsolete"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["path"].as_str().unwrap().ends_with(".bak"))
    );
    let markers: Vec<&str> = report["markers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["marker"].as_str().unwrap())
        .collect();
    assert!(markers.contains(&"TODO"));
    assert!(markers.contains(&"未完成"));
    // 只读：文件仍在。
    assert!(root.join("src/old.rs.bak").exists());
}

#[test]
fn map_commands_report_project_structure_and_impact() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "src/lib.rs", "pub mod parser;\npub mod evaluator;\n");
    write(root, "src/parser.rs", "pub fn parse() -> i32 { 1 }\n");
    write(
        root,
        "src/evaluator.rs",
        "use crate::parser;\npub fn eval() -> i32 { parser::parse() }\n",
    );
    write(
        root,
        "tests/evaluator_test.rs",
        "use sample::evaluator;\n#[test]\nfn eval_works() { assert_eq!(1, 1); }\n",
    );

    run_json(root, &["context", "refresh"]);

    let summary = run_json(root, &["map", "summary"]);
    assert_eq!(summary["source_files"], 3);
    assert_eq!(summary["test_files"], 1);
    assert!(
        summary["dependencies"].as_u64().unwrap() >= 1,
        "summary={summary}"
    );

    let file = run_json(root, &["map", "file", "src/evaluator.rs"]);
    assert_eq!(file["path"], "src/evaluator.rs");
    assert!(
        file["outgoing_dependencies"]
            .as_array()
            .unwrap()
            .iter()
            .any(|dependency| dependency["to_path"] == "src/parser.rs")
    );

    let symbols = run_json(root, &["map", "symbol", "eval"]);
    assert!(
        symbols["matches"]
            .as_array()
            .unwrap()
            .iter()
            .any(|symbol| symbol["path"] == "src/evaluator.rs")
    );

    let impact = run_json(root, &["map", "impact", "src/evaluator.rs"]);
    assert!(
        impact["related_tests"]
            .as_array()
            .unwrap()
            .iter()
            .any(|test| test["path"] == "tests/evaluator_test.rs")
    );
    assert_eq!(
        impact["related_tests"][0]["basis"],
        "same_package_test_text_reference_heuristic"
    );
    assert!(
        impact["recommendation_basis"]
            .as_str()
            .unwrap()
            .contains("heuristic")
    );
    assert!(
        impact["recommended_checks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|check| check == "cargo test --all")
    );
    let project_map = root.join(".RaymanCodingSkill/context/project_map.json");
    assert!(
        !project_map.exists(),
        "read-only map queries must not create a cache"
    );
    let refreshed = run(root, &["map", "refresh"]);
    assert_eq!(refreshed.status, 0, "stderr={}", refreshed.stderr);
    assert!(project_map.exists());
}

#[test]
fn map_topology_and_impact_include_cargo_path_dependents() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(
        root,
        "Cargo.toml",
        "[workspace]\nmembers = [\"crates/core\", \"crates/app\"]\nresolver = \"2\"\n",
    );
    write(
        root,
        "crates/core/Cargo.toml",
        "[package]\nname = \"core\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    );
    write(
        root,
        "crates/core/src/lib.rs",
        "pub fn core_api() -> i32 { 1 }\n",
    );
    write(
        root,
        "crates/app/Cargo.toml",
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[dependencies]\ncore = { path = \"../core\" }\n",
    );
    write(
        root,
        "crates/app/src/lib.rs",
        "pub fn app_api() -> i32 { core::core_api() }\n",
    );
    write(
        root,
        "crates/app/tests/app_test.rs",
        "use app::app_api;\n#[test]\nfn app_works() { assert_eq!(app_api(), 1); }\n",
    );
    run_json(root, &["context", "refresh"]);

    let topology = run_json(root, &["map", "topology"]);
    assert_eq!(topology["packages"].as_array().unwrap().len(), 2);
    assert!(
        topology["package_dependencies"]
            .as_array()
            .unwrap()
            .iter()
            .any(|dependency| {
                dependency["from_package"] == "app" && dependency["to_package"] == "core"
            }),
        "topology={topology}"
    );

    let impact = run_json(root, &["map", "impact", "crates/core/src/lib.rs"]);
    assert_eq!(impact["package"], "core");
    assert!(
        impact["package_dependents"]
            .as_array()
            .unwrap()
            .iter()
            .any(|dependency| dependency["from_package"] == "app"),
        "impact={impact}"
    );
    assert!(
        impact["recommended_checks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|check| check == "cargo test -p core"),
        "impact={impact}"
    );
    assert!(
        impact["recommended_checks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|check| check == "cargo test -p app"),
        "impact={impact}"
    );
}

#[test]
fn map_topology_includes_workspace_inherited_path_dependents() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(
        root,
        "Cargo.toml",
        "[workspace]\nmembers = [\"crates/core\", \"crates/app\"]\nresolver = \"2\"\n\n[workspace.dependencies]\ncore = { path = \"crates/core\" }\n",
    );
    write(
        root,
        "crates/core/Cargo.toml",
        "[package]\nname = \"core\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    );
    write(
        root,
        "crates/core/src/lib.rs",
        "pub fn core_api() -> i32 { 1 }\n",
    );
    write(
        root,
        "crates/app/Cargo.toml",
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[dependencies]\ncore = { workspace = true }\n",
    );
    write(
        root,
        "crates/app/src/lib.rs",
        "pub fn app_api() -> i32 { core::core_api() }\n",
    );
    run_json(root, &["context", "refresh"]);

    let topology = run_json(root, &["map", "topology"]);
    assert!(
        topology["package_dependencies"]
            .as_array()
            .unwrap()
            .iter()
            .any(|dependency| {
                dependency["from_package"] == "app"
                    && dependency["from_root_path"] == "crates/app"
                    && dependency["to_package"] == "core"
                    && dependency["to_root_path"] == "crates/core"
            }),
        "topology={topology}"
    );

    let impact = run_json(root, &["map", "impact", "crates/core/src/lib.rs"]);
    assert!(
        impact["package_dependents"]
            .as_array()
            .unwrap()
            .iter()
            .any(|dependency| dependency["from_package"] == "app"),
        "impact={impact}"
    );
    assert!(
        impact["recommended_checks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|check| check == "cargo test -p core"),
        "impact={impact}"
    );
    assert!(
        impact["recommended_checks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|check| check == "cargo test -p app"),
        "impact={impact}"
    );
}

#[test]
fn map_topology_includes_dotted_workspace_inherited_path_dependents() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(
        root,
        "Cargo.toml",
        "[workspace]\nmembers = [\"crates/core\", \"crates/app\"]\nresolver = \"2\"\n\n[workspace.dependencies]\ncore.path = \"crates/core\"\n",
    );
    write(
        root,
        "crates/core/Cargo.toml",
        "[package]\nname = \"core\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    );
    write(
        root,
        "crates/core/src/lib.rs",
        "pub fn core_api() -> i32 { 1 }\n",
    );
    write(
        root,
        "crates/app/Cargo.toml",
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[target.'cfg(windows)'.dependencies]\ncore.workspace = true\n",
    );
    write(
        root,
        "crates/app/src/lib.rs",
        "pub fn app_api() -> i32 { core::core_api() }\n",
    );
    run_json(root, &["context", "refresh"]);

    let topology = run_json(root, &["map", "topology"]);
    assert!(
        topology["package_dependencies"]
            .as_array()
            .unwrap()
            .iter()
            .any(|dependency| {
                dependency["from_package"] == "app"
                    && dependency["from_root_path"] == "crates/app"
                    && dependency["to_package"] == "core"
                    && dependency["to_root_path"] == "crates/core"
            }),
        "topology={topology}"
    );
}

#[test]
fn map_plan_check_blocks_broad_source_change_without_test_anchor() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(
        root,
        "Cargo.toml",
        "[package]\nname = \"sample\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    write(root, "src/lib.rs", "pub mod parser;\npub mod evaluator;\n");
    write(root, "src/parser.rs", "pub fn parse() -> i32 { 1 }\n");
    write(root, "src/evaluator.rs", "pub fn eval() -> i32 { 1 }\n");
    run_json(root, &["context", "refresh"]);

    let plan = run(
        root,
        &[
            "map",
            "plan",
            "src/lib.rs",
            "src/parser.rs",
            "src/evaluator.rs",
            "--check",
        ],
    );
    assert_eq!(plan.status, 1);
    assert!(
        plan.stdout.contains("no same-package candidate test"),
        "stdout={}",
        plan.stdout
    );
}

#[test]
fn map_plan_check_passes_broad_change_without_supported_package() {
    // Real-world basis: dogfooding rayman against a 792-file, 60k-line C# repo showed
    // this heuristic hard-blocking a well-tested change because it only understands
    // modeled package shapes. Outside Cargo/pyproject packages it must be advisory.
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "src/A.cs", "public class A {}\n");
    write(root, "src/B.cs", "public class B {}\n");
    write(root, "src/C.cs", "public class C {}\n");
    run_json(root, &["context", "refresh"]);

    let plan = run(
        root,
        &["map", "plan", "src/A.cs", "src/B.cs", "src/C.cs", "--check"],
    );
    assert_eq!(plan.status, 0, "stdout={}", plan.stdout);
    assert!(
        plan.stdout
            .contains("no Cargo or pyproject package detected"),
        "stdout={}",
        plan.stdout
    );
}

#[test]
fn map_plan_check_blocks_package_broad_change_without_indexed_test_anchor() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(
        root,
        "Cargo.toml",
        "[workspace]\nmembers = [\"crates/core\", \"crates/app\"]\nresolver = \"2\"\n",
    );
    write(
        root,
        "crates/core/Cargo.toml",
        "[package]\nname = \"core\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    );
    write(root, "crates/core/src/lib.rs", "pub mod a;\npub mod b;\n");
    write(root, "crates/core/src/a.rs", "pub fn a() -> i32 { 1 }\n");
    write(root, "crates/core/src/b.rs", "pub fn b() -> i32 { 2 }\n");
    write(
        root,
        "crates/app/Cargo.toml",
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[dependencies]\ncore = { path = \"../core\" }\n",
    );
    write(
        root,
        "crates/app/src/lib.rs",
        "pub fn app_api() -> i32 { core::a::a() + core::b::b() }\n",
    );
    run_json(root, &["context", "refresh"]);

    let plan = run(
        root,
        &[
            "map",
            "plan",
            "crates/core/src/lib.rs",
            "crates/core/src/a.rs",
            "crates/core/src/b.rs",
            "--check",
        ],
    );
    assert_eq!(plan.status, 1);
    assert!(
        plan.stdout
            .contains("no same-package candidate test target")
            && plan.stdout.contains("indexed package test anchor"),
        "stdout={}",
        plan.stdout
    );
    assert!(
        plan.stdout.contains("cargo test -p core") && plan.stdout.contains("cargo test -p app"),
        "stdout={}",
        plan.stdout
    );
}

#[test]
fn map_plan_check_accepts_package_test_anchors_for_broad_source_change() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(
        root,
        "Cargo.toml",
        "[workspace]\nmembers = [\"crates/core\", \"crates/app\"]\nresolver = \"2\"\n",
    );
    write(
        root,
        "crates/core/Cargo.toml",
        "[package]\nname = \"core\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    );
    write(root, "crates/core/src/lib.rs", "pub mod a;\npub mod b;\n");
    write(root, "crates/core/src/a.rs", "pub fn a() -> i32 { 1 }\n");
    write(root, "crates/core/src/b.rs", "pub fn b() -> i32 { 2 }\n");
    write(
        root,
        "crates/app/Cargo.toml",
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[dependencies]\ncore = { path = \"../core\" }\n",
    );
    write(
        root,
        "crates/app/src/lib.rs",
        "pub fn app_api() -> i32 { core::a::a() + core::b::b() }\n",
    );
    write(
        root,
        "crates/app/tests/app_test.rs",
        "use app::app_api;\n#[test]\nfn app_works() { assert_eq!(app_api(), 3); }\n",
    );
    run_json(root, &["context", "refresh"]);

    let plan = run_json(
        root,
        &[
            "map",
            "plan",
            "crates/core/src/lib.rs",
            "crates/core/src/a.rs",
            "crates/core/src/b.rs",
            "--check",
        ],
    );
    assert_eq!(plan["ready"], true, "plan={plan}");
    assert!(
        plan["blockers"].as_array().unwrap().is_empty(),
        "plan={plan}"
    );
    assert!(
        plan["recommended_checks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|check| check == "cargo test -p core"),
        "plan={plan}"
    );
    assert!(
        plan["recommended_checks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|check| check == "cargo test -p app"),
        "plan={plan}"
    );
}

#[test]
fn map_quality_check_blocks_multi_source_project_without_tests() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(
        root,
        "Cargo.toml",
        "[package]\nname = \"sample\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    write(root, "src/lib.rs", "pub mod parser;\npub mod evaluator;\n");
    write(root, "src/parser.rs", "pub fn parse() -> i32 { 1 }\n");
    write(root, "src/evaluator.rs", "pub fn eval() -> i32 { 1 }\n");
    run_json(root, &["context", "refresh"]);

    let quality = run_json(root, &["map", "quality"]);
    assert_eq!(quality["ready"], false);
    assert_eq!(quality["error_count"], 1);
    assert!(
        quality["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|finding| {
                finding["kind"] == "multi_source_project_without_tests"
                    && finding["severity"] == "error"
            }),
        "quality={quality}"
    );

    let quality_check = run(root, &["map", "quality", "--check"]);
    assert_eq!(quality_check.status, 1);
    assert!(
        quality_check
            .stdout
            .contains("multi_source_project_without_tests"),
        "stdout={}",
        quality_check.stdout
    );

    let standard = run(root, &["check", "--profile", "standard"]);
    assert_eq!(standard.status, 1);
    assert!(
        standard
            .stdout
            .contains("quality multi_source_project_without_tests"),
        "stdout={}",
        standard.stdout
    );
}

#[test]
fn strict_quality_config_can_block_configured_warning_kinds() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "src/lib.rs", "pub fn api() {}\n");
    write(
        root,
        ".RaymanCodingSkill/quality.json",
        "{\n  \"block_warning_kinds\": [\"public_api_without_test_evidence\"]\n}\n",
    );
    run_json(root, &["context", "refresh"]);

    let standard = run(root, &["map", "quality", "--check"]);
    assert_eq!(
        standard.status, 0,
        "stdout={} stderr={}",
        standard.stdout, standard.stderr
    );

    let strict = run(root, &["map", "quality", "--profile", "strict", "--check"]);
    assert_eq!(strict.status, 1);
    assert!(
        strict
            .stdout
            .contains("configured as blocking by .RaymanCodingSkill/quality.json"),
        "stdout={}",
        strict.stdout
    );
}

#[test]
fn strict_quality_config_fails_closed_on_unknown_fields() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "src/lib.rs", "pub fn api() {}\n");
    write(
        root,
        ".RaymanCodingSkill/quality.json",
        "{\n  \"block_warning_kind\": [\"public_api_without_test_evidence\"]\n}\n",
    );
    run_json(root, &["context", "refresh"]);

    let standard = run(root, &["check", "--profile", "standard"]);
    assert_eq!(
        standard.status, 0,
        "stdout={} stderr={}",
        standard.stdout, standard.stderr
    );

    let strict = run(root, &["map", "quality", "--profile", "strict", "--check"]);
    assert_eq!(strict.status, 1);
    assert!(
        strict.stderr.contains("quality.json") && strict.stderr.contains("unknown field"),
        "stderr={}",
        strict.stderr
    );

    let release = run(root, &["check", "--profile", "release"]);
    assert_eq!(release.status, 1);
    assert!(
        release.stderr.contains("quality.json") && release.stderr.contains("unknown field"),
        "stderr={}",
        release.stderr
    );
}

#[test]
fn strict_quality_config_fails_closed_on_unknown_warning_kinds() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "src/lib.rs", "pub fn api() {}\n");
    write(
        root,
        ".RaymanCodingSkill/quality.json",
        "{\n  \"block_warning_kinds\": [\"public_api_without_test_evdence\"]\n}\n",
    );
    run_json(root, &["context", "refresh"]);

    let standard = run(root, &["check", "--profile", "standard"]);
    assert_eq!(
        standard.status, 0,
        "stdout={} stderr={}",
        standard.stdout, standard.stderr
    );

    let strict = run(root, &["map", "quality", "--profile", "strict", "--check"]);
    assert_eq!(strict.status, 1);
    assert!(
        strict.stderr.contains("quality.json")
            && strict.stderr.contains("unknown block_warning_kinds entry"),
        "stderr={}",
        strict.stderr
    );

    let release = run(root, &["check", "--profile", "release"]);
    assert_eq!(release.status, 1);
    assert!(
        release.stderr.contains("quality.json")
            && release.stderr.contains("unknown block_warning_kinds entry"),
        "stderr={}",
        release.stderr
    );
}

#[test]
fn map_quality_check_passes_with_a_test_anchor() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "src/lib.rs", "pub mod parser;\npub mod evaluator;\n");
    write(root, "src/parser.rs", "pub fn parse() -> i32 { 1 }\n");
    write(
        root,
        "src/evaluator.rs",
        "use crate::parser;\npub fn eval() -> i32 { parser::parse() }\n",
    );
    write(
        root,
        "tests/evaluator_test.rs",
        "use sample::evaluator;\n#[test]\nfn evaluator_works() {}\n",
    );
    run_json(root, &["context", "refresh"]);

    let quality = run_json(root, &["map", "quality"]);
    assert_eq!(quality["ready"], true, "quality={quality}");
    assert_eq!(quality["error_count"], 0);

    let quality_check = run(root, &["map", "quality", "--check"]);
    assert_eq!(
        quality_check.status, 0,
        "stdout={} stderr={}",
        quality_check.stdout, quality_check.stderr
    );
}

#[test]
fn map_commands_fail_closed_on_missing_or_stale_context() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "src/lib.rs", "pub fn answer() -> i32 { 42 }\n");

    let missing = run(root, &["--format", "json", "map", "summary"]);
    assert_eq!(missing.status, 1);
    let missing_error: Value = serde_json::from_str(&missing.stderr)
        .unwrap_or_else(|error| panic!("stderr is not JSON: {error}\n{}", missing.stderr));
    assert!(
        missing_error["error"]
            .as_str()
            .unwrap()
            .contains("上下文索引")
    );

    run_json(root, &["context", "refresh"]);
    write(root, "src/new.rs", "pub fn new_item() {}\n");
    let stale = run(root, &["--format", "json", "map", "summary"]);
    assert_eq!(stale.status, 1);
    let stale_error: Value = serde_json::from_str(&stale.stderr)
        .unwrap_or_else(|error| panic!("stderr is not JSON: {error}\n{}", stale.stderr));
    assert!(
        stale_error["error"]
            .as_str()
            .unwrap()
            .contains("不是 ready")
    );
}

#[test]
fn map_impact_does_not_infer_related_tests_across_package_roots() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(
        root,
        "Cargo.toml",
        "[workspace]\nmembers = [\"crates/rayman\"]\nexclude = [\"evals\"]\nresolver = \"2\"\n",
    );
    write(
        root,
        "crates/rayman/Cargo.toml",
        "[package]\nname = \"rayman\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    );
    write(root, "crates/rayman/src/lib.rs", "pub fn cli() {}\n");
    write(
        root,
        "crates/rayman/tests/cli.rs",
        "use rayman::cli;\n#[test]\nfn cli_works() { cli(); }\n",
    );
    write(
        root,
        "evals/tasks/add-feature/fixture/Cargo.toml",
        "[package]\nname = \"task\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    );
    write(
        root,
        "evals/tasks/add-feature/fixture/src/lib.rs",
        "pub fn add(left: i32, right: i32) -> i32 { left + right }\n",
    );
    run_json(root, &["context", "refresh"]);

    let impact = run_json(
        root,
        &[
            "map",
            "impact",
            "evals/tasks/add-feature/fixture/src/lib.rs",
        ],
    );
    assert!(
        impact["related_tests"].as_array().unwrap().is_empty(),
        "impact={impact}"
    );
    assert!(
        impact["recommended_checks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|check| check
                == "cargo test --manifest-path evals/tasks/add-feature/fixture/Cargo.toml"),
        "impact={impact}"
    );
    assert!(
        !impact["recommended_checks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|check| check == "cargo test -p task"),
        "impact={impact}"
    );
    assert!(
        !impact["recommended_checks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|check| check
                .as_str()
                .unwrap()
                .contains("crates/rayman/tests/cli.rs")),
        "impact={impact}"
    );
}

#[test]
fn map_impact_uses_manifest_path_for_duplicate_workspace_package_names() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(
        root,
        "Cargo.toml",
        "[workspace]\nmembers = [\"crates/one\", \"crates/two\"]\nresolver = \"2\"\n",
    );
    write(
        root,
        "crates/one/Cargo.toml",
        "[package]\nname = \"shared\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    );
    write(root, "crates/one/src/lib.rs", "pub fn one() {}\n");
    write(
        root,
        "crates/two/Cargo.toml",
        "[package]\nname = \"shared\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    );
    write(root, "crates/two/src/lib.rs", "pub fn two() {}\n");
    run_json(root, &["context", "refresh"]);

    let impact = run_json(root, &["map", "impact", "crates/one/src/lib.rs"]);
    assert!(
        impact["recommended_checks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|check| check == "cargo test --manifest-path crates/one/Cargo.toml"),
        "impact={impact}"
    );
    assert!(
        !impact["recommended_checks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|check| check == "cargo test -p shared"),
        "impact={impact}"
    );
}

#[test]
fn map_impact_uses_manifest_path_for_nested_package_under_workspace_member_glob() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(
        root,
        "Cargo.toml",
        "[workspace]\nmembers = [\"crates/*\"]\nresolver = \"2\"\n",
    );
    write(
        root,
        "crates/app/Cargo.toml",
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    );
    write(root, "crates/app/src/lib.rs", "pub fn app() {}\n");
    write(
        root,
        "crates/app/fixture/Cargo.toml",
        "[package]\nname = \"task\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    );
    write(root, "crates/app/fixture/src/lib.rs", "pub fn task() {}\n");
    run_json(root, &["context", "refresh"]);

    let impact = run_json(root, &["map", "impact", "crates/app/fixture/src/lib.rs"]);
    assert!(
        impact["recommended_checks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|check| check == "cargo test --manifest-path crates/app/fixture/Cargo.toml"),
        "impact={impact}"
    );
    assert!(
        !impact["recommended_checks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|check| check == "cargo test -p task"),
        "impact={impact}"
    );
}

#[test]
fn map_impact_rejects_directory_inputs_instead_of_returning_empty_success() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "src/lib.rs", "pub fn answer() -> i32 { 42 }\n");
    run_json(root, &["context", "refresh"]);

    let impact = run(root, &["map", "impact", "src"]);
    assert_eq!(impact.status, 1);
    assert!(
        impact.stderr.contains("indexed directory"),
        "{}",
        impact.stderr
    );
    assert!(impact.stderr.contains("map plan"), "{}", impact.stderr);
}
