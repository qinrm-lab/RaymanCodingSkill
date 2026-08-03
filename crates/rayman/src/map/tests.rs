use super::*;
use std::fs;

fn write(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

#[test]
fn cargo_workspace_requires_metadata_provenance_for_authoritative_topology() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::write(root.join("Cargo.toml"), "[workspace]\nmembers = []\n").unwrap();

    assert!(topology_provenance_is_authoritative(root, "cargo_metadata"));
    assert!(!topology_provenance_is_authoritative(
        root,
        "heuristic_fallback: cargo metadata unavailable"
    ));
    assert!(!topology_provenance_is_authoritative(root, ""));
}

#[test]
fn subdirectory_only_cargo_workspace_obtains_authority_instead_of_being_blocked_forever() {
    // 根目录没有 Cargo.toml 只说明"在根上跑 cargo metadata 会失败"，不说明拓扑不可信。
    // 早先的判据把这种仓库判为非权威，而权威性是 standard/release 的硬前提，于是
    // crate 全在子目录的多语言 monorepo 被永久阻塞、且没有任何可达的解除路径。
    // 现在改为对索引到的每个 manifest 逐个跑 cargo metadata，真的把权威拿到手。
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root.join("crates/foo/Cargo.toml").as_path(),
        "[package]\nname = \"foo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    write(
        root.join("crates/foo/src/lib.rs").as_path(),
        "pub fn foo() {}\n",
    );
    context::refresh(root).unwrap();

    let map = build_readonly(root).unwrap();
    assert_eq!(map.topology_provenance, "cargo_metadata");
    assert!(map.packages.iter().any(|package| package.name == "foo"));
    assert!(topology_is_authoritative(root, &map));
}

#[test]
fn a_cargo_manifest_metadata_cannot_parse_stays_non_authoritative() {
    // 拿不到权威时仍必须 fail-closed：启发式拓扑不得支撑 standard/release readiness。
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root.join("crates/broken/Cargo.toml").as_path(),
        "[package\nname = \"broken\"\n",
    );
    write(
        root.join("crates/broken/src/lib.rs").as_path(),
        "pub fn broken() {}\n",
    );
    context::refresh(root).unwrap();

    let map = build_readonly(root).unwrap();
    assert_ne!(map.topology_provenance, "cargo_metadata");
    assert!(!topology_is_authoritative(root, &map));
}

#[test]
fn workspaces_without_cargo_packages_stay_authoritative() {
    // The fix above must not start blocking ecosystems Cargo has no opinion about.
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root.join("src/app.js").as_path(),
        "export function app() { return 1; }\n",
    );
    write(
        root.join("pyproject.toml").as_path(),
        "[project]\nname = \"svc\"\n",
    );
    write(
        root.join("svc/core.py").as_path(),
        "def value():\n    return 1\n",
    );
    context::refresh(root).unwrap();

    let map = build_readonly(root).unwrap();
    assert!(
        map.packages
            .iter()
            .any(|package| package.manifest_path == "pyproject.toml")
    );
    assert!(topology_is_authoritative(root, &map));
}

#[test]
fn unrelated_cargo_fixture_does_not_block_javascript_conclusions() {
    // Reproduces the reported repro: a small JavaScript repository that also carries one
    // unrelated Rust fixture crate. Every JavaScript file used to jump from advisory to hard
    // blocker just because some `Cargo.toml` existed somewhere in the tree.
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    for name in ["app", "router", "store", "view"] {
        write(
            root.join(format!("src/{name}.js")).as_path(),
            &format!("export function {name}() {{ return 1; }}\n"),
        );
    }
    write(
        root.join("fixtures/tiny/Cargo.toml").as_path(),
        "[package]\nname = \"tiny\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    write(
        root.join("fixtures/tiny/src/lib.rs").as_path(),
        "pub fn tiny() {}\n",
    );
    context::refresh(root).unwrap();

    let map = build_readonly(root).unwrap();
    assert!(map.packages.iter().any(|package| package.name == "tiny"));

    let plan = change_plan(
        &map,
        &[
            "src/app.js".to_string(),
            "src/router.js".to_string(),
            "src/store.js".to_string(),
        ],
    )
    .unwrap();
    assert!(plan.ready, "blockers={:?}", plan.blockers);
    assert!(
        plan.warnings
            .iter()
            .any(|warning| warning.contains("no Cargo or pyproject package detected"))
    );

    let quality = quality_report(&map);
    assert!(quality.ready, "findings={:?}", quality.findings);
    assert!(quality.findings.iter().any(|finding| {
        finding.kind == "multi_source_project_without_tests" && finding.severity == "warning"
    }));
}

#[test]
fn cargo_package_sources_still_block_next_to_unsupported_ecosystem_files() {
    // Fail-safe direction check for the scoping above: once the changed set really is Cargo
    // source files in a Cargo package with no test anchor, it must still block.
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root.join("web/app.js").as_path(),
        "export function app() {}\n",
    );
    write(
        root.join("crates/core/Cargo.toml").as_path(),
        "[package]\nname = \"core\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    write(
        root.join("crates/core/src/lib.rs").as_path(),
        "pub mod a;\npub mod b;\n",
    );
    write(
        root.join("crates/core/src/a.rs").as_path(),
        "pub fn a() {}\n",
    );
    write(
        root.join("crates/core/src/b.rs").as_path(),
        "pub fn b() {}\n",
    );
    context::refresh(root).unwrap();

    let map = build_readonly(root).unwrap();
    let plan = change_plan(
        &map,
        &[
            "crates/core/src/lib.rs".to_string(),
            "crates/core/src/a.rs".to_string(),
            "crates/core/src/b.rs".to_string(),
        ],
    )
    .unwrap();

    assert!(!plan.ready);
    assert!(
        plan.blockers
            .iter()
            .any(|blocker| blocker.contains("3 source files"))
    );
}

#[test]
fn python_production_file_does_not_mint_a_self_covering_test_anchor() {
    // pytest never collects a production module, so pytest-shaped names inside one are not a
    // validation anchor. Counting them used to produce a `inline_test_in_source_file`
    // TestTarget covering the very file that defined them and unblock uncovered changes.
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root.join("pyproject.toml").as_path(),
        "[project]\nname = \"svc\"\n",
    );
    write(
        root.join("svc/models.py").as_path(),
        "class TestResult:\n    def test_passed(self):\n        return True\n\n\ndef test_matrix():\n    return []\n",
    );
    write(
        root.join("svc/api.py").as_path(),
        "def handle():\n    return 1\n",
    );
    write(
        root.join("svc/worker.py").as_path(),
        "def work():\n    return 2\n",
    );
    context::refresh(root).unwrap();

    let map = build_readonly(root).unwrap();
    assert!(
        !map.tests.iter().any(|test| test.path == "svc/models.py"),
        "tests={:?}",
        map.tests
    );

    let plan = change_plan(
        &map,
        &[
            "svc/models.py".to_string(),
            "svc/api.py".to_string(),
            "svc/worker.py".to_string(),
        ],
    )
    .unwrap();
    assert!(!plan.ready, "warnings={:?}", plan.warnings);
    assert!(
        plan.blockers
            .iter()
            .any(|blocker| blocker.contains("3 source files"))
    );
}

#[test]
fn real_pytest_files_keep_their_test_anchor() {
    // The Python fix must only remove the fake anchors, not pytest's real ones.
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root.join("pyproject.toml").as_path(),
        "[project]\nname = \"svc\"\n",
    );
    write(
        root.join("svc/api.py").as_path(),
        "def handle():\n    return 1\n",
    );
    write(
        root.join("tests/test_api.py").as_path(),
        "from svc.api import handle\n\n\nclass TestApi:\n    def test_handle(self):\n        assert handle() == 1\n",
    );
    context::refresh(root).unwrap();

    let map = build_readonly(root).unwrap();
    let test = map
        .tests
        .iter()
        .find(|test| test.path == "tests/test_api.py")
        .unwrap();
    assert!(test.test_count > 0);
    assert!(
        test.candidate_paths
            .iter()
            .any(|candidate| candidate == "svc/api.py")
    );
}

#[test]
fn map_build_tolerates_non_utf8_cargo_and_pyproject_manifests() {
    // A manifest with a GBK comment anywhere in the tree — even a workspace-excluded fixture —
    // used to make `build_from_index` bail, which failed map/check/prepare/finish outright and
    // left the workspace unable to reach READY under any profile.
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root.join("src/lib.rs").as_path(), "pub fn ok() {}\n");
    fs::create_dir_all(root.join("fixtures/legacy")).unwrap();
    fs::write(
        root.join("fixtures/legacy/Cargo.toml"),
        gbk_comment_manifest(
            b"[package]\nname = \"legacy\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        ),
    )
    .unwrap();
    write(
        root.join("fixtures/legacy/src/lib.rs").as_path(),
        "pub fn legacy() {}\n",
    );
    fs::create_dir_all(root.join("fixtures/pylegacy")).unwrap();
    fs::write(
        root.join("fixtures/pylegacy/pyproject.toml"),
        gbk_comment_manifest(b"[project]\nname = \"legacy-python\"\n"),
    )
    .unwrap();
    context::refresh(root).unwrap();

    let map = build_readonly(root).unwrap();
    assert!(map.packages.iter().any(|package| package.name == "legacy"));
    assert!(
        map.packages
            .iter()
            .any(|package| package.name == "legacy-python")
    );
}

#[test]
fn map_build_tolerates_a_non_utf8_root_workspace_manifest() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::write(
        root.join("Cargo.toml"),
        gbk_comment_manifest(
            b"[package]\nname = \"sample\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        ),
    )
    .unwrap();
    write(root.join("src/lib.rs").as_path(), "pub fn ok() {}\n");
    context::refresh(root).unwrap();

    // cargo metadata cannot read a non-UTF-8 manifest either, so the heuristic fallback has to
    // carry the map instead of the whole pipeline failing.
    let map = build_readonly(root).unwrap();
    assert!(
        map.topology_provenance.starts_with("heuristic_fallback"),
        "provenance={}",
        map.topology_provenance
    );
    assert!(map.packages.iter().any(|package| package.name == "sample"));
    assert!(!topology_is_authoritative(root, &map));
}

/// `# 中文` in GBK followed by `body`: valid TOML apart from a comment that is not UTF-8.
fn gbk_comment_manifest(body: &[u8]) -> Vec<u8> {
    let mut bytes = vec![b'#', b' ', 0xD6, 0xD0, 0xCE, 0xC4, b'\n'];
    bytes.extend_from_slice(body);
    bytes
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
fn map_build_tolerates_non_utf8_source_files() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root.join("src/lib.rs").as_path(), "pub fn ok() {}\n");
    // GBK 编码字节：非法 UTF-8。此前会让整条 map/check 管线 bail。
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/gbk.rs"), [0xD6u8, 0xD0, 0xCE, 0xC4, b'\n']).unwrap();
    context::refresh(root).unwrap();

    let map = build(root).unwrap();
    assert!(map.modules.iter().any(|module| module.path == "src/gbk.rs"));
}

#[test]
fn map_build_rejects_source_drift_after_index_validation() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root.join("src/lib.rs").as_path(),
        "pub fn value() -> i32 { 1 }\n",
    );
    context::refresh(root).unwrap();
    let index = context::verified_index(root).unwrap();

    write(
        root.join("src/lib.rs").as_path(),
        "pub fn value() -> i32 { 2 }\n",
    );
    let error = build_from_index(root, &index).unwrap_err().to_string();
    assert!(
        error.contains("项目地图读取失败") && error.contains("src/lib.rs"),
        "error={error}"
    );
}

#[test]
fn use_of_workspace_crate_name_creates_local_dependency_edge() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root.join("Cargo.toml").as_path(),
        "[package]\nname = \"acme-widgets\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    write(root.join("src/lib.rs").as_path(), "pub mod parser;\n");
    write(root.join("src/parser.rs").as_path(), "pub fn parse() {}\n");
    write(
        root.join("tests/parser_test.rs").as_path(),
        "use acme_widgets::parser;\n#[test]\nfn t() { parser::parse(); }\n",
    );
    context::refresh(root).unwrap();

    // crate 名来自实际发现的 package（acme-widgets → acme_widgets），不是硬编码的 rayman。
    let map = build(root).unwrap();
    assert!(map.dependencies.iter().any(|dependency| {
        dependency.from_path == "tests/parser_test.rs" && dependency.to_path == "src/parser.rs"
    }));
}

#[test]
fn public_api_risk_skips_test_helper_files() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root.join("tests/common/mod.rs").as_path(),
        "pub fn setup() {}\n",
    );
    write(root.join("src/lib.rs").as_path(), "fn private_only() {}\n");
    context::refresh(root).unwrap();

    let map = build(root).unwrap();
    assert!(
        !map.risks.iter().any(|risk| {
            risk.kind == "public_api_without_test_evidence" && risk.path.starts_with("tests/")
        }),
        "测试辅助文件不应触发 public_api_without_test_evidence"
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
        check == "cargo deny --manifest-path evals/Cargo.toml check --config evals/deny.toml"
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
        root.join("Cargo.toml").as_path(),
        "[package]\nname = \"sample\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
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
fn change_plan_warns_instead_of_blocking_without_supported_package() {
    // Verified against a real 60k-line C# workspace: outside currently modeled package
    // ecosystems, the "no test anchor" finding must stay advisory rather than hard-blocking.
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root.join("src/a.cs").as_path(), "public class A {}\n");
    write(root.join("src/b.cs").as_path(), "public class B {}\n");
    write(root.join("src/c.cs").as_path(), "public class C {}\n");
    context::refresh(root).unwrap();

    let map = build_readonly(root).unwrap();
    let plan = change_plan(
        &map,
        &[
            "src/a.cs".to_string(),
            "src/b.cs".to_string(),
            "src/c.cs".to_string(),
        ],
    )
    .unwrap();

    assert!(plan.ready, "blockers={:?}", plan.blockers);
    assert!(
        plan.warnings
            .iter()
            .any(|warning| { warning.contains("no Cargo or pyproject package detected") })
    );
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
    write(
        root.join("Cargo.toml").as_path(),
        "[package]\nname = \"sample\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
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
fn quality_report_downgrades_missing_tests_without_supported_package() {
    // Same real-world basis as change_plan_warns_instead_of_blocking_without_supported_package:
    // outside Cargo/pyproject packages the test-detection heuristic has no real signal.
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root.join("src/a.cs").as_path(), "public class A {}\n");
    write(root.join("src/b.cs").as_path(), "public class B {}\n");
    write(root.join("src/c.cs").as_path(), "public class C {}\n");
    context::refresh(root).unwrap();

    let map = build_readonly(root).unwrap();
    let quality = quality_report(&map);

    assert!(quality.ready, "findings={:?}", quality.findings);
    assert_eq!(quality.error_count, 0);
    assert!(quality.findings.iter().any(|finding| {
        finding.kind == "multi_source_project_without_tests" && finding.severity == "warning"
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
fn quality_report_does_not_exempt_eval_fixture_shaped_paths() {
    // A hardcoded "evals/tasks/*/fixture/src/*" exemption used to suppress all
    // risk detection for any file matching that shape, in any workspace — not
    // just this repo's own eval harness. That let risky/untested code hide
    // from `rayman check` by simply living at a path with this shape. The
    // generic risk engine must not special-case any project's own directory
    // layout; a workspace-specific exemption belongs in that workspace's own
    // `.RaymanCodingSkill/quality.json`, not in map.rs.
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root.join("evals/tasks/sample/fixture/src/lib.rs").as_path(),
        "pub fn intentionally_untested_fixture_api() {}\n",
    );
    context::refresh(root).unwrap();

    let map = build_readonly(root).unwrap();
    let quality = quality_report(&map);

    let finding = quality
        .findings
        .iter()
        .find(|finding| finding.path == "evals/tasks/sample/fixture/src/lib.rs")
        .unwrap();
    assert_eq!(finding.kind, "public_api_without_test_evidence");
    assert_eq!(finding.role, "fixture");
    assert_eq!(
        quality.findings_by_role["fixture"].findings,
        quality
            .findings
            .iter()
            .filter(|item| item.role == "fixture")
            .count()
    );
}

#[test]
fn strict_profile_blocks_large_file_by_default_while_standard_does_not() {
    // `strict`/`release` used to be functionally identical to `standard`
    // (both had empty `block_warning_kinds`), so the release gate provided no
    // extra rigor unless a workspace hand-wrote `.RaymanCodingSkill/quality.json`.
    // `strict()` must now block on its own by default.
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let big_body = "// pad\n".repeat(2_001);
    write(root.join("src/lib.rs").as_path(), &big_body);
    context::refresh(root).unwrap();

    let map = build_readonly(root).unwrap();

    let standard = quality_report_with_config(&map, &QualityConfig::standard());
    assert!(standard.ready, "standard findings={:?}", standard.findings);

    let strict = quality_report_with_config(&map, &QualityConfig::strict());
    assert!(!strict.ready);
    assert!(strict.findings.iter().any(|finding| {
        finding.kind == "large_file" && finding.path == "src/lib.rs" && finding.severity == "error"
    }));
}

#[test]
fn strict_quality_file_adds_to_defaults_instead_of_replacing_them() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root.join(".RaymanCodingSkill/quality.json").as_path(),
        r#"{
  "block_warning_kinds": ["public_api_without_test_evidence"]
}
"#,
    );

    let config = load_quality_config(root, "strict").unwrap();

    assert!(
        config
            .block_warning_kinds
            .contains(&"large_file".to_string())
    );
    assert!(
        config
            .block_warning_kinds
            .contains(&"high_fan_in".to_string())
    );
    assert!(
        config
            .block_warning_kinds
            .contains(&"public_api_without_test_evidence".to_string())
    );
    assert_eq!(
        config.configured_block_warning_kinds,
        vec!["public_api_without_test_evidence".to_string()]
    );
}

#[test]
fn strict_quality_file_cannot_raise_the_missing_test_threshold() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root.join(".RaymanCodingSkill/quality.json").as_path(),
        r#"{
  "multi_source_no_test_min_sources": 999999
}
"#,
    );

    let config = load_quality_config(root, "strict").unwrap();

    assert_eq!(config.multi_source_no_test_min_sources, 3);
}

#[test]
fn strict_quality_exact_exemption_preserves_finding_and_records_provenance() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let big_body = "// pad\n".repeat(2_001);
    write(root.join("src/lib.rs").as_path(), &big_body);
    write(
        root.join(".RaymanCodingSkill/quality.json").as_path(),
        r#"{
  "exemptions": [
    {
      "path": "src/lib.rs",
      "kind": "large_file",
      "reason": "Generated compatibility table is reviewed and covered by package tests."
    }
  ]
}
"#,
    );
    context::refresh(root).unwrap();

    let map = build_readonly(root).unwrap();
    let config = load_quality_config(root, "strict").unwrap();
    let quality = quality_report_with_config(&map, &config);

    assert!(quality.ready, "findings={:?}", quality.findings);
    let finding = quality
        .findings
        .iter()
        .find(|finding| finding.path == "src/lib.rs" && finding.kind == "large_file")
        .unwrap();
    assert_eq!(finding.severity, "info");
    assert_eq!(
        finding.blocking_policy_source.as_deref(),
        Some("strict_default")
    );
    assert!(finding.exemption_reason.is_some());
}

#[test]
fn strict_quality_exemptions_reject_globs_and_blank_reasons() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root.join(".RaymanCodingSkill/quality.json").as_path(),
        r#"{
  "exemptions": [
    { "path": "evals/tasks/*/fixture/src", "kind": "high_fan_in", "reason": "broad" }
  ]
}
"#,
    );
    let glob_error = load_quality_config(root, "strict").unwrap_err().to_string();
    assert!(glob_error.contains("exact normalized workspace-relative file path"));

    write(
        root.join(".RaymanCodingSkill/quality.json").as_path(),
        r#"{
  "exemptions": [
    { "path": "src/lib.rs", "kind": "large_file", "reason": "   " }
  ]
}
"#,
    );
    let reason_error = load_quality_config(root, "strict").unwrap_err().to_string();
    assert!(reason_error.contains("requires a non-empty reason"));
}

#[test]
fn strict_quality_exemptions_require_an_existing_ordinary_file() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    write(
        root.join(".RaymanCodingSkill/quality.json").as_path(),
        r#"{
  "exemptions": [
    { "path": "src", "kind": "large_file", "reason": "directory is too broad" }
  ]
}
"#,
    );
    let directory_error = load_quality_config(root, "strict").unwrap_err().to_string();
    assert!(
        directory_error.contains("ordinary file"),
        "{directory_error}"
    );

    write(
        root.join(".RaymanCodingSkill/quality.json").as_path(),
        r#"{
  "exemptions": [
    { "path": "src/missing.rs", "kind": "large_file", "reason": "future file" }
  ]
}
"#,
    );
    let missing_error = load_quality_config(root, "strict").unwrap_err().to_string();
    assert!(
        missing_error.contains("existing ordinary file"),
        "{missing_error}"
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

#[test]
fn python_map_links_pyproject_imports_and_pytest_targets() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root.join("pyproject.toml").as_path(),
        "[project]\nname = \"demo-python\"\n",
    );
    write(
        root.join("app/service.py").as_path(),
        "def run():\n    return 1\n",
    );
    write(
        root.join("app/api.py").as_path(),
        "from app.service import run\ndef handler():\n    return run()\n",
    );
    write(
        root.join("app/worker.py").as_path(),
        "from app.service import run\ndef work():\n    return run()\n",
    );
    write(
        root.join("tests/test_api.py").as_path(),
        "from app.api import handler\ndef test_handler():\n    assert handler() == 1\n",
    );
    context::refresh(root).unwrap();

    let map = build_readonly(root).unwrap();
    assert!(map.packages.iter().any(|package| {
        package.name == "demo-python" && package.manifest_path == "pyproject.toml"
    }));
    assert!(map.dependencies.iter().any(|dependency| {
        dependency.from_path == "app/api.py"
            && dependency.to_path == "app/service.py"
            && dependency.kind == "python_import"
    }));
    let impact = impact_report(&map, "app/api.py").unwrap();
    assert_eq!(impact.package.as_deref(), Some("demo-python"));
    assert_eq!(impact.manifest_path.as_deref(), Some("pyproject.toml"));
    assert!(
        impact
            .related_tests
            .iter()
            .any(|test| test.path == "tests/test_api.py"
                && test.basis == "python_import_graph"
                && test.confidence == "high")
    );
    assert!(
        impact
            .recommended_checks
            .iter()
            .any(|check| check == "python -m pytest")
    );
    let plan = change_plan(
        &map,
        &[
            "app/api.py".into(),
            "app/service.py".into(),
            "app/worker.py".into(),
        ],
    )
    .unwrap();
    assert!(plan.ready, "{plan:?}");
}

/// 同名 stem 回退不得给测试从未 import 的文件铸造 import-graph/high 覆盖：
/// import 命中时回退必须禁用（否则回退候选挂上 import_graph/high 假标签，
/// 验证门禁据此接受假覆盖 receipt）；未命中时回退只在测试的包前缀内匹配。
#[test]
fn python_same_stem_files_in_other_packages_are_not_marked_covered() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root.join("pyproject.toml").as_path(),
        "[project]\nname = \"stems\"\n",
    );
    write(
        root.join("app1/models.py").as_path(),
        "def one():\n    return 1\n",
    );
    write(
        root.join("app2/models.py").as_path(),
        "def two():\n    return 2\n",
    );
    write(
        root.join("tests/test_models.py").as_path(),
        "from app1.models import one\ndef test_one():\n    assert one() == 1\n",
    );
    context::refresh(root).unwrap();

    let map = build_readonly(root).unwrap();
    let covered = impact_report(&map, "app1/models.py").unwrap();
    assert!(covered.related_tests.iter().any(|test| {
        test.path == "tests/test_models.py"
            && test.basis == "python_import_graph"
            && test.confidence == "high"
    }));
    let uncovered = impact_report(&map, "app2/models.py").unwrap();
    assert!(
        !uncovered
            .related_tests
            .iter()
            .any(|test| test.path == "tests/test_models.py"),
        "测试只 import 了 app1.models，app2/models.py 不得被标为已覆盖: {:?}",
        uncovered.related_tests
    );
}

#[test]
fn python_plan_blocks_broad_source_change_without_pytest_anchor() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root.join("pyproject.toml").as_path(),
        "[project]\nname = \"uncovered\"\n",
    );
    for name in ["a", "b", "c"] {
        write(
            root.join(format!("pkg/{name}.py")).as_path(),
            &format!("def {name}():\n    return 1\n"),
        );
    }
    context::refresh(root).unwrap();
    let map = build_readonly(root).unwrap();
    let plan = change_plan(
        &map,
        &["pkg/a.py".into(), "pkg/b.py".into(), "pkg/c.py".into()],
    )
    .unwrap();
    assert!(!plan.ready);
    assert!(
        plan.blockers
            .iter()
            .any(|blocker| blocker.contains("3 source files"))
    );
}
#[test]
fn nested_pyproject_imports_resolve_against_the_package_root() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root.join("packages/api/pyproject.toml").as_path(),
        "[project]\nname = \"nested-api\"\n",
    );
    write(
        root.join("packages/api/src/api/core.py").as_path(),
        "def value():\n    return 1\n",
    );
    write(
        root.join("packages/api/tests/test_behavior.py").as_path(),
        "from api import core\ndef test_behavior():\n    assert core.value() == 1\n",
    );
    context::refresh(root).unwrap();

    let map = build_readonly(root).unwrap();
    assert!(map.dependencies.iter().any(|dependency| {
        dependency.from_path == "packages/api/tests/test_behavior.py"
            && dependency.to_path == "packages/api/src/api/core.py"
            && dependency.kind == "python_import"
    }));
    let impact = impact_report(&map, "packages/api/src/api/core.py").unwrap();
    assert_eq!(impact.package.as_deref(), Some("nested-api"));
    assert!(impact.related_tests.iter().any(|test| {
        test.path == "packages/api/tests/test_behavior.py" && test.basis == "python_import_graph"
    }));
    assert!(
        impact
            .recommended_checks
            .iter()
            .any(|check| check == "python -m pytest packages/api")
    );
}

/// Package source/test counts used to include every indexed language, so one
/// `tests/*.py` inside a Cargo package satisfied `package_has_test_anchor` and
/// silenced `multi_source_project_without_tests` — the only *blocking* quality
/// error — on a package with zero Rust tests.
#[test]
fn package_counts_ignore_files_the_package_toolchain_never_builds() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        &root.join("Cargo.toml"),
        "[package]\nname = \"rusty\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    for module in ["a", "b", "c"] {
        write(
            &root.join(format!("src/{module}.rs")),
            &format!("pub fn {module}() {{}}\n"),
        );
    }

    // A foreign-language test file must not count as this package's test anchor.
    write(
        &root.join("tests/test_smoke.py"),
        "def test_smoke():\n    pass\n",
    );

    crate::context::refresh(root).unwrap();
    let map = build(root).unwrap();
    let package = map
        .packages
        .iter()
        .find(|package| package.name == "rusty")
        .expect("the Cargo package must be mapped");

    assert_eq!(package.source_files, 3, "only .rs sources count");
    assert_eq!(
        package.test_files, 0,
        "a .py test is not a Rust test anchor"
    );
    assert!(!package_has_test_anchor(&map, package));
}

/// "cargo could not be run" and "the topology is untrustworthy" both fail
/// closed, but only one of them is fixed by the operator's PATH. `check` used
/// to emit the same opaque blocker for both, with the actionable half buried at
/// the tail of a provenance string.
#[test]
fn a_missing_cargo_is_reported_as_an_environment_boundary() {
    assert!(topology_blocked_by_missing_cargo(&format!(
        "heuristic_fallback: {TOPOLOGY_TOOL_UNAVAILABLE}: cargo 不在本进程 PATH 中"
    )));
    // A manifest that cargo itself rejected is a repository defect, not a PATH
    // problem, and must not be relabelled as one.
    assert!(!topology_blocked_by_missing_cargo(
        "heuristic_fallback: cargo metadata 失败: error: failed to parse manifest"
    ));
    assert!(!topology_blocked_by_missing_cargo("cargo_metadata"));
}
