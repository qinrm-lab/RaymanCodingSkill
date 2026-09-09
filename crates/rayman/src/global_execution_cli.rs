use crate::cli::{GlobalExecutionAction as A, GlobalExecutionCmd};
use anyhow::{Result, bail};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

pub fn run(json: bool, command: &GlobalExecutionCmd) -> Result<()> {
    let configured = std::env::var_os("RAYMAN_GLOBAL_EXECUTION_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("C:/ProgramData/Rayman/CodexGlobalExecution"));
    let root = match &command.action {
        A::Status { root }
        | A::RegisterInstallAdapter { root, .. }
        | A::Install { root, .. }
        | A::Initialize { root, .. }
        | A::Enroll { root, .. }
        | A::Result { root, .. }
        | A::Commit { root, .. }
        | A::Submit { root, .. }
        | A::Serve { root, .. } => root,
        _ => &configured,
    };
    let management = matches!(command.action, A::Initialize { .. } | A::Serve { .. });
    let program = root.join(if management {
        "worker.exe"
    } else {
        "client.exe"
    });
    if !program.is_file() {
        bail!(
            "Global execution is not installed at {}. Install the reviewed global package first.",
            root.display()
        );
    }
    let mut args: Vec<OsString> = vec![
        "--format".into(),
        if json { "json".into() } else { "text".into() },
    ];
    fn path(args: &mut Vec<OsString>, key: &str, value: &Path) {
        args.push(key.into());
        args.push(value.as_os_str().into());
    }
    match &command.action {
        A::RegisterInstallAdapter {
            root,
            workspace,
            specification,
            yes,
        } => {
            args.push("register-install-adapter".into());
            path(&mut args, "--root", root);
            path(&mut args, "--workspace", workspace);
            path(&mut args, "--specification", specification);
            if *yes {
                args.push("--yes".into());
            }
        }
        A::Install {
            root,
            workspace,
            adapter_id,
            version,
            sources,
            yes,
            timeout_seconds,
        } => {
            args.push("install".into());
            path(&mut args, "--root", root);
            path(&mut args, "--workspace", workspace);
            path(&mut args, "--sources", sources);
            args.extend([
                "--adapter-id".into(),
                adapter_id.into(),
                "--version".into(),
                version.into(),
                "--timeout-seconds".into(),
                timeout_seconds.to_string().into(),
            ]);
            if *yes {
                args.push("--yes".into());
            }
        }
        A::Status { root } => {
            args.push("status".into());
            path(&mut args, "--root", root);
        }
        A::Initialize {
            root,
            source_workspace,
            yes,
        } => {
            args.push("initialize".into());
            path(&mut args, "--root", root);
            path(&mut args, "--source-workspace", source_workspace);
            if *yes {
                args.push("--yes".into());
            }
        }
        A::Enroll {
            root,
            workspace,
            git,
            author_name,
            author_email,
            formal_state,
            yes,
        } => {
            args.push("enroll".into());
            path(&mut args, "--root", root);
            path(&mut args, "--workspace", workspace);
            if let Some(git) = git {
                path(&mut args, "--git", git);
            }
            if let Some(name) = author_name {
                args.extend(["--author-name".into(), name.into()]);
            }
            if let Some(email) = author_email {
                args.extend(["--author-email".into(), email.into()]);
            }
            if *formal_state {
                args.push("--formal-state".into());
            }
            if *yes {
                args.push("--yes".into());
            }
        }
        A::Result { root, request_id } => {
            args.push("result".into());
            path(&mut args, "--root", root);
            args.extend(["--request-id".into(), request_id.into()]);
        }
        A::Commit {
            root,
            workspace,
            paths,
            message,
            yes,
            timeout_seconds,
        } => {
            args.push("commit".into());
            path(&mut args, "--root", root);
            path(&mut args, "--workspace", workspace);
            for selected in paths {
                args.extend(["--path".into(), selected.into()]);
            }
            args.extend([
                "--message".into(),
                message.into(),
                "--timeout-seconds".into(),
                timeout_seconds.to_string().into(),
            ]);
            if *yes {
                args.push("--yes".into());
            }
        }
        A::Submit {
            root,
            request,
            yes,
            timeout_seconds,
        } => {
            args.push("submit".into());
            path(&mut args, "--root", root);
            path(&mut args, "--request", request);
            args.extend([
                "--timeout-seconds".into(),
                timeout_seconds.to_string().into(),
            ]);
            if *yes {
                args.push("--yes".into());
            }
        }
        A::Serve { root, once } => {
            args.push("serve".into());
            path(&mut args, "--root", root);
            if *once {
                args.push("--once".into());
            }
        }
        A::Inspect {
            workspace,
            probe_writes,
        } => {
            args.push("inspect".into());
            for root in workspace {
                path(&mut args, "--workspace", root);
            }
            if *probe_writes {
                args.push("--probe-writes".into());
            }
        }
        A::ValidateRequest {
            registration,
            request,
        } => {
            args.push("validate-request".into());
            path(&mut args, "--registration", registration);
            path(&mut args, "--request", request);
        }
    }
    let mut child = std::process::Command::new(program);
    child.args(args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        child.creation_flags(0x08000000);
    }
    let result = child.status()?;
    if !result.success() {
        bail!("global execution command failed: {result}");
    }
    Ok(())
}
