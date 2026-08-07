use std::fs;
use std::path::Path;
use std::process::Command;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

#[test]
fn audit_self_test_exercises_exact_isolated_msrv_contract() {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repository root must resolve");
    let fixture = tempfile::tempdir().expect("audit self-test fixture must be created");
    let fixture_bin = fixture.path().join("bin");
    let process_temp = fixture.path().join("process-temp");
    fs::create_dir_all(&fixture_bin).expect("fixture bin must be created");
    fs::create_dir_all(&process_temp).expect("fixture process temp must be created");

    #[cfg(windows)]
    let cargo_deny = fixture_bin.join("cargo-deny.cmd");
    #[cfg(not(windows))]
    let cargo_deny = fixture_bin.join("cargo-deny");
    #[cfg(windows)]
    fs::write(&cargo_deny, "@exit /b 0\r\n").expect("cargo-deny fixture must be written");
    #[cfg(not(windows))]
    {
        fs::write(&cargo_deny, "#!/bin/sh\nexit 0\n").expect("cargo-deny fixture must be written");
        let mut permissions = fs::metadata(&cargo_deny)
            .expect("cargo-deny fixture metadata must resolve")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&cargo_deny, permissions)
            .expect("cargo-deny fixture must be executable");
    }

    let ambient_path = std::env::var_os("PATH").unwrap_or_default();
    let path = std::env::join_paths(
        std::iter::once(fixture_bin.clone()).chain(std::env::split_paths(&ambient_path)),
    )
    .expect("fixture PATH must be representable");
    let script = repo_root.join("scripts/audit-repository.ps1");
    let output = Command::new("pwsh")
        .args([
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
            script.to_str().expect("audit script path must be UTF-8"),
            "-SelfTest",
        ])
        .current_dir(&repo_root)
        .env("PATH", path)
        .env("TMP", &process_temp)
        .env("TEMP", &process_temp)
        .output()
        .expect("PowerShell 7 must run the audit self-test");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "audit self-test failed\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("exact isolated MSRV")
            && stdout.contains("audit-repository.ps1 self-test passed."),
        "audit self-test did not exercise the exact isolated MSRV contract\nstdout:\n{stdout}"
    );
}
