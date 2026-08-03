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
fn default_save_preserves_all_recovery_points_until_explicit_prune() {
    let ws = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    let root = ws.path();
    write(&root.join("src/main.rs"), "fn main() {}");

    for index in 0..4 {
        write(
            &root.join("src/main.rs"),
            &format!("fn value() -> usize {{ {index} }}"),
        );
        let outcome = save_without_prune(root, Some(store.path())).unwrap();
        assert_eq!(outcome.pruned, 0);
    }
    assert_eq!(list(root, Some(store.path())).unwrap().len(), 4);

    let removed = prune_standard_snapshots(root, Some(store.path()), 2).unwrap();
    assert_eq!(removed, 2);
    assert_eq!(list(root, Some(store.path())).unwrap().len(), 2);
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

/// 恢复被删除的文件必须还原**原始大小写**的文件名。
///
/// manifest 路径曾经存的是大小写折叠后的比较键，而 restore 又拿它当真实落盘路径，
/// 于是 Windows 上恢复 `README.md` 会造出 `readme.md`。本地 git 有 core.ignorecase
/// 兜着看不出来，跨平台克隆/CI 才会炸。测试文件名必须含大写字母——现有恢复测试全用
/// 小写文件名，测不出这个缺陷。
#[cfg(windows)]
#[test]
fn restore_recreates_deleted_files_with_their_original_name_case() {
    fn entry_names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        names.sort();
        names
    }

    let ws = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    let root = ws.path();
    write(&root.join("README.md"), "docs");
    write(&root.join("Cargo.toml"), "[package]");
    write(&root.join("Docs/Guide.md"), "guide");
    let saved = save(root, Some(store.path()), DEFAULT_KEEP).unwrap();

    let manifest = verify_snapshot(&saved.path).unwrap();
    let recorded: Vec<String> = manifest.files.iter().map(|f| f.path.clone()).collect();
    assert!(recorded.contains(&"README.md".to_string()), "{recorded:?}");
    assert!(recorded.contains(&"Cargo.toml".to_string()), "{recorded:?}");
    assert!(
        recorded.contains(&"Docs/Guide.md".to_string()),
        "{recorded:?}"
    );

    fs::remove_file(root.join("README.md")).unwrap();
    fs::remove_file(root.join("Cargo.toml")).unwrap();
    fs::remove_dir_all(root.join("Docs")).unwrap();
    restore(root, Some(store.path()), Some(&saved.id)).unwrap();

    assert_eq!(
        entry_names(root),
        vec![
            "Cargo.toml".to_string(),
            "Docs".to_string(),
            "README.md".to_string()
        ],
        "恢复必须重建原始大小写的文件名和父目录名"
    );
    assert_eq!(
        entry_names(&root.join("Docs")),
        vec!["Guide.md".to_string()]
    );
    assert_eq!(fs::read_to_string(root.join("README.md")).unwrap(), "docs");
}

/// committed 孤儿 = 恢复已经发布完成、只是清理目录没删掉（AV/索引器持句柄很常见）。
///
/// 这种孤儿曾会让下一次 save/restore 对整个工作区重跑发布后校验：恢复之后的任何
/// 正常编辑都会让校验失败并 bail，于是 `checkpoint save`（含每 30 分钟的 autosave
/// tick）和 `restore` 全部永久失败，只能手工删目录。
#[test]
fn committed_orphan_transaction_is_cleanable_after_ordinary_edits() {
    let ws = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    let root = ws.path();
    write(&root.join("a.txt"), "checkpoint-a");
    let saved = save(root, Some(store.path()), DEFAULT_KEEP).unwrap();
    let manifest = verify_snapshot(&saved.path).unwrap();

    let ws_dir = workspace_dir(root, Some(store.path())).unwrap();
    let transaction = committed_orphan(root, &ws_dir, &manifest, RestorePhase::Committed);

    // 恢复发布完成之后用户继续编辑，是完全正常的事。
    write(&root.join("a.txt"), "edited-after-a-successful-restore");

    let after = save(root, Some(store.path()), DEFAULT_KEEP).unwrap();
    assert!(after.path.exists(), "committed 孤儿不得阻塞后续保存");
    assert!(
        !transaction.exists(),
        "committed 孤儿必须能被清理，否则没有自愈路径"
    );
    restore(root, Some(store.path()), Some(&saved.id)).unwrap();
}

/// 发布中途的孤儿仍必须 fail-closed：那时工作区状态未知，只有回滚能证明安全。
#[test]
fn publishing_orphan_transaction_still_fails_closed_on_third_party_drift() {
    let ws = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    let root = ws.path();
    write(&root.join("a.txt"), "checkpoint-a");
    let saved = save(root, Some(store.path()), DEFAULT_KEEP).unwrap();
    let manifest = verify_snapshot(&saved.path).unwrap();

    let ws_dir = workspace_dir(root, Some(store.path())).unwrap();
    let transaction = committed_orphan(root, &ws_dir, &manifest, RestorePhase::Publishing);
    write(&root.join("a.txt"), "written-by-someone-else");

    assert!(save(root, Some(store.path()), DEFAULT_KEEP).is_err());
    assert!(
        transaction.exists(),
        "发布中途的孤儿回滚失败时必须保留供人工恢复"
    );
}

/// 造一个孤儿 restore transaction：所有条目都已发布、原本不存在于工作区。
fn committed_orphan(
    root: &Path,
    ws_dir: &Path,
    manifest: &Manifest,
    phase: RestorePhase,
) -> PathBuf {
    let transaction = ws_dir.join(format!(
        "{RESTORE_TRANSACTION_PREFIX}{}-orphan-{:?}",
        std::process::id(),
        phase
    ));
    fs::create_dir(&transaction).unwrap();
    fs::create_dir(transaction.join("staged")).unwrap();
    fs::create_dir(transaction.join("backups")).unwrap();
    let journal = RestoreJournal {
        schema: RESTORE_JOURNAL_SCHEMA.to_string(),
        version: RESTORE_JOURNAL_VERSION,
        workspace_root: display_path(&root.canonicalize().unwrap()),
        phase,
        entries: manifest
            .files
            .iter()
            .map(|expected| RestoreTransactionEntry {
                expected: expected.clone(),
                original: None,
                destination_prepared: true,
                publish_attempted: true,
                rollback_complete: false,
            })
            .collect(),
        created_directories: Vec::new(),
    };
    crate::file_io::write_json(&transaction.join(RESTORE_JOURNAL_NAME), &journal).unwrap();
    transaction
}

/// 相对 `--dir` 必须锚在工作区根，而不是进程 cwd：autosave 把这个相对字符串
/// 原样存进状态，计划任务却以工作区根为工作目录运行，从子目录跑一次手动 save
/// 就会把 store 劈成两个，`checkpoint list` 互相看不见对方的快照。
#[test]
fn a_relative_dir_store_is_anchored_at_the_workspace_root() {
    let ws = tempfile::tempdir().unwrap();
    let root = ws.path();
    write(&root.join("a.txt"), "checkpoint-a");
    fs::create_dir_all(root.join("sub")).unwrap();

    let from_root = checkpoints_root(root, Some(Path::new("ckpt"))).unwrap();
    let canonical = root.canonicalize().unwrap();
    assert_eq!(from_root, canonical.join("ckpt"));

    // 绝对路径原样保留。
    let absolute = ws.path().join("elsewhere");
    assert_eq!(
        checkpoints_root(root, Some(&absolute)).unwrap(),
        absolute
    );

    // 存进去再列出来：同一个相对 --dir 必须命中同一个 store。
    let saved = save(root, Some(Path::new("ckpt")), DEFAULT_KEEP).unwrap();
    let listed = list(root, Some(Path::new("ckpt"))).unwrap();
    assert!(
        listed.iter().any(|entry| entry.id == saved.id),
        "listed={:?}",
        listed.iter().map(|entry| &entry.id).collect::<Vec<_>>()
    );
    assert!(canonical.join("ckpt").is_dir());
}

/// 崩溃点落在"Planned 目录记录落盘"与"Created 记录落盘"之间：目录已由本事务
/// `create_dir` 出来，journal 里却还是 Planned。回滚此前无条件拒绝删除这种目录，
/// 于是孤儿事务永远回滚不完，之后每一次 save/restore/autosave tick 都在工作区锁
/// 下报错，且没有任何自愈路径。空目录必须安全回收；非空目录仍然保留，并且错误
/// 里要指出 transaction 目录，用户才知道怎么解除阻塞。
#[test]
fn planned_created_directory_orphan_is_reclaimed_when_empty() {
    fn planned_orphan(root: &Path, ws_dir: &Path, manifest: &Manifest, suffix: &str) -> PathBuf {
        let transaction = ws_dir.join(format!(
            "{RESTORE_TRANSACTION_PREFIX}{}-planned-{suffix}",
            std::process::id()
        ));
        fs::create_dir(&transaction).unwrap();
        fs::create_dir(transaction.join("staged")).unwrap();
        fs::create_dir(transaction.join("backups")).unwrap();
        let journal = RestoreJournal {
            schema: RESTORE_JOURNAL_SCHEMA.to_string(),
            version: RESTORE_JOURNAL_VERSION,
            workspace_root: display_path(&root.canonicalize().unwrap()),
            phase: RestorePhase::Preparing,
            entries: manifest
                .files
                .iter()
                .map(|expected| RestoreTransactionEntry {
                    expected: expected.clone(),
                    original: None,
                    destination_prepared: false,
                    publish_attempted: false,
                    rollback_complete: false,
                })
                .collect(),
            created_directories: vec![RestoreCreatedDirectory {
                path: "nested".into(),
                state: RestoreDirectoryState::Planned,
            }],
        };
        crate::file_io::write_json(&transaction.join(RESTORE_JOURNAL_NAME), &journal).unwrap();
        transaction
    }

    let ws = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    let root = ws.path();
    write(&root.join("nested/new.txt"), "checkpoint-new");
    let saved = save(root, Some(store.path()), DEFAULT_KEEP).unwrap();
    let manifest = verify_snapshot(&saved.path).unwrap();
    let ws_dir = workspace_dir(root, Some(store.path())).unwrap();

    // 空目录：本事务刚建的，安全回收，阻塞解除。
    fs::remove_file(root.join("nested/new.txt")).unwrap();
    let transaction = planned_orphan(root, &ws_dir, &manifest, "empty");
    recover_orphaned_restore_transactions(root, &ws_dir).unwrap();
    assert!(!transaction.exists(), "空的 Planned 孤儿必须能被清理");
    assert!(!root.join("nested").exists());
    save(root, Some(store.path()), DEFAULT_KEEP).expect("孤儿清理后保存必须恢复正常");

    // 非空目录：无法证明所有权，保留并给出可执行的解除指引。
    write(&root.join("nested/other.txt"), "third-party");
    let transaction = planned_orphan(root, &ws_dir, &manifest, "nonempty");
    let error = format!(
        "{:#}",
        recover_orphaned_restore_transactions(root, &ws_dir).unwrap_err()
    );
    assert!(
        error.contains("且非空") && error.contains("以解除阻塞"),
        "非空目录必须给出可执行的解除指引: {error}"
    );
    assert!(transaction.exists(), "非空目录的孤儿必须保留供人工处置");
    assert_eq!(
        fs::read_to_string(root.join("nested/other.txt")).unwrap(),
        "third-party",
        "第三方内容不得被删除"
    );
}

/// 没有 journal 的孤儿 = 事务还没开始发布任何文件（journal 在任何落盘之前写入），
/// 或者清理 `remove_dir_all` 只删到一半。
///
/// 这种孤儿曾直接 bail "无法安全加载"，而 save 与 restore 都在工作区锁下跑这段恢复：
/// 一个半创建的空目录就能让 `checkpoint save`（含每次 autosave tick）和 `restore`
/// 永久失败，只能手工删目录。
#[test]
fn journalless_orphan_transaction_is_reaped_instead_of_deadlocking() {
    let ws = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    let root = ws.path();
    write(&root.join("a.txt"), "checkpoint-a");
    let saved = save(root, Some(store.path()), DEFAULT_KEEP).unwrap();
    let ws_dir = workspace_dir(root, Some(store.path())).unwrap();

    // 创建到一半就断电：目录与两个空子目录已 mkdir，journal 尚未落盘。
    let half_created = ws_dir.join(format!(
        "{RESTORE_TRANSACTION_PREFIX}{}-half-created",
        std::process::id()
    ));
    fs::create_dir(&half_created).unwrap();
    fs::create_dir(half_created.join("staged")).unwrap();
    fs::create_dir(half_created.join("backups")).unwrap();

    // 删除到一半：journal 与 backups 已删，staged 的副本还在。
    let half_deleted = ws_dir.join(format!(
        "{RESTORE_TRANSACTION_PREFIX}{}-half-deleted",
        std::process::id()
    ));
    fs::create_dir(&half_deleted).unwrap();
    write(&half_deleted.join("staged/a.txt"), "checkpoint-a");

    let after = save(root, Some(store.path()), DEFAULT_KEEP).unwrap();
    assert!(after.path.exists(), "没有 journal 的孤儿不得阻塞保存");
    assert!(
        !half_created.exists() && !half_deleted.exists(),
        "没有 journal 的孤儿必须能被清理，否则没有自愈路径"
    );
    restore(root, Some(store.path()), Some(&saved.id)).unwrap();
}

/// 但没有 journal **且**留有备份时仍必须 fail-closed：备份可能是被发布覆盖掉的原始
/// 内容的唯一副本，而没有 journal 就无从判断该回滚哪些目标。
#[test]
fn journalless_orphan_transaction_with_backups_still_fails_closed() {
    let ws = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    let root = ws.path();
    write(&root.join("a.txt"), "checkpoint-a");
    save(root, Some(store.path()), DEFAULT_KEEP).unwrap();
    let ws_dir = workspace_dir(root, Some(store.path())).unwrap();

    let orphan = ws_dir.join(format!(
        "{RESTORE_TRANSACTION_PREFIX}{}-backups-without-journal",
        std::process::id()
    ));
    fs::create_dir(&orphan).unwrap();
    fs::create_dir(orphan.join("staged")).unwrap();
    write(&orphan.join("backups/a.txt"), "pre-restore-a");

    assert!(save(root, Some(store.path()), DEFAULT_KEEP).is_err());
    assert!(
        orphan.join("backups/a.txt").exists(),
        "无 journal 的备份是唯一副本，必须保留供人工恢复"
    );
}

/// 升级前生成的快照：tree 里是真实大小写，manifest 里存的却是折叠后的小写键。
///
/// 上一轮只让**新**快照保留大小写，存量快照仍会把 `README.md` 恢复成 `readme.md`，
/// 而且没有任何机制告诉用户。恢复必须明确拒绝，且不得把"文件名本来就是小写"误报。
#[cfg(windows)]
#[test]
fn restore_refuses_legacy_snapshot_whose_manifest_folded_path_case() {
    let ws = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    let root = ws.path();
    write(&root.join("README.md"), "docs");
    write(&root.join("Docs/Guide.md"), "guide");
    write(&root.join("lower.txt"), "lower");
    let saved = save(root, Some(store.path()), DEFAULT_KEEP).unwrap();

    // 旧版本的 manifest 记的是 normalized_path_key（Windows 上小写）；tree 一直是真实大小写。
    let manifest_path = saved.path.join(MANIFEST_NAME);
    let mut manifest = crate::file_io::read_json::<Manifest>(&manifest_path)
        .unwrap()
        .unwrap();
    for file in &mut manifest.files {
        file.path = file.path.to_ascii_lowercase();
    }
    crate::file_io::write_json(&manifest_path, &manifest).unwrap();
    // 折叠后的 manifest 在 Windows 上仍能通过完整性校验——正是它静默生效的原因。
    assert_eq!(
        verify_snapshot(&saved.path).unwrap().status,
        SnapshotStatus::Complete
    );

    fs::remove_file(root.join("README.md")).unwrap();
    let error = restore(root, Some(store.path()), Some(&saved.id))
        .err()
        .expect("旧快照的折叠路径必须被检出，而不是静默恢复错误大小写")
        .to_string();

    assert!(error.contains("大小写"), "{error}");
    assert!(error.contains("README.md"), "{error}");
    assert!(error.contains("Docs/Guide.md"), "{error}");
    // 本来就是小写的文件名不是折叠键，绝不能误报。
    assert!(!error.contains("lower.txt"), "{error}");
    assert!(
        !root.join("README.md").exists(),
        "拒绝恢复时不得落盘任何文件"
    );
}

/// partial 快照是取证证据，但不能无界增长：autosave 的计划任务每 N 分钟重试一次
/// 失败的保存，无上限时一天就能堆出约 48 份，而 prune 从不删它们、失败路径也从不
/// 调 prune。轮换只针对 partial，最近的完整恢复点绝不受影响。
#[test]
fn partial_snapshots_are_capped_without_touching_the_last_complete_snapshot() {
    let ws = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    let root = ws.path();
    write(&root.join("a.txt"), "a");
    let complete = save(root, Some(store.path()), 1).unwrap();

    // goals 应为目录；同名普通文件让此后每次保存都失败并提交一份 partial。
    write(&root.join(".RaymanCodingSkill/goals"), "not a directory");
    let attempts = MAX_PARTIAL_SNAPSHOTS + 3;
    for _ in 0..attempts {
        assert!(save(root, Some(store.path()), 1).is_err());
    }

    let checkpoints = list(root, Some(store.path())).unwrap();
    let partials = checkpoints
        .iter()
        .filter(|checkpoint| checkpoint.status == SnapshotStatus::Partial)
        .count();
    assert!(
        partials <= MAX_PARTIAL_SNAPSHOTS,
        "partial 快照必须有上限（{attempts} 次失败后实得 {partials} 份）"
    );
    assert!(partials > 0, "仍必须保留最近的 partial 快照供取证");
    assert!(
        complete.path.exists(),
        "轮换 partial 绝不能删掉最近的完整恢复点"
    );
    assert_eq!(
        latest(root, Some(store.path())).unwrap().unwrap().id,
        complete.id
    );
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
