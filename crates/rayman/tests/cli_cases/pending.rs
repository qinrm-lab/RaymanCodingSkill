use super::*;

#[test]
fn pending_items_roundtrip_and_block_check() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "src/a.rs", "fn a() {}");
    run(root, &["context", "refresh"]);

    // 干净工作区（无 pending、上下文 ready）→ check READY，退出 0。
    let ready = run(root, &["check"]);
    assert_eq!(
        ready.status, 0,
        "stdout={} stderr={}",
        ready.stdout, ready.stderr
    );

    // 加一个待完成项 → check BLOCKED，退出 1。
    run(
        root,
        &["goal", "pending", "add", "finish gate", "-m", "wire CI"],
    );
    let blocked = run(root, &["check"]);
    assert_eq!(blocked.status, 1);
    assert!(blocked.stdout.contains("BLOCKED"));

    // 解决后恢复 READY。
    let items = run_json(root, &["goal", "pending", "list"]);
    let pending_id = items[0]["id"].as_str().unwrap().to_string();
    run(root, &["goal", "pending", "resolve", &pending_id]);
    assert_eq!(run(root, &["check"]).status, 0);
}

#[test]
fn frontier_requires_a_complete_solution_package_before_asking_user() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "README.md", "workspace");
    run_json(root, &["context", "refresh"]);
    let started = run_json(root, &["goal", "start", "owner", "--must", "finish"]);
    let id = started["id"].as_str().unwrap();
    let agent = run_json(
        root,
        &[
            "goal",
            "pending",
            "add",
            "local repair",
            "-m",
            "agent can still fix it",
            "--goal",
            id,
        ],
    );
    let frontier = run_json(root, &["goal", "frontier", id]);
    assert_eq!(frontier["decision"], "continue");
    assert_eq!(frontier["ask_user_allowed"], false);
    assert_eq!(frontier["execution"], "continue_foreground");
    assert_eq!(frontier["consultation"], "none");
    assert_eq!(
        run(root, &["goal", "close", id, "--status", "blocked"]).status,
        1
    );
    run(
        root,
        &["goal", "pending", "resolve", agent["id"].as_str().unwrap()],
    );

    let incomplete = run(
        root,
        &[
            "goal",
            "pending",
            "add",
            "choice",
            "-m",
            "business choice",
            "--goal",
            id,
            "--owner",
            "human",
            "--kind",
            "human_input",
            "--capability-key",
            "owner/choice",
            "--boundary-class",
            "owner_decision",
        ],
    );
    assert_eq!(incomplete.status, 1);
    let choice = run_json(
        root,
        &[
            "goal",
            "pending",
            "add",
            "choice",
            "-m",
            "business choice",
            "--goal",
            id,
            "--owner",
            "human",
            "--kind",
            "human_input",
            "--attempt",
            "tested both variants",
            "--evidence-path",
            "reports/options.md",
            "--minimum-input",
            "choose A or B",
            "--recommended",
            "choose A",
            "--alternative",
            "choose B",
            "--risk",
            "A is safer; B is faster",
            "--resume-command",
            "rayman prepare --goal owner",
            "--auto-resume-condition",
            "choice recorded",
            "--capability-key",
            "owner/choice",
            "--boundary-class",
            "owner_decision",
        ],
    );
    let frontier = run_json(root, &["goal", "frontier", id]);
    assert_eq!(frontier["decision"], "ask_user");
    assert_eq!(frontier["ask_user_allowed"], true);
    assert_eq!(frontier["execution"], "paused_for_user");
    assert_eq!(frontier["consultation"], "ready");
    let rendered = run_json(root, &["goal", "pending", "render", "--goal", id]);
    assert!(
        rendered["text"]
            .as_str()
            .is_some_and(|text| text.contains(choice["id"].as_str().unwrap())),
        "{rendered}"
    );
    let retired = run(
        root,
        &[
            "goal",
            "pending",
            "present",
            choice["id"].as_str().unwrap(),
            "--goal",
            id,
            "--package-sha256",
            choice["package_sha256"].as_str().unwrap(),
            "--channel",
            "codex",
        ],
    );
    assert_eq!(retired.status, 1);
    assert!(retired.stderr.contains("已退役"), "{}", retired.stderr);
    let frontier = run_json(root, &["goal", "frontier", id]);
    assert_eq!(frontier["decision"], "ask_user");
    assert_eq!(frontier["ask_user_allowed"], true);
    assert_eq!(frontier["consultation"], "ready");
    assert_eq!(
        run(root, &["goal", "close", id, "--status", "blocked"]).status,
        0
    );
}

#[test]
fn frontier_requires_complete_background_authority_before_rendered_parallel_work() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "README.md", "workspace");
    run_json(root, &["context", "refresh"]);
    let started = run_json(root, &["goal", "start", "mixed", "--must", "finish"]);
    let id = started["id"].as_str().unwrap();
    run_json(
        root,
        &[
            "goal",
            "pending",
            "add",
            "local repair",
            "-m",
            "safe work",
            "--goal",
            id,
        ],
    );

    let partial = run(
        root,
        &[
            "goal",
            "pending",
            "add",
            "urgent choice",
            "-m",
            "owner input",
            "--goal",
            id,
            "--owner",
            "human",
            "--kind",
            "human_input",
            "--attempt",
            "tested both",
            "--evidence-path",
            "reports/options.md",
            "--minimum-input",
            "choose A or B",
            "--recommended",
            "choose A",
            "--alternative",
            "choose B",
            "--risk",
            "tradeoff",
            "--resume-command",
            "rayman prepare --goal mixed",
            "--auto-resume-condition",
            "choice recorded",
            "--consultation-timing",
            "immediate",
            "--background-mechanism",
            "worktree task",
            "--background-authority-evidence",
            "user instruction codex://threads/test",
            "--capability-key",
            "mixed/urgent-choice",
            "--boundary-class",
            "owner_decision",
        ],
    );
    assert_eq!(partial.status, 1);

    let immediate = run_json(
        root,
        &[
            "goal",
            "pending",
            "add",
            "urgent choice",
            "-m",
            "owner input",
            "--goal",
            id,
            "--owner",
            "human",
            "--kind",
            "human_input",
            "--attempt",
            "tested both",
            "--evidence-path",
            "reports/options.md",
            "--minimum-input",
            "choose A or B",
            "--recommended",
            "choose A",
            "--alternative",
            "choose B",
            "--risk",
            "tradeoff",
            "--resume-command",
            "rayman prepare --goal mixed",
            "--auto-resume-condition",
            "choice recorded",
            "--consultation-timing",
            "immediate",
            "--capability-key",
            "mixed/urgent-choice",
            "--boundary-class",
            "owner_decision",
        ],
    );
    let frontier = run_json(root, &["goal", "frontier", id]);
    assert_eq!(frontier["decision"], "ask_user");
    assert_eq!(frontier["execution"], "paused_for_user");
    assert_eq!(frontier["consultation"], "ready");
    assert_eq!(frontier["background_execution_allowed"], false);
    let rendered = run_json(root, &["goal", "pending", "render", "--current"]);
    assert!(
        rendered["text"]
            .as_str()
            .is_some_and(|text| text.contains(immediate["id"].as_str().unwrap())),
        "{rendered}"
    );
    let frontier = run_json(root, &["goal", "frontier", id]);
    assert_eq!(frontier["decision"], "ask_user");
    assert_eq!(frontier["consultation"], "ready");
    run(
        root,
        &[
            "goal",
            "pending",
            "resolve",
            immediate["id"].as_str().unwrap(),
        ],
    );

    let background = run_json(
        root,
        &[
            "goal",
            "pending",
            "add",
            "urgent choice",
            "-m",
            "owner input",
            "--goal",
            id,
            "--owner",
            "human",
            "--kind",
            "human_input",
            "--attempt",
            "tested both",
            "--evidence-path",
            "reports/options.md",
            "--minimum-input",
            "choose A or B",
            "--recommended",
            "choose A",
            "--alternative",
            "choose B",
            "--risk",
            "tradeoff",
            "--resume-command",
            "rayman prepare --goal mixed",
            "--auto-resume-condition",
            "choice recorded",
            "--consultation-timing",
            "immediate",
            "--background-mechanism",
            "isolated worktree task task_123",
            "--background-authority-evidence",
            "user instruction codex://threads/test",
            "--background-isolation-evidence",
            "isolated worktree task task_123",
            "--capability-key",
            "mixed/urgent-choice",
            "--boundary-class",
            "owner_decision",
        ],
    );
    let frontier = run_json(root, &["goal", "frontier", id]);
    assert_eq!(frontier["decision"], "ask_user");
    assert_eq!(frontier["execution"], "continue_background");
    assert_eq!(frontier["consultation"], "ready");
    assert_eq!(frontier["background_execution_allowed"], true);
    let rendered = run_json(root, &["goal", "pending", "render", "--current"]);
    assert!(
        rendered["text"]
            .as_str()
            .is_some_and(|text| text.contains(background["id"].as_str().unwrap())),
        "{rendered}"
    );
    let frontier = run_json(root, &["goal", "frontier", id]);
    assert_eq!(frontier["decision"], "ask_user");
    assert_eq!(frontier["execution"], "continue_background");
    assert_eq!(frontier["consultation"], "ready");
    assert_eq!(frontier["ask_user_allowed"], true);
}

#[test]
fn pending_render_current_matches_the_workspace_aggregate() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "README.md", "workspace");
    run_json(root, &["context", "refresh"]);
    let goal_a = run_json(root, &["goal", "start", "aggregate A", "--must", "choose"]);
    let goal_b = run_json(root, &["goal", "start", "aggregate B", "--must", "choose"]);
    let id_a = goal_a["id"].as_str().unwrap();
    let id_b = goal_b["id"].as_str().unwrap();
    let item_a = add_complete_human_pending(root, id_a, "owner/shared", "choice A");
    let item_b = add_complete_human_pending(root, id_b, "owner/shared", "choice B");

    let aggregate = run_json(root, &["goal", "pending", "render", "--current"]);
    let partial = run_json(root, &["goal", "pending", "render", "--goal", id_a]);
    let aggregate_text = aggregate["text"].as_str().unwrap();
    assert!(aggregate_text.contains("rayman.human-boundary-aggregate.v1"));
    assert!(aggregate_text.contains("\"scope\": \"current_response_only\""));
    assert!(!aggregate_text.contains("rayman.codex-stop-candidate"));
    assert!(aggregate_text.contains(item_a["id"].as_str().unwrap()));
    assert!(aggregate_text.contains(item_b["id"].as_str().unwrap()));
    assert_eq!(aggregate["goal_ids"].as_array().unwrap().len(), 2);
    assert_ne!(aggregate["render_sha256"], partial["render_sha256"]);
    assert_eq!(run(root, &["goal", "pending", "render"]).status, 1);
    assert_ne!(
        run(
            root,
            &["goal", "pending", "render", "--goal", id_a, "--current"],
        )
        .status,
        0
    );
}

#[test]
fn pending_render_text_is_protocol_exact_under_the_english_locale() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "README.md", "workspace");
    run_json(root, &["context", "refresh"]);
    let goal = run_json(
        root,
        &["goal", "start", "exact aggregate", "--must", "choose"],
    );
    let id = goal["id"].as_str().unwrap();
    add_complete_human_pending_with_title(root, id, "owner/exact", "秒", "choose safely");

    let expected = run_json(root, &["goal", "pending", "render", "--current"]);
    let expected = expected["text"].as_str().unwrap();
    assert!(expected.contains("秒"), "{expected}");

    let rendered = run(
        root,
        &["--language", "en", "goal", "pending", "render", "--current"],
    );
    assert_eq!(rendered.status, 0, "{}", rendered.stderr);
    assert_eq!(
        rendered.stdout.trim_end_matches(&['\r', '\n'][..]),
        expected,
        "text-mode output must remain byte-for-byte compatible with the client-neutral aggregate"
    );
}
