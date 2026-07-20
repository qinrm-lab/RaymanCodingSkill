use super::*;

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

#[test]
fn save_with_dir_inside_workspace_excludes_prior_snapshots() {
    let ws = tempfile::tempdir().unwrap();
    let root = ws.path();
    write(&root.join("src/main.rs"), "fn main() {}");
    let ckpt_dir = root.join(".ckpts");

    let first = save(root, Some(&ckpt_dir), DEFAULT_KEEP).unwrap();
    let second = save(root, Some(&ckpt_dir), DEFAULT_KEEP).unwrap();

    // 快照目录在工作区内时，旧快照绝不能被递归拷进新快照（体积会几何级膨胀）。
    let tree = second.path.join(TREE_SUBDIR);
    assert!(
        !tree.join(".ckpts").exists(),
        "旧快照被拷进了新快照: {}",
        tree.display()
    );
    assert_eq!(second.file_count, first.file_count);
}

#[test]
fn save_with_keep_zero_still_retains_latest_snapshot() {
    let ws = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    let root = ws.path();
    write(&root.join("src/main.rs"), "fn main() {}");

    let outcome = save(root, Some(store.path()), 0).unwrap();
    assert!(
        outcome.path.join(MANIFEST_NAME).exists(),
        "keep=0 不得把刚保存的快照也删掉"
    );
    assert_eq!(
        verify_snapshot(&outcome.path).unwrap().status,
        SnapshotStatus::Complete
    );
}

#[test]
fn save_captures_only_v2_state_whitelist_and_restores() {
    let ws = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    let root = ws.path();
    let dir = Some(store.path());

    write(&root.join("src/main.rs"), "fn main() {}");
    write(&root.join(".gitignore"), "ignored.txt\n");
    write(&root.join("ignored.txt"), "secret");
    write(&root.join("node_modules/pkg/index.js"), "x");
    write(
        &root.join(".RaymanCodingSkill/goals/g1.json"),
        "{\"id\":\"g1\"}",
    );
    write(
        &root.join(".RaymanCodingSkill/pending.json"),
        "{\"items\":[]}",
    );
    write(
        &root.join(".RaymanCodingSkill/context/index.json"),
        "{\"version\":2}",
    );
    write(
        &root.join(".RaymanCodingSkill/context/project_map.json"),
        "{\"version\":2}",
    );
    write(
        &root.join(".RaymanCodingSkill/autosave.json"),
        "{\"active\":false}",
    );
    write(
        &root.join(".RaymanCodingSkill/tmp/scratch.txt"),
        "transient",
    );
    write(
        &root.join(".RaymanCodingSkill/regression/history.jsonl"),
        "retired",
    );

    let outcome = save(root, dir, DEFAULT_KEEP).unwrap();
    let tree = outcome.path.join(TREE_SUBDIR);
    assert!(tree.join("src/main.rs").exists());
    assert!(tree.join(".RaymanCodingSkill/goals/g1.json").exists());
    assert!(tree.join(".RaymanCodingSkill/pending.json").exists());
    assert!(tree.join(".RaymanCodingSkill/context/index.json").exists());
    assert!(
        tree.join(".RaymanCodingSkill/context/project_map.json")
            .exists()
    );
    assert!(tree.join(".RaymanCodingSkill/autosave.json").exists());
    // gitignore 命中、vendor 目录、易变 tmp 和已退役状态都不应进快照。
    assert!(!tree.join("ignored.txt").exists());
    assert!(!tree.join("node_modules/pkg/index.js").exists());
    assert!(!tree.join(".RaymanCodingSkill/tmp/scratch.txt").exists());
    assert!(
        !tree
            .join(".RaymanCodingSkill/regression/history.jsonl")
            .exists()
    );

    fs::remove_file(root.join("src/main.rs")).unwrap();
    let restored = restore(root, dir, None).unwrap();
    assert!(restored.restored >= 2);
    assert_eq!(
        fs::read_to_string(root.join("src/main.rs")).unwrap(),
        "fn main() {}"
    );
}

#[test]
fn partial_save_is_not_latest_and_does_not_prune_last_complete_snapshot() {
    let ws = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    let root = ws.path();
    write(&root.join("a.txt"), "a");
    let complete = save(root, Some(store.path()), 1).unwrap();

    // goals 应为目录；用同名普通文件模拟状态遍历失败，不依赖权限模型。
    write(&root.join(".RaymanCodingSkill/goals"), "not a directory");
    assert!(save(root, Some(store.path()), 1).is_err());

    let checkpoints = list(root, Some(store.path())).unwrap();
    assert!(
        checkpoints
            .iter()
            .any(|checkpoint| checkpoint.status == SnapshotStatus::Partial)
    );
    let latest_complete = latest(root, Some(store.path())).unwrap().unwrap();
    assert_eq!(latest_complete.id, complete.id);
    assert!(
        latest_complete.path.exists(),
        "last complete snapshot must survive"
    );
}

#[test]
fn verify_rejects_tampering_before_restore_writes_any_file() {
    let ws = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    let root = ws.path();
    write(&root.join("a.txt"), "a");
    write(&root.join("b.txt"), "b");
    let saved = save(root, Some(store.path()), DEFAULT_KEEP).unwrap();
    fs::write(saved.path.join(TREE_SUBDIR).join("b.txt"), "tampered").unwrap();

    fs::remove_file(root.join("a.txt")).unwrap();
    fs::remove_file(root.join("b.txt")).unwrap();
    assert!(verify_snapshot(&saved.path).is_err());
    assert!(restore(root, Some(store.path()), Some(&saved.id)).is_err());
    assert!(
        !root.join("a.txt").exists(),
        "restore must verify before its first write"
    );
    assert!(!root.join("b.txt").exists());
}

#[test]
fn prune_only_removes_verified_complete_snapshots() {
    let ws = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    let root = ws.path();
    write(&root.join("a.txt"), "a");
    let first = save(root, Some(store.path()), 10).unwrap();
    write(&root.join("a.txt"), "b");
    let second = save(root, Some(store.path()), 10).unwrap();
    write(&root.join("a.txt"), "c");
    let third = save(root, Some(store.path()), 10).unwrap();

    let ws_dir = workspace_dir(root, Some(store.path())).unwrap();
    assert_eq!(prune(&ws_dir, 2).unwrap(), 1);
    assert!(!first.path.exists());
    assert!(second.path.exists());
    assert!(third.path.exists());
}

#[test]
fn prune_never_removes_uncommitted_staging() {
    let store = tempfile::tempdir().unwrap();
    let staging = store.path().join(".staging-live");
    fs::create_dir(&staging).unwrap();
    write(&staging.join("in-progress.txt"), "copying");

    assert_eq!(prune(store.path(), 1).unwrap(), 0);
    assert!(staging.join("in-progress.txt").exists());
}

#[test]
fn concurrent_saves_are_serialized_and_both_remain_verifiable() {
    use std::sync::{Arc, Barrier};

    let ws = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    write(&ws.path().join("src/main.rs"), "fn main() {}\n");
    let root = ws.path().to_path_buf();
    let checkpoint_root = store.path().to_path_buf();
    let barrier = Arc::new(Barrier::new(3));
    let handles: Vec<_> = (0..2)
        .map(|_| {
            let root = root.clone();
            let checkpoint_root = checkpoint_root.clone();
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                save(&root, Some(&checkpoint_root), 3)
            })
        })
        .collect();
    barrier.wait();
    let outcomes: Vec<_> = handles
        .into_iter()
        .map(|handle| handle.join().unwrap().unwrap())
        .collect();

    assert_eq!(outcomes.len(), 2);
    for outcome in outcomes {
        assert_eq!(
            verify_snapshot(&outcome.path).unwrap().status,
            SnapshotStatus::Complete
        );
    }
}

#[test]
fn restore_is_binary_safe_and_idempotent() {
    let ws = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    let root = ws.path();
    let bytes = [0_u8, 1, 2, 3, 254, 255];
    fs::write(root.join("payload.bin"), bytes).unwrap();
    let saved = save(root, Some(store.path()), DEFAULT_KEEP).unwrap();
    fs::write(root.join("payload.bin"), b"broken").unwrap();

    restore(root, Some(store.path()), Some(&saved.id)).unwrap();
    restore(root, Some(store.path()), Some(&saved.id)).unwrap();
    assert_eq!(fs::read(root.join("payload.bin")).unwrap(), bytes);
}

#[test]
fn restore_rolls_back_first_file_when_second_publish_fails() {
    let ws = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    let root = ws.path();
    write(&root.join("a.txt"), "checkpoint-a");
    write(&root.join("b.txt"), "checkpoint-b");
    let saved = save(root, Some(store.path()), DEFAULT_KEEP).unwrap();
    write(&root.join("a.txt"), "live-a");
    write(&root.join("b.txt"), "live-b");

    let error = restore_impl(root, Some(store.path()), Some(&saved.id), Some(1))
        .err()
        .expect("fault injection must fail the restore")
        .to_string();

    assert!(error.contains("已完整回滚"), "{error}");
    assert_eq!(fs::read_to_string(root.join("a.txt")).unwrap(), "live-a");
    assert_eq!(fs::read_to_string(root.join("b.txt")).unwrap(), "live-b");
    let workspace_store = workspace_dir(root, Some(store.path())).unwrap();
    assert!(
        fs::read_dir(workspace_store)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .all(|entry| !entry
                .file_name()
                .to_string_lossy()
                .starts_with(RESTORE_TRANSACTION_PREFIX)),
        "successful rollback should remove its transaction directory"
    );
}

#[test]
fn restore_rollback_removes_new_file_and_new_parent_directory() {
    let ws = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    let root = ws.path();
    write(&root.join("nested/new.txt"), "checkpoint-new");
    write(&root.join("z.txt"), "checkpoint-z");
    let saved = save(root, Some(store.path()), DEFAULT_KEEP).unwrap();
    fs::remove_file(root.join("nested/new.txt")).unwrap();
    fs::remove_dir(root.join("nested")).unwrap();
    write(&root.join("z.txt"), "live-z");

    let error = restore_impl(root, Some(store.path()), Some(&saved.id), Some(1))
        .err()
        .expect("fault injection must fail the restore")
        .to_string();

    assert!(error.contains("已完整回滚"), "{error}");
    assert!(!root.join("nested/new.txt").exists());
    assert!(!root.join("nested").exists());
    assert_eq!(fs::read_to_string(root.join("z.txt")).unwrap(), "live-z");
}

#[cfg(unix)]
#[test]
fn save_and_restore_reject_symlink_traversal() {
    use std::os::unix::fs::symlink;

    let ws = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let root = ws.path();
    write(&root.join("nested/link/a.txt"), "a");
    let saved = save(root, Some(store.path()), DEFAULT_KEEP).unwrap();

    // 恢复目标的父目录若是 symlink，绝不能把内容写到工作区外。
    fs::remove_file(root.join("nested/link/a.txt")).unwrap();
    fs::remove_dir(root.join("nested/link")).unwrap();
    symlink(outside.path(), root.join("nested/link")).unwrap();
    // manifest 仍完整有效，目的路径本身才是被替换的攻击面。
    assert!(verify_snapshot(&saved.path).is_ok());
    assert!(restore(root, Some(store.path()), Some(&saved.id)).is_err());
    assert!(!outside.path().join("a.txt").exists());

    // 状态白名单中的 symlink 也会使保存 fail-closed，而不是跟随链接抓取外部文件。
    let second = tempfile::tempdir().unwrap();
    write(&second.path().join("source.txt"), "x");
    write(&outside.path().join("pending.json"), "outside");
    fs::create_dir_all(second.path().join(".RaymanCodingSkill")).unwrap();
    symlink(
        outside.path().join("pending.json"),
        second.path().join(".RaymanCodingSkill/pending.json"),
    )
    .unwrap();
    assert!(save(second.path(), Some(store.path()), DEFAULT_KEEP).is_err());

    // The checkpoint root itself must not be reached through a symlinked
    // parent, even when the final directory resolves to a real directory.
    let redirected_parent = store.path().join("redirected");
    symlink(outside.path(), &redirected_parent).unwrap();
    assert!(
        save(
            root,
            Some(&redirected_parent.join("checkpoints")),
            DEFAULT_KEEP
        )
        .is_err()
    );
    assert!(
        !outside.path().join("checkpoints").exists(),
        "checkpoint path validation must happen before creating through a link"
    );
}
