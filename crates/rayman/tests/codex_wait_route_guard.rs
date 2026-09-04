use std::path::{Path, PathBuf};
use std::process::Command;

fn powershell_path(path: &Path) -> String {
    let path = path.to_string_lossy();
    #[cfg(windows)]
    {
        if let Some(path) = path.strip_prefix(r"\\?\UNC\") {
            return format!(r"\\{path}");
        }
        if let Some(path) = path.strip_prefix(r"\\?\") {
            return path.to_owned();
        }
    }
    path.into_owned()
}

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repository root must resolve")
}

#[test]
fn codex_wait_route_guard_deterministic_suite() {
    let repo_root = repository_root();
    let script = powershell_path(&repo_root.join("scripts/codex-wait-route-guard.ps1"));
    let config = powershell_path(&repo_root.join(".codex/hooks.json"));
    assert!(
        Path::new(&config).is_file(),
        "project wait-route Hook must be materialized before validation"
    );

    let output = Command::new("pwsh")
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-File",
            &script,
            "-DeterministicTest",
            "-DeterministicTestConfigPath",
            &config,
        ])
        .current_dir(&repo_root)
        .output()
        .expect("wait-route deterministic PowerShell suite must start");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success() && stdout.contains("codex-wait-route-guard.ps1 self-test passed."),
        "wait-route deterministic suite failed\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
}
