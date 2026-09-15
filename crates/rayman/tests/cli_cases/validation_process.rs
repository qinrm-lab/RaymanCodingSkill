use super::*;

#[cfg(windows)]
#[test]
fn generic_validation_child_can_run_nested_goal_validate_from_its_managed_temp() {
    let probe = NestedValidationProbe::build();
    let workspace = tempfile::tempdir().unwrap();
    let root = workspace.path();
    write(root, "README.md", "generic nested validation fixture\n");
    run_json(root, &["context", "refresh"]);
    let goal = run_json(
        root,
        &[
            "goal",
            "start",
            "generic nested validation",
            "--must-proof",
            "generic::run a nested validation",
        ],
    );
    let id = goal["id"].as_str().unwrap();

    let evidence_home = tempfile::tempdir().unwrap();
    let host_temp_root = evidence_home.path().join("host-temp");
    std::fs::create_dir_all(&host_temp_root).unwrap();
    let canonical_host_temp_root = canonical_display_path(&host_temp_root);
    let trace = evidence_home.path().join("nested-validation.trace");
    let skill = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("SKILL.md")
        .canonicalize()
        .unwrap();
    let logical_command = format!("\"{}\" outer", probe.executable.display());
    let output = run_with_path_and_env(
        root,
        &[
            "--format",
            "json",
            "goal",
            "validate",
            id,
            "--req",
            "req_1",
            "-m",
            "generic child reentered goal validate",
            "--command",
            &logical_command,
            "--changed",
            "README.md",
        ],
        &[],
        None,
        &[
            (
                "RAYMAN_VALIDATION_TEMP_ROOT",
                host_temp_root.to_str().unwrap(),
            ),
            ("RAYMAN_NESTED_RAYMAN", BIN),
            ("RAYMAN_NESTED_SKILL", skill.to_str().unwrap()),
            ("RAYMAN_NESTED_TRACE", trace.to_str().unwrap()),
        ],
    );
    assert_eq!(
        output.status,
        0,
        "stdout={}\nstderr={}\ntrace={}",
        output.stdout,
        output.stderr,
        std::fs::read_to_string(&trace).unwrap_or_default()
    );

    let records = nested_validation_trace(&trace);
    assert_eq!(
        records
            .iter()
            .map(|(phase, _, _)| phase.as_str())
            .collect::<Vec<_>>(),
        ["outer", "inner"],
        "records={records:?}"
    );
    let (_, outer_temp, outer_nested) = &records[0];
    let (_, inner_temp, inner_nested) = &records[1];
    let outer_lease_root = outer_temp.parent().unwrap();
    let inner_lease_root = inner_temp.parent().unwrap();
    assert!(
        outer_lease_root.starts_with(canonical_host_temp_root.join("v")),
        "outer lease escaped configured root: {}",
        outer_lease_root.display()
    );
    assert_eq!(outer_nested, &outer_lease_root.join("n"));
    assert!(
        inner_lease_root.starts_with(outer_nested.join("v")),
        "inner lease escaped the parent nested root: {}",
        inner_lease_root.display()
    );
    assert_eq!(inner_nested, &inner_lease_root.join("n"));
    for path in [outer_temp, outer_nested, inner_temp, inner_nested] {
        assert!(
            !path.exists(),
            "successful nested validation left {}",
            path.display()
        );
    }
    assert!(!host_temp_root.join(".RaymanCodingSkill").exists());

    let persisted: Value = serde_json::from_str(
        &std::fs::read_to_string(
            root.join(".RaymanCodingSkill/goals")
                .join(format!("{id}.json")),
        )
        .unwrap(),
    )
    .unwrap();
    visit_json_strings(&persisted, &mut |text| {
        for (_, temp, nested) in &records {
            assert!(!text.contains(temp.to_string_lossy().as_ref()), "{text}");
            assert!(!text.contains(nested.to_string_lossy().as_ref()), "{text}");
            for path in [temp, nested] {
                let lease_id = path
                    .parent()
                    .and_then(Path::file_name)
                    .unwrap()
                    .to_string_lossy();
                assert!(!text.contains(lease_id.as_ref()), "{text}");
            }
        }
    });
}

#[cfg(windows)]
#[test]
fn goal_validate_self_hosted_gate_uses_one_managed_target_without_rewriting_running_cli() {
    let trace_home = tempfile::tempdir().unwrap();
    // The actual Cargo invocation below protects this layout from drifting
    // back to the verbose form that exhausts MSVC's practical path budget.
    let workspace = tempfile::tempdir().unwrap();
    let root = workspace.path();
    let trace = trace_home.path().join("self-hosted-target.trace");
    write(
        root,
        "Cargo.toml",
        "[package]\nname = \"self-hosted-validation-fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\nbuild = \"build.rs\"\n\n[[bin]]\nname = \"rayman\"\npath = \"src/main.rs\"\n",
    );
    write(root, "src/main.rs", "fn main() {}\n");
    write(
        root,
        "build.rs",
        r#"use std::fs::OpenOptions;
use std::io::Write;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let trace = std::env::var("RAYMAN_SELF_HOST_TRACE").unwrap();
    let target = std::env::var("CARGO_TARGET_DIR").unwrap();
    let temp = std::env::var("TEMP").unwrap();
    let mut file = OpenOptions::new().create(true).append(true).open(trace).unwrap();
    writeln!(file, "build\t{target}\t{temp}").unwrap();
}
"#,
    );
    write(
        root,
        "tests/target_trace.rs",
        r#"use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;

#[test]
fn records_the_managed_target() {
    assert!(Path::new(env!("CARGO_BIN_EXE_rayman")).is_file());
    let trace = std::env::var("RAYMAN_SELF_HOST_TRACE").unwrap();
    let target = std::env::var("CARGO_TARGET_DIR").unwrap();
    let temp = std::env::var("TEMP").unwrap();
    let mut file = OpenOptions::new().create(true).append(true).open(trace).unwrap();
    writeln!(file, "run\t{target}\t{temp}").unwrap();
}
"#,
    );
    generate_lockfile(root);
    run_json(root, &["context", "refresh"]);
    let goal = run_json(
        root,
        &[
            "goal",
            "start",
            "self-hosted gate",
            "--must-proof",
            "test::run the self-hosted gate",
        ],
    );
    let id = goal["id"].as_str().unwrap();

    let collision_target = root.join(".RaymanCodingSkill/tmp/collision-target");
    let copied_cli = collision_target.join("debug/rayman.exe");
    std::fs::create_dir_all(copied_cli.parent().unwrap()).unwrap();
    std::fs::copy(BIN, &copied_cli).unwrap();
    let original_bytes = std::fs::read(&copied_cli).unwrap();
    let canonical_root = canonical_display_path(root);
    let canonical_collision_target = canonical_display_path(&collision_target);
    let collision_text = collision_target.to_str().unwrap();
    let trace_text = trace.to_str().unwrap();
    let host_temp_root = trace_home.path().join("rayman-host-temp");
    let inherited_temp = trace_home.path().join("inherited-temp");
    std::fs::create_dir(&host_temp_root).unwrap();
    std::fs::create_dir(&inherited_temp).unwrap();
    let canonical_host_temp_root = canonical_display_path(&host_temp_root);
    let host_temp_text = host_temp_root.to_str().unwrap();
    let inherited_temp_text = inherited_temp.to_str().unwrap();
    let logical_command = "cargo test --workspace --all-targets";
    let output = run_binary_with_env(
        &copied_cli,
        root,
        &[
            "--format",
            "json",
            "goal",
            "validate",
            id,
            "--req",
            "req_1",
            "-m",
            "self-hosted gate stayed isolated",
            "--command",
            logical_command,
            "--workspace-snapshot",
            "--authority",
            "--repeat",
            "2",
        ],
        &[
            ("CARGO_TARGET_DIR", collision_text),
            ("RAYMAN_SELF_HOST_TRACE", trace_text),
            ("RAYMAN_VALIDATION_TEMP_ROOT", host_temp_text),
            ("TEMP", inherited_temp_text),
            ("TMP", inherited_temp_text),
            ("TMPDIR", inherited_temp_text),
        ],
    );
    assert_eq!(
        output.status,
        0,
        "stdout={}\nstderr={}\ntrace={}",
        output.stdout,
        output.stderr,
        std::fs::read_to_string(&trace).unwrap_or_default()
    );
    assert_eq!(std::fs::read(&copied_cli).unwrap(), original_bytes);
    assert!(!collision_target.join("debug/deps").exists());
    assert!(!collision_target.join(".rustc_info.json").exists());

    let records = std::fs::read_to_string(&trace)
        .unwrap()
        .lines()
        .map(|line| {
            let mut fields = line.split('\t');
            let phase = fields.next().unwrap();
            let target = fields.next().unwrap();
            let temp = fields.next().unwrap();
            assert!(fields.next().is_none(), "unexpected trace row: {line}");
            (
                phase.to_string(),
                PathBuf::from(target),
                PathBuf::from(temp),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        records
            .iter()
            .map(|(phase, _, _)| phase.as_str())
            .collect::<Vec<_>>(),
        ["build", "run", "run"],
        "list proof and both repeats must be observable: {records:?}"
    );
    assert!(
        records.windows(2).all(|pair| pair[0].1 == pair[1].1),
        "list proof and repeats must reuse one target: {records:?}"
    );
    let process_temps = records
        .iter()
        .map(|(_, _, temp)| temp.clone())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        process_temps.len(),
        3,
        "list proof and repeats must use independent process temp leases: {records:?}"
    );
    for process_temp in &process_temps {
        assert!(
            process_temp.starts_with(canonical_host_temp_root.join("v")),
            "validation process temp escaped the configured root: {}",
            process_temp.display()
        );
        assert_eq!(
            process_temp.file_name().and_then(|name| name.to_str()),
            Some("t"),
            "validation process temp did not use the compact child alias: {}",
            process_temp.display()
        );
        let process_lease = process_temp.parent().unwrap();
        let compact_process_parent = canonical_host_temp_root.join("v");
        assert_eq!(
            process_lease.parent(),
            Some(compact_process_parent.as_path()),
            "validation process lease did not use the compact parent alias: {}",
            process_temp.display()
        );
        assert!(
            process_lease
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|id| id.starts_with("v-")),
            "validation process lease id did not use its compact label: {}",
            process_lease.display()
        );
        assert!(
            !process_temp.exists(),
            "validation process lease was not released: {}",
            process_temp.display()
        );
    }
    assert!(
        !host_temp_root.join(".RaymanCodingSkill").exists(),
        "external validation root became a false workspace marker"
    );
    let managed_target = &records[0].1;
    assert!(
        managed_target.starts_with(canonical_root.join(".RaymanCodingSkill/tmp/c")),
        "unexpected managed target: {}",
        managed_target.display()
    );
    assert_eq!(
        managed_target.file_name().and_then(|name| name.to_str()),
        Some("t"),
        "managed Cargo target did not use the compact child alias: {}",
        managed_target.display()
    );
    let target_lease = managed_target.parent().unwrap();
    let compact_target_parent = canonical_root.join(".RaymanCodingSkill/tmp/c");
    assert_eq!(
        target_lease.parent(),
        Some(compact_target_parent.as_path()),
        "managed Cargo target did not use the compact parent alias: {}",
        managed_target.display()
    );
    let target_lease_id = target_lease
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap();
    let legacy_lease_id = format!(
        "cargo-{}",
        target_lease_id
            .strip_prefix("c-")
            .expect("compact Cargo lease label")
    );
    let legacy_target = canonical_root
        .join(".RaymanCodingSkill/tmp/cargo-target-leases")
        .join(legacy_lease_id)
        .join("target");
    assert!(
        legacy_target.to_string_lossy().len() >= managed_target.to_string_lossy().len() + 27,
        "compact Cargo layout did not recover the required Windows path budget: managed={} legacy={}",
        managed_target.display(),
        legacy_target.display()
    );
    assert!(!managed_target.starts_with(&canonical_collision_target));
    assert!(!managed_target.to_string_lossy().starts_with(r"\\?\"));
    assert!(
        !managed_target.exists(),
        "successful validation did not release {}",
        managed_target.display()
    );

    let returned: Value = serde_json::from_str(&output.stdout).unwrap();
    let validated: rayman::goal::Goal = serde_json::from_value(returned).unwrap();
    let validation = &validated.requirements[0].validations[0];
    let authority = &validated.authority_receipts[0];
    assert_eq!(validation.command, logical_command);
    assert_eq!(authority.command, logical_command);
    assert_eq!(authority.repeat, 2);
    assert_eq!(authority.runs.len(), 2);
    let receipt = validation.receipt.as_ref().unwrap();
    assert_eq!(receipt.listed_tests, Some(1));
    assert_eq!(receipt.passed_tests, Some(1));
    assert_eq!(
        receipt.invocation_sha256,
        rayman::goal::validation_invocation_sha256_scoped_mode(
            logical_command,
            &validation.impact_scopes,
            validation.non_code,
            validation.workspace_snapshot,
        )
    );
    assert_eq!(
        authority.invocation_sha256,
        rayman::goal::authority_invocation_sha256_mode(
            logical_command,
            "req_1",
            2,
            &authority.impact_scopes,
            authority.non_code,
            authority.workspace_snapshot,
        )
    );
    let lease_id = managed_target
        .parent()
        .and_then(Path::file_name)
        .and_then(|value| value.to_str())
        .unwrap();
    let process_lease_ids = process_temps
        .iter()
        .filter_map(|temp| temp.parent())
        .filter_map(Path::file_name)
        .filter_map(|value| value.to_str())
        .collect::<Vec<_>>();

    let persisted_path = root
        .join(".RaymanCodingSkill/goals")
        .join(format!("{id}.json"));
    let persisted: Value =
        serde_json::from_str(&std::fs::read_to_string(persisted_path).unwrap()).unwrap();
    visit_json_strings(&persisted, &mut |text| {
        assert!(
            !text.contains(collision_text),
            "collision target leaked: {text}"
        );
        assert!(
            !text.contains(managed_target.to_string_lossy().as_ref()),
            "managed target leaked: {text}"
        );
        assert!(
            !text.contains(lease_id),
            "Cargo target lease id leaked: {text}"
        );
        assert!(
            !text.contains(copied_cli.to_string_lossy().as_ref()),
            "copied CLI leaked: {text}"
        );
        for process_temp in &process_temps {
            assert!(
                !text.contains(process_temp.to_string_lossy().as_ref()),
                "validation process temp leaked: {text}"
            );
        }
        for process_lease_id in &process_lease_ids {
            assert!(
                !text.contains(process_lease_id),
                "validation process lease id leaked: {text}"
            );
        }
    });
}

#[cfg(windows)]
#[test]
fn goal_progress_uses_and_releases_a_fresh_validation_process_temp() {
    const SOURCE: &str = r#"
use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    let temp = PathBuf::from(env::var_os("TEMP").expect("TEMP"));
    let tmp = PathBuf::from(env::var_os("TMP").expect("TMP"));
    let tmpdir = PathBuf::from(env::var_os("TMPDIR").expect("TMPDIR"));
    assert_eq!(temp, tmp);
    assert_eq!(temp, tmpdir);
    assert!(temp.is_dir());
    fs::write(temp.join("child-probe.txt"), b"probed").unwrap();
    fs::write(
        env::var_os("RAYMAN_PROGRESS_TEMP_TRACE").expect("trace"),
        temp.to_string_lossy().as_bytes(),
    )
    .unwrap();
}
"#;

    let workspace = tempfile::tempdir().unwrap();
    let root = workspace.path();
    write(root, "src/lib.rs", "pub fn value() -> u8 { 1 }\n");
    run_json(root, &["context", "refresh"]);
    let goal = run_json(
        root,
        &["goal", "start", "progress temp", "--must", "deliver"],
    );
    let id = goal["id"].as_str().unwrap();
    run_json(
        root,
        &[
            "goal",
            "package",
            "add",
            id,
            "stage1",
            "focused stage",
            "--req",
            "req_1",
        ],
    );

    let probe_home = tempfile::tempdir().unwrap();
    let source = probe_home.path().join("progress-temp-probe.rs");
    let executable = probe_home.path().join("progress-temp-probe.exe");
    std::fs::write(&source, SOURCE).unwrap();
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let compiled = Command::new(rustc)
        .arg("--edition=2024")
        .arg(&source)
        .arg("-o")
        .arg(&executable)
        .output()
        .unwrap();
    assert!(
        compiled.status.success(),
        "progress probe did not compile: {}",
        String::from_utf8_lossy(&compiled.stderr)
    );

    let host_temp_root = probe_home.path().join("host-temp");
    let inherited_temp = probe_home.path().join("inherited-temp");
    std::fs::create_dir(&host_temp_root).unwrap();
    std::fs::create_dir(&inherited_temp).unwrap();
    let canonical_host_temp_root = canonical_display_path(&host_temp_root);
    let trace = probe_home.path().join("progress-temp.trace");
    let command = format!("\"{}\"", executable.display());
    let output = run_with_path_and_env(
        root,
        &[
            "--format",
            "json",
            "goal",
            "progress",
            id,
            "--package",
            "stage1",
            "-m",
            "managed process temp",
            "--command",
            &command,
        ],
        &[],
        None,
        &[
            (
                "RAYMAN_VALIDATION_TEMP_ROOT",
                host_temp_root.to_str().unwrap(),
            ),
            ("RAYMAN_PROGRESS_TEMP_TRACE", trace.to_str().unwrap()),
            ("TEMP", inherited_temp.to_str().unwrap()),
            ("TMP", inherited_temp.to_str().unwrap()),
            ("TMPDIR", inherited_temp.to_str().unwrap()),
        ],
    );
    assert_eq!(output.status, 0, "{}", output.stderr);

    let process_temp = PathBuf::from(std::fs::read_to_string(&trace).unwrap());
    assert!(
        process_temp.starts_with(canonical_host_temp_root.join("v")),
        "progress temp escaped configured host root: {}",
        process_temp.display()
    );
    assert_ne!(process_temp, inherited_temp);
    assert!(
        !process_temp.exists(),
        "progress validation process lease was not released: {}",
        process_temp.display()
    );
    assert!(!host_temp_root.join(".RaymanCodingSkill").exists());

    let persisted = std::fs::read_to_string(
        root.join(".RaymanCodingSkill/goals")
            .join(format!("{id}.json")),
    )
    .unwrap();
    assert!(!persisted.contains(process_temp.to_string_lossy().as_ref()));
    assert!(
        !persisted.contains(
            process_temp
                .parent()
                .and_then(Path::file_name)
                .unwrap()
                .to_string_lossy()
                .as_ref()
        )
    );
}

#[cfg(windows)]
#[test]
fn pytest_validation_child_can_run_nested_goal_validate_from_its_managed_temp() {
    let pytest = NativePytestProbe::build();
    let nested = NestedValidationProbe::build();
    let workspace = tempfile::tempdir().unwrap();
    let root = workspace.path();
    let id = start_pytest_validation_goal(root);
    write(root, "README.md", "pytest validation fixture changed\n");
    run_json(root, &["context", "refresh"]);
    let evidence_home = tempfile::tempdir().unwrap();
    let host_temp_root = evidence_home.path().join("pytest-host-temp");
    std::fs::create_dir(&host_temp_root).unwrap();
    let pytest_trace_path = evidence_home.path().join("pytest.trace");
    let nested_trace_path = evidence_home.path().join("nested.trace");
    let skill = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("SKILL.md")
        .canonicalize()
        .unwrap();

    let output = run_with_path_and_env(
        root,
        &[
            "--format",
            "json",
            "goal",
            "validate",
            &id,
            "--req",
            "req_1",
            "-m",
            "pytest child reentered goal validate",
            "--command",
            "pytest -q --",
            "--changed",
            "README.md",
        ],
        &[pytest.bin_dir.as_path()],
        pytest.pathext(),
        &[
            (
                "RAYMAN_VALIDATION_TEMP_ROOT",
                host_temp_root.to_str().unwrap(),
            ),
            (
                "RAYMAN_PYTEST_PROBE_LOG",
                pytest_trace_path.to_str().unwrap(),
            ),
            ("RAYMAN_PYTEST_PROBE_MODE", "success"),
            ("RAYMAN_NESTED_PROBE", nested.executable.to_str().unwrap()),
            ("RAYMAN_NESTED_RAYMAN", BIN),
            ("RAYMAN_NESTED_SKILL", skill.to_str().unwrap()),
            ("RAYMAN_NESTED_TRACE", nested_trace_path.to_str().unwrap()),
        ],
    );
    assert_eq!(
        output.status,
        0,
        "stdout={}\nstderr={}\nnested_trace={}",
        output.stdout,
        output.stderr,
        std::fs::read_to_string(&nested_trace_path).unwrap_or_default()
    );

    let pytest_records = pytest_trace(&pytest_trace_path);
    assert_eq!(
        pytest_records
            .iter()
            .map(|(phase, _)| phase.as_str())
            .collect::<Vec<_>>(),
        ["collect", "run"],
        "records={pytest_records:?}"
    );
    let pytest_run_root = pytest_records
        .iter()
        .find_map(|(phase, root)| (phase == "run").then_some(root))
        .unwrap();
    let records = nested_validation_trace(&nested_trace_path);
    assert_eq!(
        records
            .iter()
            .map(|(phase, _, _)| phase.as_str())
            .collect::<Vec<_>>(),
        ["outer", "inner"],
        "records={records:?}"
    );
    let (_, outer_temp, outer_nested) = &records[0];
    let (_, inner_temp, inner_nested) = &records[1];
    assert_eq!(outer_temp, &pytest_run_root.join("t"));
    assert_eq!(outer_nested, &pytest_run_root.join("n"));
    let inner_lease_root = inner_temp.parent().unwrap();
    assert!(
        inner_lease_root.starts_with(outer_nested.join("v")),
        "nested process lease escaped pytest lease: {}",
        inner_lease_root.display()
    );
    assert_eq!(inner_nested, &inner_lease_root.join("n"));
    for (_, lease_root) in &pytest_records {
        assert!(
            !lease_root.exists(),
            "pytest lease remained: {}",
            lease_root.display()
        );
    }
    for path in [outer_temp, outer_nested, inner_temp, inner_nested] {
        assert!(!path.exists(), "nested lease remained: {}", path.display());
    }
    assert_no_live_pytest_leases(&host_temp_root);
    assert!(!host_temp_root.join(".RaymanCodingSkill").exists());

    let persisted: Value = serde_json::from_str(
        &std::fs::read_to_string(
            root.join(".RaymanCodingSkill/goals")
                .join(format!("{id}.json")),
        )
        .unwrap(),
    )
    .unwrap();
    visit_json_strings(&persisted, &mut |text| {
        assert!(!text.contains("RAYMAN_VALIDATION_TEMP_ROOT"), "{text}");
        for (_, temp, nested_root) in &records {
            assert!(!text.contains(temp.to_string_lossy().as_ref()), "{text}");
            assert!(
                !text.contains(nested_root.to_string_lossy().as_ref()),
                "{text}"
            );
            let lease_id = temp
                .parent()
                .and_then(Path::file_name)
                .unwrap()
                .to_string_lossy();
            assert!(!text.contains(lease_id.as_ref()), "{text}");
        }
    });
}

#[test]
fn goal_validate_isolates_every_pytest_process_without_receipt_leakage() {
    let probe = NativePytestProbe::build();
    let commands = [
        "pytest -q --".to_string(),
        "python -m pytest -q --".to_string(),
        "py -3.12 -m pytest -q --".to_string(),
    ];
    for (index, command) in commands.iter().enumerate() {
        let command = command.as_str();
        let workspace = tempfile::tempdir().unwrap();
        let root = workspace.path();
        let id = start_pytest_validation_goal(root);
        let host_temp_root = probe._temp.path().join(format!("pytest-host-temp-{index}"));
        std::fs::create_dir(&host_temp_root).unwrap();
        #[cfg(windows)]
        let canonical_host_temp_root = canonical_display_path(&host_temp_root);
        let host_temp_text = host_temp_root.to_str().unwrap();
        let trace = probe._temp.path().join(format!("success-{index}.trace"));
        let trace_text = trace.to_str().unwrap();
        let output = run_with_path_and_env(
            root,
            &[
                "--format",
                "json",
                "goal",
                "validate",
                &id,
                "--req",
                "req_1",
                "-m",
                "managed pytest authority",
                "--command",
                command,
                "--workspace-snapshot",
                "--authority",
                "--repeat",
                "2",
            ],
            &[probe.bin_dir.as_path()],
            probe.pathext(),
            &[
                ("RAYMAN_PYTEST_PROBE_LOG", trace_text),
                ("RAYMAN_PYTEST_PROBE_MODE", "success"),
                (
                    "PYTEST_ADDOPTS",
                    "--basetemp inherited -o cache_dir=inherited",
                ),
                ("RAYMAN_VALIDATION_TEMP_ROOT", host_temp_text),
            ],
        );
        assert_eq!(
            output.status, 0,
            "command={command}\nstdout={}\nstderr={}",
            output.stdout, output.stderr
        );
        let returned: Value = serde_json::from_str(&output.stdout).unwrap();
        let goal: rayman::goal::Goal = serde_json::from_value(returned).unwrap();
        let validation = &goal.requirements[0].validations[0];
        let receipt = validation.receipt.as_ref().unwrap();
        let authority = &goal.authority_receipts[0];
        assert_eq!(validation.command, command);
        assert_eq!(authority.command, command);
        assert_eq!(authority.repeat, 2);
        assert_eq!(authority.runs.len(), 2);
        assert_eq!(
            receipt.invocation_sha256,
            rayman::goal::validation_invocation_sha256_scoped_mode(
                command,
                &validation.impact_scopes,
                validation.non_code,
                validation.workspace_snapshot,
            )
        );
        assert_eq!(
            authority.invocation_sha256,
            rayman::goal::authority_invocation_sha256_mode(
                command,
                "req_1",
                2,
                &authority.impact_scopes,
                authority.non_code,
                authority.workspace_snapshot,
            )
        );

        let records = pytest_trace(&trace);
        assert_eq!(
            records
                .iter()
                .map(|(phase, _)| phase.as_str())
                .collect::<Vec<_>>(),
            ["collect", "run", "run"],
            "command={command} records={records:?}"
        );
        let roots = records
            .iter()
            .map(|(_, lease_root)| lease_root.clone())
            .collect::<BTreeSet<_>>();
        assert_eq!(
            roots.len(),
            3,
            "every physical pytest process needs a new lease"
        );
        for lease_root in &roots {
            #[cfg(windows)]
            assert!(
                lease_root.starts_with(canonical_host_temp_root.join("p")),
                "Windows pytest lease escaped the configured host root: {}",
                lease_root.display()
            );
            assert!(
                !lease_root.exists(),
                "successful validation left {}",
                lease_root.display()
            );
        }
        assert_no_live_pytest_leases(if cfg!(windows) { &host_temp_root } else { root });
        assert!(
            !host_temp_root.join(".RaymanCodingSkill").exists(),
            "external pytest root became a false workspace marker"
        );

        let persisted_path = root
            .join(".RaymanCodingSkill/goals")
            .join(format!("{id}.json"));
        let persisted: Value =
            serde_json::from_str(&std::fs::read_to_string(persisted_path).unwrap()).unwrap();
        visit_json_strings(&persisted, &mut |text| {
            assert!(!text.contains("--basetemp"), "managed argv leaked: {text}");
            assert!(!text.contains("cache_dir="), "managed argv leaked: {text}");
            assert!(!text.contains("addopts="), "managed argv leaked: {text}");
            assert!(
                !text.contains("PYTHONPYCACHEPREFIX"),
                "managed environment leaked: {text}"
            );
            for lease_root in &roots {
                assert!(
                    !text.contains(lease_root.to_string_lossy().as_ref()),
                    "lease path leaked into goal JSON: {text}"
                );
                let lease_id = lease_root.file_name().unwrap().to_string_lossy();
                assert!(!text.contains(lease_id.as_ref()), "lease id leaked: {text}");
            }
        });
    }
}

#[test]
fn goal_validate_pytest_failures_cleanup_and_never_write_receipts() {
    let probe = NativePytestProbe::build();
    for (index, mode) in [
        "collect-fail",
        "collect-zero",
        "run-fail",
        "cleanup-fail",
        "run-cleanup-fail",
    ]
    .into_iter()
    .enumerate()
    {
        let workspace = tempfile::tempdir().unwrap();
        let root = workspace.path();
        let id = start_pytest_validation_goal(root);
        let host_temp_root = probe
            ._temp
            .path()
            .join(format!("pytest-failure-host-temp-{index}"));
        std::fs::create_dir(&host_temp_root).unwrap();
        let host_temp_text = host_temp_root.to_str().unwrap();
        let trace = probe._temp.path().join(format!("failure-{index}.trace"));
        let trace_text = trace.to_str().unwrap();
        let output = run_with_path_and_env(
            root,
            &[
                "goal",
                "validate",
                &id,
                "--req",
                "req_1",
                "-m",
                "pytest failure must not persist",
                "--command",
                "python -m pytest -q --",
                "--workspace-snapshot",
                "--authority",
                "--repeat",
                "2",
            ],
            &[probe.bin_dir.as_path()],
            probe.pathext(),
            &[
                ("RAYMAN_PYTEST_PROBE_LOG", trace_text),
                ("RAYMAN_PYTEST_PROBE_MODE", mode),
                ("RAYMAN_LANG", "zh-CN"),
                ("RAYMAN_VALIDATION_TEMP_ROOT", host_temp_text),
            ],
        );
        assert_eq!(
            output.status, 1,
            "mode={mode}\nstdout={}\nstderr={}",
            output.stdout, output.stderr
        );
        match mode {
            "collect-fail" => assert!(output.stderr.contains("exit=41"), "{}", output.stderr),
            "collect-zero" => assert!(
                output.stderr.contains("没有收集任何测试"),
                "{}",
                output.stderr
            ),
            "run-fail" => assert!(output.stderr.contains("exit=37"), "{}", output.stderr),
            "cleanup-fail" => assert!(
                output.stderr.contains("lease") && output.stderr.contains("释放"),
                "{}",
                output.stderr
            ),
            "run-cleanup-fail" => assert!(
                output.stderr.contains("exit=37") && output.stderr.contains("lease 释放失败"),
                "{}",
                output.stderr
            ),
            _ => unreachable!(),
        }

        let shown = run_json(root, &["goal", "show", &id]);
        assert_eq!(shown["requirements"][0]["status"], "open");
        assert!(
            shown["requirements"][0]["validations"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        assert!(shown["authority_receipts"].as_array().unwrap().is_empty());

        let records = pytest_trace(&trace);
        let cleanup_was_corrupted = matches!(mode, "cleanup-fail" | "run-cleanup-fail");
        for (record_index, (_, lease_root)) in records.iter().enumerate() {
            let final_record = record_index + 1 == records.len();
            if cleanup_was_corrupted && final_record {
                assert!(lease_root.exists(), "corrupt manifest must fail closed");
            } else {
                assert!(
                    !lease_root.exists(),
                    "mode={mode} left {}",
                    lease_root.display()
                );
            }
        }
        if !cleanup_was_corrupted {
            assert_no_live_pytest_leases(if cfg!(windows) { &host_temp_root } else { root });
        }
    }

    let workspace = tempfile::tempdir().unwrap();
    let root = workspace.path();
    let id = start_pytest_validation_goal(root);
    let empty_path = tempfile::tempdir().unwrap();
    let spawn_host_temp_root = probe._temp.path().join("pytest-spawn-host-temp");
    std::fs::create_dir(&spawn_host_temp_root).unwrap();
    let spawn_host_temp_text = spawn_host_temp_root.to_str().unwrap();
    let trace = probe._temp.path().join("spawn-failure.trace");
    let trace_text = trace.to_str().unwrap();
    let output = run_with_exact_path_and_env(
        root,
        &[
            "goal",
            "validate",
            &id,
            "--req",
            "req_1",
            "-m",
            "spawn failure must release",
            "--command",
            "pytest -q --",
            "--workspace-snapshot",
            "--authority",
            "--repeat",
            "2",
        ],
        empty_path.path(),
        probe.pathext(),
        &[
            ("RAYMAN_PYTEST_PROBE_LOG", trace_text),
            ("RAYMAN_VALIDATION_TEMP_ROOT", spawn_host_temp_text),
        ],
    );
    assert_eq!(output.status, 1, "{}", output.stderr);
    assert!(output.stderr.contains("pytest"), "{}", output.stderr);
    assert!(pytest_trace(&trace).is_empty());
    assert_no_live_pytest_leases(if cfg!(windows) {
        &spawn_host_temp_root
    } else {
        root
    });
    let shown = run_json(root, &["goal", "show", &id]);
    assert!(
        shown["requirements"][0]["validations"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(shown["authority_receipts"].as_array().unwrap().is_empty());
}

#[test]
fn pytest_lease_cli_is_manifest_owned_and_releasable() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let lease = run_json(root, &["temp", "pytest-lease", "focused tests"]);
    let id = lease["id"].as_str().unwrap();
    assert_eq!(lease["schema"], "rayman.pytest-lease.v1");
    let lease_root = Path::new(lease["root"].as_str().unwrap())
        .canonicalize()
        .unwrap();
    let expected_root = root
        .join(".RaymanCodingSkill/tmp/leases")
        .canonicalize()
        .unwrap();
    assert!(lease_root.starts_with(expected_root));
    assert_eq!(lease["pytest_args"].as_array().unwrap().len(), 4);
    let probed = run_json(root, &["temp", "pytest-probe", id]);
    assert_eq!(probed["id"], id);
    let released = run_json(root, &["temp", "pytest-release", id]);
    assert_eq!(released["removed"], true);
    let traversal = run(root, &["temp", "pytest-probe", "../outside"]);
    assert_eq!(traversal.status, 1);
}
