use super::*;
use std::fs;

fn write(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

#[test]
fn map_builds_dependencies_and_impact_from_current_context() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root.join("src/lib.rs").as_path(),
        "pub mod parser;\npub mod evaluator;\n",
    );
    write(
        root.join("src/parser.rs").as_path(),
        "pub fn parse() -> i32 { 1 }\n",
    );
    write(
        root.join("src/evaluator.rs").as_path(),
        "use crate::parser;\npub fn eval() -> i32 { parser::parse() }\n",
    );
    write(
        root.join("tests/evaluator_test.rs").as_path(),
        "use sample::evaluator;\n#[test]\nfn eval_works() { assert_eq!(1, 1); }\n",
    );
    context::refresh(root).unwrap();

    let map = build(root).unwrap();
    assert!(
        map.dependencies
            .iter()
            .any(|dependency| dependency.from_path == "src/evaluator.rs"
                && dependency.to_path == "src/parser.rs")
    );
    let impact = impact_report(&map, "src/evaluator.rs").unwrap();
    assert!(
        impact
            .related_tests
            .iter()
            .any(|test| test.path == "tests/evaluator_test.rs")
    );
    assert!(
        impact
            .recommended_checks
            .iter()
            .any(|check| check == "cargo test --all")
    );
}

#[test]
fn evals_dependency_policy_changes_recommend_evals_deny_check() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root.join("Cargo.toml").as_path(),
        "[workspace]\nmembers = [\"crates/rayman\"]\nexclude = [\"evals\"]\n",
    );
    write(
        root.join("crates/rayman/Cargo.toml").as_path(),
        "[package]\nname = \"rayman\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    write(
        root.join("evals/Cargo.toml").as_path(),
        "[package]\nname = \"rayman-evals\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    write(root.join("evals/deny.toml").as_path(), "[bans]\n");
    context::refresh(root).unwrap();

    let map = build_readonly(root).unwrap();
    let impact = impact_report(&map, "evals/Cargo.toml").unwrap();

    assert!(impact.recommended_checks.iter().any(|check| {
        check == "cargo deny --manifest-path evals\\Cargo.toml check --config evals\\deny.toml"
    }));
}

#[test]
fn change_plan_groups_impacted_files_tests_and_checks() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root.join("src/lib.rs").as_path(),
        "pub mod parser;\npub mod evaluator;\n",
    );
    write(
        root.join("src/parser.rs").as_path(),
        "pub fn parse() -> i32 { 1 }\n",
    );
    write(
        root.join("src/evaluator.rs").as_path(),
        "use crate::parser;\npub fn eval() -> i32 { parser::parse() }\n",
    );
    write(
        root.join("tests/evaluator_test.rs").as_path(),
        "use sample::evaluator;\n#[test]\nfn evaluator_works() {}\n",
    );
    context::refresh(root).unwrap();

    let map = build_readonly(root).unwrap();
    let plan = change_plan(&map, &["src/parser.rs".to_string()]).unwrap();

    assert!(plan.ready);
    assert_eq!(plan.review_priority, "normal");
    assert!(
        plan.impacted_files
            .iter()
            .any(|file| { file.path == "src/parser.rs" && file.role == "changed" })
    );
    assert!(
        plan.impacted_files
            .iter()
            .any(|file| { file.path == "src/evaluator.rs" && file.role == "dependent" })
    );
    assert!(
        plan.related_tests
            .iter()
            .any(|test| test.path == "tests/evaluator_test.rs")
    );
    assert!(
        plan.recommended_checks
            .iter()
            .any(|check| check == "cargo test --all")
    );
}

#[test]
fn change_plan_blocks_broad_source_change_without_test_anchor() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root.join("src/lib.rs").as_path(),
        "pub mod a;\npub mod b;\n",
    );
    write(root.join("src/a.rs").as_path(), "pub fn a() {}\n");
    write(root.join("src/b.rs").as_path(), "pub fn b() {}\n");
    context::refresh(root).unwrap();

    let map = build_readonly(root).unwrap();
    let plan = change_plan(
        &map,
        &[
            "src/lib.rs".to_string(),
            "src/a.rs".to_string(),
            "src/b.rs".to_string(),
        ],
    )
    .unwrap();

    assert!(!plan.ready);
    assert_eq!(plan.review_priority, "high");
    assert!(plan.blockers.iter().any(|blocker| {
        blocker.contains("3 source files") && blocker.contains("no same-package candidate test")
    }));
}

#[test]
fn map_resolves_use_dependencies_inside_nested_crate_src_roots() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root.join("crates/app/src/lib.rs").as_path(),
        "pub mod context;\npub mod map;\n",
    );
    write(
        root.join("crates/app/src/context.rs").as_path(),
        "pub fn refresh() {}\n",
    );
    write(
        root.join("crates/app/src/map.rs").as_path(),
        "use crate::context;\npub fn build() { context::refresh(); }\n",
    );
    context::refresh(root).unwrap();

    let map = build(root).unwrap();
    assert!(
        map.dependencies
            .iter()
            .any(|dependency| dependency.from_path == "crates/app/src/map.rs"
                && dependency.to_path == "crates/app/src/context.rs"),
        "dependencies={:?}",
        map.dependencies
    );
}

#[test]
fn quality_report_blocks_multi_source_project_without_tests() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root.join("src/lib.rs").as_path(), "pub mod parser;\n");
    write(root.join("src/parser.rs").as_path(), "pub fn parse() {}\n");
    write(
        root.join("src/evaluator.rs").as_path(),
        "pub fn eval() {}\n",
    );
    context::refresh(root).unwrap();

    let map = build_readonly(root).unwrap();
    let quality = quality_report(&map);

    assert!(!quality.ready);
    assert_eq!(quality.error_count, 1);
    assert!(quality.findings.iter().any(|finding| {
        finding.kind == "multi_source_project_without_tests" && finding.severity == "error"
    }));
}

#[test]
fn quality_report_keeps_uncovered_public_api_as_warning() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root.join("src/lib.rs").as_path(), "pub fn api() {}\n");
    write(
        root.join("tests/api_test.rs").as_path(),
        "use sample::api;\n#[test]\nfn api_works() {}\n",
    );
    context::refresh(root).unwrap();

    let map = build_readonly(root).unwrap();
    let quality = quality_report(&map);

    assert!(quality.ready);
    assert_eq!(quality.error_count, 0);
    assert!(quality.warning_count <= quality.findings.len());
}

#[test]
fn quality_report_ignores_eval_fixture_source_risks() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root.join("evals/tasks/sample/fixture/src/lib.rs").as_path(),
        "pub fn intentionally_broken_fixture_api() {}\n",
    );
    context::refresh(root).unwrap();

    let map = build_readonly(root).unwrap();
    let quality = quality_report(&map);

    assert!(quality.ready);
    assert_eq!(quality.warning_count, 0);
    assert!(
        quality
            .findings
            .iter()
            .all(|finding| !finding.path.contains("/fixture/"))
    );
}

#[test]
fn quality_report_keeps_non_eval_fixture_source_risks() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root.join("crates/sample/fixture/src/lib.rs").as_path(),
        "pub fn real_api() {}\n",
    );
    context::refresh(root).unwrap();

    let map = build_readonly(root).unwrap();
    let quality = quality_report(&map);

    assert!(quality.findings.iter().any(|finding| {
        finding.path == "crates/sample/fixture/src/lib.rs"
            && finding.kind == "public_api_without_test_evidence"
    }));
}
