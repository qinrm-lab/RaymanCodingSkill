use crate::cli::{GlobalExecutionAction, GlobalExecutionCmd};
use crate::global_execution as global;
use anyhow::{Result, bail};
use std::{fs::File, io::Read, path::Path};

pub fn run(json: bool, command: &GlobalExecutionCmd) -> Result<()> {
    match &command.action {
        GlobalExecutionAction::RebindWorkspace {
            root,
            workspace,
            expected_sha256,
            yes,
        } => {
            #[cfg(windows)]
            println!(
                "{}",
                serde_json::to_string_pretty(&global::rebind_workspace(
                    root,
                    workspace,
                    expected_sha256.as_deref(),
                    *yes
                )?)?
            );
            #[cfg(not(windows))]
            {
                let _ = (root, workspace, expected_sha256, yes);
                bail!("physical rebinding requires Windows");
            }
        }
        GlobalExecutionAction::InstallWorktreeHook { root, yes } => {
            #[cfg(windows)]
            println!(
                "{}",
                serde_json::to_string_pretty(&global::install_worktree_hook(root, *yes)?)?
            );
            #[cfg(not(windows))]
            {
                let _ = (root, yes);
                bail!("worktree hook installation requires Windows");
            }
        }
        GlobalExecutionAction::BootstrapWorktree {
            root,
            workspace,
            yes,
        } => {
            #[cfg(windows)]
            println!(
                "{}",
                serde_json::to_string_pretty(
                    &global::Client::open(root)?.bootstrap_linked_worktree(workspace, *yes)?
                )?
            );
            #[cfg(not(windows))]
            {
                let _ = (root, workspace, yes);
                bail!("worktree bootstrap requires Windows");
            }
        }
        GlobalExecutionAction::WorktreeHook { root } => {
            #[cfg(windows)]
            {
                let outcome = (|| -> Result<serde_json::Value> {
                    let mut bytes = Vec::new();
                    std::io::stdin()
                        .take((global::MAX_REQUEST_BYTES + 1) as u64)
                        .read_to_end(&mut bytes)?;
                    if bytes.len() > global::MAX_REQUEST_BYTES {
                        bail!("hook event exceeds bound");
                    }
                    let event: serde_json::Value = global::decode_json(&bytes)?;
                    if event["hook_event_name"] != "SessionStart"
                        || event["permission_mode"] == "plan"
                    {
                        return Ok(serde_json::json!({"applicable":false}));
                    }
                    let cwd = event["cwd"]
                        .as_str()
                        .ok_or_else(|| anyhow::anyhow!("hook cwd missing"))?;
                    let mut workspace = std::path::PathBuf::from(cwd);
                    if !workspace.is_absolute() {
                        bail!("hook cwd must be absolute");
                    }
                    for _ in 0..64 {
                        let marker = workspace.join(".git");
                        if marker.is_dir() {
                            return Ok(serde_json::json!({"applicable":false}));
                        }
                        if marker.try_exists()? {
                            return global::Client::open(root)?
                                .bootstrap_linked_worktree(&workspace, true);
                        }
                        if !workspace.pop() {
                            break;
                        }
                    }
                    Ok(serde_json::json!({"applicable":false}))
                })();
                let context = match outcome {
                    Ok(value) if value["initialized"] == true => {
                        "The linked worktree has its own registered identity and state. Read this worktree's current AGENTS.md; use its current local runtime and vault paths, not cached paths from the parent checkout."
                    }
                    Ok(_) => "",
                    Err(error) => {
                        eprintln!("Global worktree bootstrap: {error:#}");
                        "Automatic linked-worktree setup did not complete. Preserve the current files and do not use cached parent-checkout state paths. Inspect the hook diagnostic and global worktree bootstrap preview before claiming commit or checkpoint readiness."
                    }
                };
                println!(
                    "{}",
                    serde_json::json!({"hookSpecificOutput":{"hookEventName":"SessionStart","additionalContext":context}})
                );
            }
            #[cfg(not(windows))]
            {
                let _ = root;
                println!("{{}}");
            }
        }
        GlobalExecutionAction::AuthorizeWorktrees {
            root,
            workspace,
            allowed_root,
            formal_state,
            yes,
        } => {
            #[cfg(windows)]
            println!(
                "{}",
                serde_json::to_string_pretty(&global::authorize_worktrees(
                    root,
                    workspace,
                    allowed_root,
                    *formal_state,
                    *yes
                )?)?
            );
            #[cfg(not(windows))]
            {
                let _ = (root, workspace, allowed_root, formal_state, yes);
                bail!("worktree authorization requires Windows");
            }
        }
        GlobalExecutionAction::EnrollLinkedWorktree {
            root,
            workspace,
            yes,
            timeout_seconds,
        } => {
            #[cfg(windows)]
            {
                let client = global::Client::open(root)?;
                let (request, anchor) = client
                    .prepare_linked_worktree_request(workspace, chrono::Utc::now().timestamp())?;
                if *yes {
                    submit_and_wait(&client, &request, &anchor.registration, *timeout_seconds)?;
                } else {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(
                            &serde_json::json!({"preview":true,"executed":false,"request":request,"state_initialization_required":true})
                        )?
                    );
                }
            }
            #[cfg(not(windows))]
            {
                let _ = (root, workspace, yes, timeout_seconds);
                bail!("worktree enrollment requires Windows");
            }
        }
        GlobalExecutionAction::PublishSkill {
            root,
            yes,
            expected_sha256,
        } => {
            if !yes {
                bail!("global skill publication requires explicit --yes");
            }
            #[cfg(windows)]
            println!(
                "{}",
                serde_json::to_string_pretty(&global::publish_global_skill_checked(
                    root,
                    expected_sha256.as_deref()
                )?)?
            );
            #[cfg(not(windows))]
            {
                let _ = (root, expected_sha256);
                bail!("global skill publication requires Windows");
            }
        }
        GlobalExecutionAction::RegisterInstallAdapter {
            root,
            workspace,
            specification,
            yes,
        } => {
            if !yes {
                bail!("adapter registration requires explicit --yes");
            }
            #[cfg(windows)]
            println!(
                "{}",
                serde_json::to_string_pretty(&global::register_install_adapter(
                    root,
                    workspace,
                    specification
                )?)?
            );
            #[cfg(not(windows))]
            {
                let _ = (root, workspace, specification);
                bail!("installation adapters require Windows");
            }
        }
        GlobalExecutionAction::Install {
            root,
            workspace,
            adapter_id,
            version,
            sources,
            yes,
            timeout_seconds,
        } => {
            #[cfg(windows)]
            {
                let paths = global::decode_install_sources(&bounded_read(sources)?)?;
                let client = global::Client::open(root)?;
                let request = client.prepare_install_request(
                    workspace,
                    adapter_id,
                    version,
                    &paths,
                    chrono::Utc::now().timestamp(),
                )?;
                if !yes {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(
                            &serde_json::json!({"preview":true,"executed":false,"request":request})
                        )?
                    );
                } else {
                    let enrollment = client.enrollment(workspace)?;
                    submit_and_wait(
                        &client,
                        &request,
                        &enrollment.registration,
                        *timeout_seconds,
                    )?;
                }
            }
            #[cfg(not(windows))]
            {
                let _ = (
                    root,
                    workspace,
                    adapter_id,
                    version,
                    sources,
                    yes,
                    timeout_seconds,
                );
                bail!("installation adapters require Windows");
            }
        }
        GlobalExecutionAction::RollbackCheckpointMigration {
            root,
            workspace,
            transaction_id,
            yes,
        } => {
            if !yes {
                bail!("checkpoint rollback requires --yes");
            }
            #[cfg(windows)]
            {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&global::rollback_checkpoint_migration(
                        root,
                        workspace,
                        transaction_id
                    )?)?
                );
            }
            #[cfg(not(windows))]
            {
                let _ = (root, workspace, transaction_id);
                bail!("global execution currently requires Windows");
            }
        }

        GlobalExecutionAction::PublishCheckpointMigration {
            root,
            workspace,
            transaction_id,
            yes,
        } => {
            if !yes {
                bail!("checkpoint migration publication requires --yes");
            }
            #[cfg(windows)]
            {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&global::publish_checkpoint_migration(
                        root,
                        workspace,
                        transaction_id
                    )?)?
                );
            }
            #[cfg(not(windows))]
            {
                let _ = (root, workspace, transaction_id);
                bail!("global execution currently requires Windows");
            }
        }

        GlobalExecutionAction::Status { root } => {
            #[cfg(windows)]
            {
                let report = global::Client::open(root)?.status()?;
                if json {
                    println!("{}", serde_json::to_string_pretty(&report)?);
                } else {
                    println!(
                        "安装身份：{}",
                        report["owner_sid"].as_str().unwrap_or("未知")
                    );
                    if let Some(problems) = report["queue"]["problems"].as_array()
                        && !problems.is_empty()
                    {
                        println!(
                            "有 {} 个请求未正常处理；使用 --format json status 查看具体原因。",
                            problems.len()
                        );
                    }
                    if report["heartbeat_fresh"] != true {
                        println!("后台心跳缺失或已过期；不能确认仍在运行。");
                    } else if report["service_healthy"] != true {
                        println!(
                            "后台仍有心跳，但服务报告错误：{}",
                            report["heartbeat"]["error"]
                        );
                    } else {
                        let activity = &report["heartbeat"]["activity"];
                        if let Some(id) = activity["request_id"].as_str() {
                            let stage = match activity["stage"].as_str().unwrap_or_default() {
                                "hash_source" => "核对源码",
                                "record_intent" => "记录事务",
                                "prepare_commit" => "准备提交",
                                "publish_commit" => "写入提交",
                                "record_result" => "保存回执",
                                "state_storage" => "写入状态",
                                _ => "校验请求",
                            };
                            let elapsed = chrono::Utc::now()
                                .timestamp()
                                .saturating_sub(activity["started_at"].as_i64().unwrap_or(0));
                            println!(
                                "后台心跳正常；当前阶段：{stage}，本请求已运行 {elapsed} 秒。请求 ID：{id}"
                            );
                        } else {
                            println!("后台心跳正常，正在等待请求。");
                        }
                        println!(
                            "本次启动已处理 {} 个请求；心跳正常不代表任务已经完成。",
                            activity["completed_requests"]
                        );
                    }
                }
            }
            #[cfg(not(windows))]
            {
                let _ = root;
                bail!("global execution currently requires Windows");
            }
        }
        GlobalExecutionAction::PrepareCheckpointMigration {
            root,
            workspace,
            runtime_sha256,
            yes,
        } => {
            if !yes {
                bail!("checkpoint migration preparation requires --yes");
            }
            #[cfg(windows)]
            {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&global::prepare_checkpoint_migration(
                        root,
                        workspace,
                        runtime_sha256
                    )?)?
                );
            }
            #[cfg(not(windows))]
            {
                let _ = (root, workspace, runtime_sha256);
                bail!("global execution currently requires Windows");
            }
        }
        GlobalExecutionAction::CheckpointCopy {
            root,
            workspace,
            destination,
        } => {
            #[cfg(windows)]
            {
                let client = global::Client::open(root)?;
                let enrollment = client.enrollment(workspace)?;
                client.copy_checkpoint(&enrollment, destination)?;
                println!(
                    "{}",
                    serde_json::json!({"copied":true,"destination":destination})
                );
            }
            #[cfg(not(windows))]
            {
                let _ = (root, workspace, destination);
                bail!("global execution currently requires Windows");
            }
        }
        GlobalExecutionAction::CheckpointApply {
            root,
            workspace,
            before,
            after,
            lease_id,
            transaction_id,
        } => {
            #[cfg(windows)]
            {
                let client = global::Client::open(root)?;
                let enrollment = client.enrollment(workspace)?;
                let result = client.apply_checkpoint_copies(
                    &enrollment,
                    before,
                    after,
                    lease_id,
                    transaction_id,
                )?;
                println!("{}", serde_json::to_string(&result)?);
            }
            #[cfg(not(windows))]
            {
                let _ = (root, workspace, before, after, lease_id, transaction_id);
                bail!("global execution currently requires Windows");
            }
        }
        GlobalExecutionAction::AppState {
            no_wait,
            root,
            workspace,
            action,
            object,
            lease_id,
            expected_sha256,
        } => {
            #[cfg(windows)]
            {
                use crate::cli::StateAction;
                let client = global::Client::open(root)?;
                let enrollment = client.enrollment(workspace)?;
                let key = || -> Result<global::StateObject> {
                    match object.as_deref() {
                        Some("goals-store") => Ok(global::StateObject::GoalsStore),
                        Some("pending") => Ok(global::StateObject::Pending),
                        Some("context") => Ok(global::StateObject::ContextIndex),
                        Some("checkpoints") => Ok(global::StateObject::Checkpoints),
                        Some(id) if id.starts_with("goal_") => {
                            Ok(global::StateObject::Goal { id: id.into() })
                        }
                        _ => bail!("unknown application state object"),
                    }
                };
                let lease = || {
                    lease_id
                        .as_deref()
                        .ok_or_else(|| anyhow::anyhow!("state lease ID is required"))
                };
                if *no_wait {
                    if !matches!(action, StateAction::Release) {
                        bail!("only lease release may be queued without waiting");
                    }
                    let id = client.enqueue_storage(
                        &enrollment,
                        global::StorageAction::Release {
                            lease_id: lease()?.into(),
                        },
                    )?;
                    println!("{}", serde_json::json!({"queued":true,"request_id":id}));
                    return Ok(());
                }
                let reply = match action {
                    StateAction::Acquire => client.storage_call(
                        &enrollment,
                        global::StorageAction::Acquire { object: key()? },
                    )?,
                    StateAction::Release => client.storage_call(
                        &enrollment,
                        global::StorageAction::Release {
                            lease_id: lease()?.into(),
                        },
                    )?,
                    StateAction::Renew => client.storage_call(
                        &enrollment,
                        global::StorageAction::Renew {
                            lease_id: lease()?.into(),
                        },
                    )?,
                    StateAction::Write => {
                        let mut bytes = Vec::new();
                        std::io::stdin()
                            .take(64 * 1024 * 1024 + 1)
                            .read_to_end(&mut bytes)?;
                        client.write_application_state(
                            &enrollment,
                            key()?,
                            lease()?,
                            expected_sha256.clone(),
                            &bytes,
                        )?
                    }
                };
                println!("{}", serde_json::to_string_pretty(&reply)?);
            }
            #[cfg(not(windows))]
            {
                let _ = (
                    root,
                    workspace,
                    action,
                    object,
                    lease_id,
                    expected_sha256,
                    no_wait,
                );
                bail!("global execution currently requires Windows");
            }
        }
        GlobalExecutionAction::Initialize {
            root,
            source_workspace,
            yes,
        } => {
            if !yes {
                bail!("initialization requires explicit --yes");
            }
            #[cfg(windows)]
            {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&global::initialize(root, source_workspace)?)?
                );
            }
            #[cfg(not(windows))]
            {
                let _ = (root, source_workspace);
                bail!("global execution currently requires Windows");
            }
        }
        GlobalExecutionAction::Enroll {
            root,
            workspace,
            git,
            author_name,
            author_email,
            formal_state,
            preflight,
            yes,
        } => {
            if *yes == *preflight {
                bail!("enrollment requires exactly one of --yes or --preflight");
            }
            #[cfg(windows)]
            {
                let commit = if let Some(git) = git {
                    Some((
                        git.as_path(),
                        author_name
                            .as_deref()
                            .ok_or_else(|| anyhow::anyhow!("commit author name is required"))?,
                        author_email
                            .as_deref()
                            .ok_or_else(|| anyhow::anyhow!("commit author email is required"))?,
                    ))
                } else {
                    None
                };
                println!(
                    "{}",
                    serde_json::to_string_pretty(&global::enroll(
                        root,
                        workspace,
                        commit,
                        *formal_state,
                        *yes
                    )?)?
                );
            }
            #[cfg(not(windows))]
            {
                let _ = (
                    root,
                    workspace,
                    git,
                    author_name,
                    author_email,
                    formal_state,
                    preflight,
                );
                bail!("global execution currently requires Windows");
            }
        }
        GlobalExecutionAction::ActivateRaymanState {
            root,
            workspace,
            yes,
        } => {
            if !yes {
                bail!("Rayman state activation requires --yes");
            }
            #[cfg(windows)]
            {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&global::enable_application_bridge(
                        root, workspace
                    )?)?
                );
            }
            #[cfg(not(windows))]
            {
                let _ = (root, workspace);
                bail!("global execution currently requires Windows");
            }
        }
        GlobalExecutionAction::Result { root, request_id } => {
            #[cfg(windows)]
            {
                let result = global::Client::open(root)?.query(request_id)?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(
                        &serde_json::json!({"completed":result.is_some(),"result":result})
                    )?
                );
            }
            #[cfg(not(windows))]
            {
                let _ = (root, request_id);
                bail!("global execution currently requires Windows");
            }
        }
        GlobalExecutionAction::RecoverCommit {
            root,
            workspace,
            original_request_id,
            candidate_sha256,
            yes,
            timeout_seconds,
        } => {
            #[cfg(windows)]
            {
                if *yes && candidate_sha256.is_none() {
                    bail!("commit recovery execution requires the reviewed --candidate-sha256");
                }
                let client = global::Client::open(root)?;
                let request = client.prepare_commit_recovery_request(
                    workspace,
                    original_request_id,
                    candidate_sha256.as_deref(),
                    chrono::Utc::now().timestamp(),
                )?;
                if !yes {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(
                            &serde_json::json!({"preview":true,"executed":false,"request":request})
                        )?
                    );
                } else {
                    let enrollment = client.enrollment(workspace)?;
                    submit_and_wait(
                        &client,
                        &request,
                        &enrollment.registration,
                        *timeout_seconds,
                    )?;
                }
            }
            #[cfg(not(windows))]
            {
                let _ = (
                    root,
                    workspace,
                    original_request_id,
                    candidate_sha256,
                    yes,
                    timeout_seconds,
                );
                bail!("global execution currently requires Windows");
            }
        }
        GlobalExecutionAction::Commit {
            root,
            workspace,
            paths,
            message,
            yes,
            timeout_seconds,
        } => {
            #[cfg(windows)]
            {
                let client = global::Client::open(root)?;
                let request = client.prepare_commit_request(
                    workspace,
                    paths,
                    message,
                    chrono::Utc::now().timestamp(),
                )?;
                if !yes {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(
                            &serde_json::json!({"preview":true,"executed":false,"request":request})
                        )?
                    );
                    return Ok(());
                }
                let enrollment = client.enrollment(workspace)?;
                submit_and_wait(
                    &client,
                    &request,
                    &enrollment.registration,
                    *timeout_seconds,
                )?;
            }
            #[cfg(not(windows))]
            {
                let _ = (root, workspace, paths, message, yes, timeout_seconds);
                bail!("global execution currently requires Windows");
            }
        }
        GlobalExecutionAction::Submit {
            root,
            request,
            yes,
            timeout_seconds,
        } => {
            #[cfg(windows)]
            {
                if !yes {
                    bail!("global submission requires explicit --yes");
                }
                let request = global::decode_request(&bounded_read(request)?)?;
                let client = global::Client::open(root)?;
                // A submission cannot supply a replacement enrollment.
                let path = root.join(format!("project-{}.json", request.worktree_id));
                if request.worktree_id.len() != 32
                    || !request.worktree_id.bytes().all(|b| b.is_ascii_hexdigit())
                {
                    bail!("invalid worktree identifier");
                }
                let enrollment: global::Enrollment = serde_json::from_slice(&bounded_read(&path)?)?;
                let registered = client.enrollment(&enrollment.workspace)?;
                submit_and_wait(
                    &client,
                    &request,
                    &registered.registration,
                    *timeout_seconds,
                )?;
            }
            #[cfg(not(windows))]
            {
                let _ = (root, request, yes, timeout_seconds);
                bail!("global execution currently requires Windows");
            }
        }
        GlobalExecutionAction::Serve { root, once } => {
            #[cfg(windows)]
            {
                global::serve(root, *once)?;
            }
            #[cfg(not(windows))]
            {
                let _ = (root, once);
                bail!("global execution worker currently requires Windows");
            }
        }
        GlobalExecutionAction::Inspect {
            workspace,
            probe_writes,
        } => {
            let report = global::preflight(workspace, *probe_writes)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!(
                    "执行身份：{}",
                    report
                        .execution_context
                        .principal_account
                        .as_deref()
                        .unwrap_or("unknown")
                );
                for project in &report.projects {
                    println!(
                        "{}：state_probe={} state_writable={} commit_ready={} install_ready={} blockers={}",
                        project.workspace,
                        project.formal_state.probed,
                        project.formal_state.writable,
                        project.commit_ready,
                        project.installation_ready,
                        project.blockers.join(",")
                    );
                }
            }
        }
        GlobalExecutionAction::ValidateRequest {
            registration,
            request,
        } => {
            let registration = global::decode_registration(&bounded_read(registration)?)?;
            let request = global::decode_request(&bounded_read(request)?)?;
            global::validate_request(&request, &registration, chrono::Utc::now().timestamp())?;
            let result = serde_json::json!({"schema":"rayman.global-request-check.v1","valid":true,"executed":false,"registration_attested":false,"request_sha256":request.digest()?});
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
    }
    Ok(())
}

#[cfg(windows)]
fn submit_and_wait(
    client: &global::Client,
    request: &global::Request,
    registration: &global::Registration,
    timeout: u64,
) -> Result<()> {
    if timeout == 0 || timeout > 600 {
        bail!("timeout must be between 1 and 600 seconds");
    }
    client.submit(request, registration, chrono::Utc::now().timestamp())?;
    eprintln!(
        "已提交全局请求 {}，等待受保护后台处理。",
        request.request_id
    );
    let result = client.wait(
        request,
        std::time::Duration::from_secs(timeout),
        |elapsed| {
            eprintln!(
                "请求 {} 尚未返回结果，已等待 {} 秒。",
                request.request_id,
                elapsed.as_secs()
            )
        },
    )?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    if matches!(
        result.result,
        global::RecordedResult::EffectFailed { .. } | global::RecordedResult::RecoveryRequired
    ) {
        bail!("global operation did not complete successfully; inspect its retained result");
    }
    Ok(())
}

fn bounded_read(path: &Path) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    File::open(path)?
        .take((global::MAX_REQUEST_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > global::MAX_REQUEST_BYTES {
        bail!("global protocol file exceeds size limit");
    }
    Ok(bytes)
}
