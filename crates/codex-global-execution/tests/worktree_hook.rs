#[cfg(windows)]
#[test]
fn session_start_hook_errors_are_informational_and_unrelated_projects_are_untouched() {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let fixture = tempfile::tempdir().unwrap();
    let workspace = fixture.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    std::fs::create_dir(workspace.join(".git")).unwrap();
    std::fs::write(workspace.join("keep.txt"), b"unchanged\r\n").unwrap();
    let root = fixture.path().join("uninstalled-backend");
    let normal = serde_json::json!({"hook_event_name":"SessionStart","cwd":workspace,"permission_mode":"default","transcript_path":"must-not-be-opened"});
    let plan = serde_json::json!({"hook_event_name":"SessionStart","cwd":"missing","permission_mode":"plan"});
    let unrelated = serde_json::json!({"hook_event_name":"UserPromptSubmit","cwd":"missing"});
    let malformed = br#"{"hook_event_name":"SessionStart","cwd":"a","cwd":"b"}"#.to_vec();
    let oversized = vec![b' '; 256 * 1024 + 1];
    for (bytes, diagnostic) in [
        (serde_json::to_vec(&normal).unwrap(), false),
        (serde_json::to_vec(&plan).unwrap(), false),
        (serde_json::to_vec(&unrelated).unwrap(), false),
        (malformed, true),
        (oversized, true),
    ] {
        let mut child = Command::new(env!("CARGO_BIN_EXE_rayman-global"))
            .args(["worktree-hook", "--root"])
            .arg(&root)
            .current_dir(&workspace)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(&bytes).unwrap();
        let result = child.wait_with_output().unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let value: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
        assert_eq!(value["hookSpecificOutput"]["hookEventName"], "SessionStart");
        assert_eq!(
            !value["hookSpecificOutput"]["additionalContext"]
                .as_str()
                .unwrap()
                .is_empty(),
            diagnostic
        );
        assert!(value.get("continue").is_none());
        assert!(!root.exists());
        assert!(!workspace.join(".agent-checkpoints").exists());
        assert_eq!(
            std::fs::read(workspace.join("keep.txt")).unwrap(),
            b"unchanged\r\n"
        );
    }
}

#[cfg(windows)]
#[test]
fn bootstrap_script_rejects_foreign_routes_and_ambiguous_checkpoint_recovery() {
    use std::process::Command;
    let fixture = tempfile::tempdir().unwrap();
    let driver = fixture.path().join("driver.ps1");
    std::fs::write(&driver, r#"
param([string]$Source,[string]$Fixture)
$ErrorActionPreference='Stop'
. $Source -Library
$instance='a'*32;$worktree='b'*32;$first='c'*32;$second='d'*32
$valid=@{schema='rayman.global-state-routing.v1';installation_id=$instance;worktree_id=$worktree}
Assert-WorktreeRoute ([pscustomobject]$valid) $instance $worktree
$rejected=0
foreach($field in @('schema','installation_id','worktree_id')){
    $bad=$valid.Clone();$bad[$field]='wrong'
    try{Assert-WorktreeRoute ([pscustomobject]$bad) $instance $worktree;throw 'accepted foreign route'}catch{if($_.Exception.Message -eq 'accepted foreign route'){throw};$rejected++}
}
function Put-Record([string]$Id,[string]$State){
    $path=Join-Path $Fixture ('checkpoint-migration-'+$worktree+'-'+$Id+'.json')
    [IO.File]::WriteAllText($path,(@{schema='rayman.checkpoint-migration.v1';worktree_id=$worktree;transaction_id=$Id;state=$State}|ConvertTo-Json))
}
Put-Record $first 'prepared'
if((Get-WorktreeCheckpointTransaction $Fixture $worktree) -cne $first){throw 'lost pending transaction'}
Put-Record $second 'publishing'
try{[void](Get-WorktreeCheckpointTransaction $Fixture $worktree);throw 'accepted ambiguous migration'}catch{if($_.Exception.Message -eq 'accepted ambiguous migration'){throw};$rejected++}
Put-Record $second 'committed'
if((Get-WorktreeCheckpointTransaction $Fixture $worktree) -cne $first){throw 'terminal record obscures recovery'}
Put-Record $first 'unknown'
try{[void](Get-WorktreeCheckpointTransaction $Fixture $worktree);throw 'accepted unknown migration'}catch{if($_.Exception.Message -eq 'accepted unknown migration'){throw};$rejected++}
@{rejected=$rejected;passed=$true}|ConvertTo-Json
"#).unwrap();
    let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../scripts/bootstrap-global-codex-worktree.ps1");
    let result = Command::new("C:/Program Files/PowerShell/7/pwsh.exe")
        .args(["-NoProfile", "-NonInteractive", "-File"])
        .arg(&driver)
        .arg(&source)
        .arg(fixture.path())
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(value["rejected"], 5);
    assert_eq!(value["passed"], true);
}
