use super::*;
use crate::file_io::is_link_or_reparse;

/// 把某个完整快照（默认最近完整快照）叠加恢复到工作区。
/// 覆盖同名文件，但不删除工作区里多出来的文件。恢复使用两阶段事务：全部源文件先
/// 复制进受锁保护的 staging 并校验，所有既有目标再备份，最后才逐个发布。任一发布或
/// 发布后校验失败都会逆序恢复备份、删除本次新增文件和空目录；回滚本身失败时保留
/// transaction 目录供人工恢复，绝不把部分恢复误报为成功。
pub fn restore(
    root: &Path,
    override_dir: Option<&Path>,
    which: Option<&str>,
) -> Result<RestoreOutcome> {
    restore_with_options(root, override_dir, which, false)
}

pub fn restore_with_options(
    root: &Path,
    override_dir: Option<&Path>,
    which: Option<&str>,
    allow_recovery_only: bool,
) -> Result<RestoreOutcome> {
    restore_impl_with_options(root, override_dir, which, allow_recovery_only, None)
}

#[cfg(test)]
pub(super) fn restore_impl(
    root: &Path,
    override_dir: Option<&Path>,
    which: Option<&str>,
    fail_on_publish_index: Option<usize>,
) -> Result<RestoreOutcome> {
    restore_impl_with_options(root, override_dir, which, false, fail_on_publish_index)
}

fn restore_impl_with_options(
    root: &Path,
    override_dir: Option<&Path>,
    which: Option<&str>,
    allow_recovery_only: bool,
    fail_on_publish_index: Option<usize>,
) -> Result<RestoreOutcome> {
    ensure_real_directory(root)?;
    let root = root
        .canonicalize()
        .with_context(|| format!("无法规范化工作区根: {}", display_path(root)))?;
    let ws_dir = workspace_dir(&root, override_dir)?;
    if !ws_dir.exists() {
        bail!("没有可恢复的 checkpoint");
    }
    ensure_real_directory_chain(&ws_dir)?;
    ensure_real_directory(&ws_dir)?;
    // Lock order is fixed at autosave → checkpoint → per-file state locks,
    // because an autosave tick takes the autosave lock and then calls
    // `checkpoint::save`. Taking the autosave lock after the checkpoint lock
    // here would close an ABBA cycle with every scheduled tick.
    //
    // Probe the managed state root first and never create it: a workspace
    // without one has no autosave writer to race, and minting state from a
    // read-shaped step is its own defect.
    let _autosave_lock = match crate::state_paths::managed_state_root(&root, false)? {
        Some(_) => Some(crate::autosave::acquire_lock(&root)?),
        None => None,
    };
    let _lock = CheckpointLock::acquire(&ws_dir)?;
    recover_orphaned_restore_transactions(&root, &ws_dir)?;
    let checkpoints = list(&root, override_dir)?;
    if checkpoints.is_empty() {
        bail!("没有可恢复的 checkpoint");
    }
    let target = match which {
        None | Some("latest") => checkpoints
            .iter()
            .rev()
            .find(|checkpoint| {
                checkpoint.status == SnapshotStatus::Complete
                    && checkpoint
                        .manifest
                        .as_ref()
                        .is_some_and(|manifest| manifest.purpose == CheckpointPurpose::Standard)
            })
            .ok_or_else(|| anyhow::anyhow!("没有完整且已验证的 checkpoint"))?,
        Some(id) => checkpoints
            .iter()
            .find(|checkpoint| checkpoint.id == id)
            .ok_or_else(|| anyhow::anyhow!("找不到 checkpoint: {id}"))?,
    };
    if target.status != SnapshotStatus::Complete {
        bail!(
            "拒绝恢复非完整 checkpoint {}（状态：{:?}）",
            target.id,
            target.status
        );
    }
    let manifest = verify_snapshot(&target.path)?;
    if manifest.purpose == CheckpointPurpose::RecoveryOnly {
        if !allow_recovery_only {
            bail!(
                "checkpoint {} 是 recovery-only；修复并重新激活工作区后，显式加 --allow-recovery-only 才能恢复",
                target.id
            );
        }
        crate::workspace::require_active(&root).with_context(|| {
            format!(
                "拒绝恢复 recovery-only checkpoint {}：当前激活合同尚未修复",
                target.id
            )
        })?;
    }
    let tree = target.path.join(TREE_SUBDIR);
    ensure_manifest_paths_preserve_case(&target.id, &tree, &manifest)?;

    let mut files = manifest.files.clone();
    files.sort_by(|a, b| a.path.cmp(&b.path));

    let mut transaction = create_restore_transaction(&root, &ws_dir, &files)?;

    // Staging only reads the snapshot tree and writes inside the transaction
    // directory — it touches nothing in the workspace — and it is by far the
    // longest phase (copy + hash-verify of every file). Taking the state locks
    // before it made restore hold locks sized for a millisecond read-modify-write
    // for the whole multi-minute copy, so any concurrent `goal`/`pending` write
    // hit the 2.5 s timeout. Growing that timeout is not fixable by any constant
    // (the hold is O(snapshot)), so shrink the hold instead.
    let staged = stage_restore_sources(&tree, &mut transaction);

    // Managed state files have their own writers (goal receipts, pending items,
    // autosave bookkeeping) that serialize on per-file state locks, not on the
    // checkpoint lock. Publishing them while holding only the checkpoint lock let
    // a concurrently committed write be silently replaced, and left a
    // validate→rename window where restore could overwrite bytes it had just
    // verified as unchanged.
    //
    // The guard MUST live in the function body, not inside the operation
    // closure: rollback republishes those same files from `backups/`, and a
    // closure-local guard is dropped before `finish_failed_restore` runs — which
    // is exactly what `recover_orphaned_restore_transactions` documents as the
    // reason it takes these locks before rolling back an orphan.
    let state_locks =
        acquire_managed_state_locks(&root, files.iter().map(|file| file.path.clone()))?;

    let operation = staged.and_then(|()| {
        prepare_restore_destinations(&root, &mut transaction)?;
        validate_restore_destinations_unchanged(&root, &transaction)?;
        publish_restore_transaction(&root, &mut transaction, fail_on_publish_index)?;
        validate_published_restore(&root, &transaction)?;
        set_restore_phase(&mut transaction, RestorePhase::Committed)
    });

    if let Err(error) = operation {
        return finish_failed_restore(&root, &ws_dir, &mut transaction, error, &state_locks);
    }
    remove_restore_transaction(&ws_dir, &transaction.path).with_context(|| {
        format!(
            "checkpoint 已完整恢复，但无法清理 restore transaction {}",
            display_path(&transaction.path)
        )
    })?;

    Ok(RestoreOutcome {
        id: target.id.clone(),
        restored: files.len(),
        failed: 0,
    })
}

/// Refuse a snapshot whose manifest recorded **case-folded** path keys.
///
/// Snapshots written before [`manifest_path_string`] existed stored the
/// comparison key — lowercased on Windows — as the manifest path, while the same
/// save always wrote the snapshot's own `tree/` through the real, case-preserving
/// relative path.  Restore joins the manifest string onto the workspace root, so
/// such a snapshot silently recreates `README.md` as `readme.md`: invisible
/// locally under `core.ignorecase`, a corrupted tree for every cross-platform
/// clone.  Preserving case in new saves fixed nothing for the snapshots users
/// already hold, and nothing told them their restores were unsafe.
///
/// The tree is the ground truth for what the file was really called, so each
/// manifest path is compared against the actual on-disk name inside the tree.
/// That is what distinguishes a folded key from a file whose name simply is
/// lowercase: the latter matches the tree exactly and passes untouched.  Entries
/// with no tree match at all are left to [`verify_manifest_tree`], whose missing
/// file/integrity errors are the accurate diagnosis there.
fn ensure_manifest_paths_preserve_case(id: &str, tree: &Path, manifest: &Manifest) -> Result<()> {
    let mut names = TreeNameIndex::default();
    let mut folded = Vec::new();
    for expected in &manifest.files {
        let relative = manifest_relative_path(&expected.path)?;
        if let Some(actual) = names.resolve(tree, &relative)?
            && actual != expected.path
        {
            // Anchored on a leading static: a template whose statics are
            // `["", " 实为 ", ""]` matches *any* line containing that fragment,
            // so registering it as an authored message let the en localizer
            // rewrite user-authored Chinese that merely happened to contain it.
            folded.push(format!("manifest 记录 {} 实为 {}", expected.path, actual));
        }
    }
    if !folded.is_empty() {
        bail!(
            "拒绝恢复 checkpoint {}：它由旧版本 Rayman 生成，manifest 记录的是大小写折叠后的比较键而非真实文件名，按它恢复出的文件名大小写不可信（{}）；请用当前版本重新 `rayman checkpoint save` 后再恢复",
            id,
            folded.join("；")
        );
    }
    Ok(())
}

/// Real on-disk names inside a snapshot tree, cached per directory.
///
/// Resolution is per path component, so a single folded component anywhere in
/// the path is detected.  The cache keeps the scan linear in the tree size
/// rather than re-listing a directory once per manifest entry it contains.
#[derive(Default)]
struct TreeNameIndex {
    directories: std::collections::HashMap<PathBuf, std::collections::HashMap<String, Vec<String>>>,
}

impl TreeNameIndex {
    /// The tree's own spelling of `relative`, or `None` when no entry matches
    /// even case-insensitively.
    fn resolve(&mut self, tree: &Path, relative: &Path) -> Result<Option<String>> {
        let mut current = tree.to_path_buf();
        let mut parts: Vec<String> = Vec::new();
        for component in relative.components() {
            let Component::Normal(name) = component else {
                bail!("checkpoint 路径含不安全组件: {}", relative.display());
            };
            let wanted = name.to_str().ok_or_else(|| {
                anyhow::anyhow!("checkpoint 路径不是有效 UTF-8: {}", relative.display())
            })?;
            let Some(actual) = self.entry_name(&current, wanted)? else {
                return Ok(None);
            };
            current.push(&actual);
            parts.push(actual);
        }
        if parts.is_empty() {
            bail!("checkpoint 路径为空");
        }
        Ok(Some(parts.join("/")))
    }

    fn entry_name(&mut self, directory: &Path, wanted: &str) -> Result<Option<String>> {
        if !self.directories.contains_key(directory) {
            let mut listing: std::collections::HashMap<String, Vec<String>> =
                std::collections::HashMap::new();
            match fs::read_dir(directory) {
                Ok(entries) => {
                    for entry in entries {
                        let entry = entry.with_context(|| {
                            format!("无法读取 checkpoint 树条目: {}", display_path(directory))
                        })?;
                        if let Some(name) = entry.file_name().to_str() {
                            listing
                                .entry(name.to_ascii_lowercase())
                                .or_default()
                                .push(name.to_string());
                        }
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!("无法列出 checkpoint 树目录: {}", display_path(directory))
                    });
                }
            }
            self.directories.insert(directory.to_path_buf(), listing);
        }
        let candidates = match self.directories[directory].get(&wanted.to_ascii_lowercase()) {
            Some(candidates) => candidates,
            None => return Ok(None),
        };
        // An exact hit wins: a case-sensitive filesystem may legitimately hold
        // both `README.md` and `readme.md` side by side.
        if let Some(exact) = candidates.iter().find(|name| name.as_str() == wanted) {
            return Ok(Some(exact.clone()));
        }
        Ok(candidates.first().cloned())
    }
}

/// Take the writers' own locks for every managed state file this restore will
/// publish, in a fixed path order so two concurrent restores (two `--dir` stores
/// for one workspace share no checkpoint lock) can never deadlock.
///
/// `autosave.json` is guarded by `autosave.lock`, which the caller already holds
/// — see the lock-order note at the top of the restore.
fn acquire_managed_state_locks(
    root: &Path,
    paths: impl IntoIterator<Item = String>,
) -> Result<Vec<crate::state_lock::StateLock>> {
    let state_prefix = format!("{}/", crate::state_paths::STATE_DIR_NAME);
    let mut targets: Vec<PathBuf> = Vec::new();
    for path in paths {
        let Some(relative) = path.strip_prefix(&state_prefix) else {
            continue;
        };
        // Only files an independent writer serializes on: pending.json and the
        // per-goal records. Context index/project map have no such writer.
        if relative != "pending.json" && !relative.starts_with("goals/") {
            continue;
        }
        let verified = manifest_relative_path(&path)?;
        let Ok(under_state) = verified.strip_prefix(crate::state_paths::STATE_DIR_NAME) else {
            continue;
        };
        // Resolve through the managed-state helper *creating parents*, exactly
        // like the writers whose locks these are. Locking a bare `root.join(..)`
        // instead required `.RaymanCodingSkill/goals/` to already exist — while
        // restore is precisely the operation that recreates it — so restoring a
        // goal ledger into a freshly repaired workspace aborted before the
        // transaction was even created, restoring nothing. The rest of the
        // restore creates its destination parents itself; this step must too.
        targets.push(crate::state_paths::managed_state_file(
            root,
            under_state,
            true,
        )?);
    }
    targets.sort();
    targets.dedup();

    let mut locks = Vec::with_capacity(targets.len());
    for target in &targets {
        locks.push(crate::state_lock::acquire_state_lock(target)?);
    }
    Ok(locks)
}

fn create_restore_transaction(
    root: &Path,
    ws_dir: &Path,
    files: &[FileIntegrity],
) -> Result<RestoreTransaction> {
    ensure_real_directory(ws_dir)?;
    let id = fs_safe_id(&crate::timefmt::now_iso());
    let path = ws_dir.join(format!(
        "{RESTORE_TRANSACTION_PREFIX}{}-{id}",
        std::process::id()
    ));
    fs::create_dir(&path)
        .with_context(|| format!("无法创建 restore transaction 目录: {}", display_path(&path)))?;
    ensure_real_directory(&path)?;
    sync_directory(ws_dir)?;
    let staged = path.join("staged");
    let backups = path.join("backups");
    fs::create_dir(&staged)
        .with_context(|| format!("无法创建 restore staging: {}", display_path(&staged)))?;
    fs::create_dir(&backups)
        .with_context(|| format!("无法创建 restore backups: {}", display_path(&backups)))?;
    ensure_real_directory(&staged)?;
    ensure_real_directory(&backups)?;
    sync_directory(&path)?;

    let mut entries = Vec::with_capacity(files.len());
    for expected in files {
        validate_integrity_record(expected)?;
        manifest_relative_path(&expected.path)?;
        entries.push(RestoreTransactionEntry {
            expected: expected.clone(),
            original: None,
            destination_prepared: false,
            publish_attempted: false,
            rollback_complete: false,
        });
    }
    let transaction = RestoreTransaction {
        path,
        staged,
        backups,
        journal: RestoreJournal {
            schema: RESTORE_JOURNAL_SCHEMA.to_string(),
            version: RESTORE_JOURNAL_VERSION,
            workspace_root: display_path(root),
            phase: RestorePhase::Preparing,
            entries,
            created_directories: Vec::new(),
        },
    };
    persist_restore_journal(&transaction)?;
    Ok(transaction)
}

fn stage_restore_sources(tree: &Path, transaction: &mut RestoreTransaction) -> Result<()> {
    for entry in &transaction.journal.entries {
        let relative = manifest_relative_path(&entry.expected.path)?;
        ensure_safe_file_under(tree, &relative)?;
        let source = integrity_for_file(tree, &relative)?;
        if source != entry.expected {
            bail!("恢复前源文件完整性发生变化: {}", entry.expected.path);
        }
        let destination = prepare_destination_file(&transaction.staged, &relative)?;
        crate::file_io::copy_atomic_verified(
            &tree.join(&relative),
            &destination,
            entry.expected.size,
            &entry.expected.sha256,
        )
        .with_context(|| {
            format!(
                "无法预备 restore staging 文件: {}",
                display_path(&tree.join(&relative))
            )
        })?;
        let staged = integrity_for_file(&transaction.staged, &relative)?;
        if staged != entry.expected {
            bail!("restore staging 完整性不匹配: {}", entry.expected.path);
        }
    }
    Ok(())
}

fn prepare_restore_destinations(root: &Path, transaction: &mut RestoreTransaction) -> Result<()> {
    for index in 0..transaction.journal.entries.len() {
        let relative = manifest_relative_path(&transaction.journal.entries[index].expected.path)?;
        let destination = prepare_restore_destination_file(root, &relative, transaction)?;
        let mut original = None;
        match fs::symlink_metadata(&destination) {
            Ok(_) => {
                let measured = integrity_for_file(root, &relative)?;
                let backup = prepare_destination_file(&transaction.backups, &relative)?;
                crate::file_io::copy_atomic_verified(
                    &destination,
                    &backup,
                    measured.size,
                    &measured.sha256,
                )
                .with_context(|| {
                    format!("无法备份 restore 目标: {}", display_path(&destination))
                })?;
                let backed_up = integrity_for_file(&transaction.backups, &relative)?;
                if backed_up != measured {
                    bail!(
                        "restore 目标备份完整性不匹配: {}",
                        transaction.journal.entries[index].expected.path
                    );
                }
                original = Some(measured);
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("无法检查 restore 目标: {}", display_path(&destination))
                });
            }
        }
        transaction.journal.entries[index].original = original;
        transaction.journal.entries[index].destination_prepared = true;
        persist_restore_journal(transaction)?;
    }
    Ok(())
}

fn validate_restore_destinations_unchanged(
    root: &Path,
    transaction: &RestoreTransaction,
) -> Result<()> {
    for entry in &transaction.journal.entries {
        if !entry.destination_prepared {
            bail!("restore transaction 目标尚未预备: {}", entry.expected.path);
        }
        validate_restore_destination_unchanged(root, entry)?;
    }
    Ok(())
}

fn validate_restore_destination_unchanged(
    root: &Path,
    entry: &RestoreTransactionEntry,
) -> Result<()> {
    let relative = manifest_relative_path(&entry.expected.path)?;
    let destination = root.join(&relative);
    match &entry.original {
        Some(original) => {
            let current = integrity_for_file(root, &relative)?;
            if current != *original {
                bail!(
                    "restore 目标在备份后发生变化，拒绝覆盖: {}",
                    entry.expected.path
                );
            }
        }
        None => match fs::symlink_metadata(&destination) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Ok(_) => bail!(
                "原本不存在的 restore 目标在发布前出现，拒绝覆盖: {}",
                entry.expected.path
            ),
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("无法复查 restore 目标: {}", display_path(&destination))
                });
            }
        },
    }
    Ok(())
}

fn publish_restore_transaction(
    root: &Path,
    transaction: &mut RestoreTransaction,
    fail_on_publish_index: Option<usize>,
) -> Result<()> {
    for index in 0..transaction.journal.entries.len() {
        publish_restore_entry(
            root,
            transaction,
            index,
            fail_on_publish_index == Some(index),
        )?;
    }
    Ok(())
}

fn publish_restore_entry(
    root: &Path,
    transaction: &mut RestoreTransaction,
    index: usize,
    fail_before_copy: bool,
) -> Result<()> {
    let entry = transaction
        .journal
        .entries
        .get(index)
        .ok_or_else(|| anyhow::anyhow!("restore transaction 发布索引越界: {index}"))?;
    validate_restore_destination_unchanged(root, entry)?;
    let relative = manifest_relative_path(&entry.expected.path)?;
    prepare_destination_file(root, &relative)?;
    ensure_safe_file_under(&transaction.staged, &relative)?;

    // Persist the attempt before touching the destination.  The atomic copy can
    // report an fsync error after rename has already published the file, and a
    // process abort at any later instruction must still trigger recovery.
    transaction.journal.phase = RestorePhase::Publishing;
    transaction.journal.entries[index].publish_attempted = true;
    persist_restore_journal(transaction)?;
    let entry = &transaction.journal.entries[index];
    if fail_before_copy {
        bail!(
            "测试注入：第 {} 个 restore 文件发布失败 ({})",
            index + 1,
            entry.expected.path
        );
    }
    let destination = root.join(&relative);
    prepare_atomic_replace_target(&destination)?;
    crate::file_io::copy_atomic_verified(
        &transaction.staged.join(&relative),
        &destination,
        entry.expected.size,
        &entry.expected.sha256,
    )
    .with_context(|| format!("无法发布 restore 文件: {}", display_path(&destination)))?;
    Ok(())
}

fn validate_published_restore(root: &Path, transaction: &RestoreTransaction) -> Result<()> {
    for entry in &transaction.journal.entries {
        let relative = manifest_relative_path(&entry.expected.path)?;
        let written = integrity_for_file(root, &relative)?;
        if written != entry.expected {
            bail!("恢复后目标文件完整性不匹配: {}", entry.expected.path);
        }
    }
    Ok(())
}

/// `state_locks` is unused at runtime and required on purpose: rollback
/// republishes managed state files from `backups/`, so the writers' per-file
/// locks must still be held here. Taking them by reference makes the compiler
/// reject the shape that caused a real regression — binding the guards inside
/// the operation closure, where they are dropped before this call.
fn finish_failed_restore(
    root: &Path,
    ws_dir: &Path,
    transaction: &mut RestoreTransaction,
    operation_error: anyhow::Error,
    state_locks: &[crate::state_lock::StateLock],
) -> Result<RestoreOutcome> {
    let _ = state_locks;
    let rollback = rollback_restore_transaction(root, transaction);
    match rollback {
        Ok(()) => match remove_restore_transaction(ws_dir, &transaction.path) {
            Ok(()) => bail!("checkpoint restore 失败，工作区已完整回滚: {operation_error:#}"),
            Err(cleanup_error) => bail!(
                "checkpoint restore 失败，工作区已回滚，但无法清理 transaction {}: {operation_error:#}; cleanup: {cleanup_error:#}",
                display_path(&transaction.path)
            ),
        },
        Err(rollback_error) => bail!(
            "checkpoint restore 失败且回滚不完整；已保留 transaction {} 供恢复: {operation_error:#}; rollback: {rollback_error:#}",
            display_path(&transaction.path)
        ),
    }
}

fn rollback_restore_transaction(root: &Path, transaction: &mut RestoreTransaction) -> Result<()> {
    if transaction.journal.phase != RestorePhase::RollingBack {
        set_restore_phase(transaction, RestorePhase::RollingBack)?;
    }
    let mut errors = Vec::new();
    for index in (0..transaction.journal.entries.len()).rev() {
        let entry = &transaction.journal.entries[index];
        if !entry.publish_attempted || entry.rollback_complete {
            continue;
        }
        let path = entry.expected.path.clone();
        match rollback_restore_entry(root, transaction, index) {
            Ok(()) => {
                transaction.journal.entries[index].rollback_complete = true;
                if let Err(error) = persist_restore_journal(transaction) {
                    errors.push(format!("{path}: 无法持久化回滚完成状态: {error:#}"));
                }
            }
            Err(error) => errors.push(format!("{path}: {error:#}")),
        }
    }
    for directory in transaction.journal.created_directories.iter().rev() {
        let relative = match manifest_relative_path(&directory.path) {
            Ok(relative) => relative,
            Err(error) => {
                errors.push(format!(
                    "无效的 restore 新建目录 {}: {error:#}",
                    directory.path
                ));
                continue;
            }
        };
        let path = root.join(relative);
        if directory.state == RestoreDirectoryState::Planned && path.exists() {
            // 崩溃点落在"Planned 落盘"与"Created 落盘"之间时，目录是本事务刚
            // create_dir 出来的空目录：安全回收（remove_dir 天然拒绝非空目录，
            // 不会误删第三方内容）。此前这里无条件拒绝，孤儿 journal 会让之后
            // 每一次 save/restore/autosave 永久报错且不给任何解除路径。
            match fs::remove_dir(&path) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => errors.push(format!(
                    "restore 目录只有创建意图且非空，无法证明所有权，拒绝删除 {}（{error}）；确认目录内容后手动删除它，再删除 transaction 目录 {} 以解除阻塞",
                    display_path(&path),
                    display_path(&transaction.path)
                )),
            }
            continue;
        }
        match fs::remove_dir(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => errors.push(format!(
                "无法删除本次 restore 新建目录 {}: {error}",
                display_path(&path)
            )),
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        bail!("{}", errors.join("; "))
    }
}

fn rollback_restore_entry(
    root: &Path,
    transaction: &RestoreTransaction,
    index: usize,
) -> Result<()> {
    let entry = transaction
        .journal
        .entries
        .get(index)
        .ok_or_else(|| anyhow::anyhow!("restore transaction 回滚索引越界: {index}"))?;
    let relative = manifest_relative_path(&entry.expected.path)?;
    let destination = root.join(&relative);
    let current = match fs::symlink_metadata(&destination) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(error)
                .with_context(|| format!("无法检查回滚目标: {}", display_path(&destination)));
        }
        Ok(metadata) if is_link_or_reparse(&metadata) || !metadata.file_type().is_file() => {
            bail!("回滚目标不是安全普通文件: {}", display_path(&destination));
        }
        Ok(_) => Some(integrity_for_file(root, &relative)?),
    };

    match &entry.original {
        Some(original) => {
            // Even when current bytes already equal the original, a publish may
            // have changed readonly/mode metadata.  Always republish the verified
            // backup for an attempted existing entry, then verify the full record.
            if let Some(current) = &current
                && !same_file_content(current, &entry.expected)
                && !same_file_content(current, original)
            {
                // Refusing is right — the user's edit must not be destroyed —
                // but this bail is permanent: the bytes will never again equal
                // `expected` or `original`, and orphan recovery runs at the head
                // of every save and restore. Spell out the escape, exactly like
                // the journal-less branch does, instead of leaving the operator
                // with a wedge and no documented way out.
                bail!(
                    "回滚目标已被第三方修改，拒绝覆盖: {}。原件仍在该 transaction 的 backups/ 子目录中；取回仍需要的内容后删除整个 transaction 目录即可解除对 save/restore/autosave 的阻塞，salvage-save 不受阻塞",
                    display_path(&destination)
                );
            }
            let backup = transaction.backups.join(&relative);
            let backed_up = integrity_for_file(&transaction.backups, &relative)?;
            if backed_up != *original {
                bail!("回滚备份完整性不匹配: {}", entry.expected.path);
            }
            prepare_destination_file(root, &relative)?;
            prepare_atomic_replace_target(&destination)?;
            crate::file_io::copy_atomic_verified(
                &backup,
                &destination,
                original.size,
                &original.sha256,
            )
            .with_context(|| format!("无法回滚 restore 目标: {}", display_path(&destination)))?;
            let restored = integrity_for_file(root, &relative)?;
            if restored != *original {
                bail!("回滚后目标完整性不匹配: {}", entry.expected.path);
            }
        }
        None => {
            if let Some(current) = current {
                if !same_file_content(&current, &entry.expected) {
                    bail!(
                        "本次新增 restore 目标已被第三方修改，拒绝删除: {}",
                        display_path(&destination)
                    );
                }
                fs::remove_file(&destination).with_context(|| {
                    format!(
                        "无法删除本次新增 restore 文件: {}",
                        display_path(&destination)
                    )
                })?;
            }
        }
    }
    Ok(())
}

fn same_file_content(left: &FileIntegrity, right: &FileIntegrity) -> bool {
    left.size == right.size && left.sha256 == right.sha256
}

fn set_restore_phase(transaction: &mut RestoreTransaction, phase: RestorePhase) -> Result<()> {
    transaction.journal.phase = phase;
    persist_restore_journal(transaction)
}

fn persist_restore_journal(transaction: &RestoreTransaction) -> Result<()> {
    ensure_real_directory(&transaction.path)?;
    let journal_path = transaction.path.join(RESTORE_JOURNAL_NAME);
    crate::file_io::write_json(&journal_path, &transaction.journal).with_context(|| {
        format!(
            "无法持久化 restore journal: {}",
            display_path(&journal_path)
        )
    })?;
    ensure_safe_file_under(&transaction.path, Path::new(RESTORE_JOURNAL_NAME))?;
    sync_directory(&transaction.path)
}

pub(super) fn recover_orphaned_restore_transactions(root: &Path, ws_dir: &Path) -> Result<()> {
    ensure_real_directory(root)?;
    ensure_real_directory(ws_dir)?;
    let mut orphans = Vec::new();
    for entry in fs::read_dir(ws_dir)
        .with_context(|| format!("无法扫描 restore transaction: {}", display_path(ws_dir)))?
    {
        let entry = entry.with_context(|| {
            format!(
                "无法读取 restore transaction 条目: {}",
                display_path(ws_dir)
            )
        })?;
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with(RESTORE_TRANSACTION_PREFIX) {
            continue;
        }
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).with_context(|| {
            format!(
                "无法检查 orphan restore transaction: {}",
                display_path(&path)
            )
        })?;
        if is_link_or_reparse(&metadata) || !metadata.file_type().is_dir() {
            bail!(
                "发现不安全的 orphan restore transaction，已保留并拒绝继续: {}",
                display_path(&path)
            );
        }
        orphans.push(path);
    }
    orphans.sort();

    for path in orphans {
        if reap_journalless_restore_transaction(ws_dir, &path)? {
            continue;
        }
        let mut transaction = load_restore_transaction(root, &path).with_context(|| {
            format!(
                "orphan restore transaction 无法安全加载，已保留并拒绝继续: {}",
                display_path(&path)
            )
        })?;
        match transaction.journal.phase {
            // `Committed` is only ever written after every file was published
            // *and* `validate_published_restore` already passed, so the restore
            // is finished and the workspace is the user's to edit again.  This
            // orphan therefore means nothing more than "the cleanup rmdir did
            // not land" — an ordinary outcome when an AV scanner or indexer
            // holds a handle, which is exactly why publishing retries renames.
            //
            // Re-validating workspace contents here made that transient failure
            // permanent: the first ordinary edit afterwards made validation fail
            // forever, and since both `save` and `restore` run this recovery
            // under the workspace lock, every autosave tick and every manual
            // save/restore bailed with no self-healing path short of deleting
            // the directory by hand.  The journal's own committed invariants are
            // still enforced by `validate_restore_journal` above.
            RestorePhase::Committed => {}
            RestorePhase::Preparing | RestorePhase::Publishing | RestorePhase::RollingBack => {
                // Rollback republishes managed state files from `backups/`, which
                // is the same write the forward path takes the writers' per-file
                // locks for. Running it under the checkpoint lock alone let an
                // unattended autosave tick revert a goal record a foreground
                // `goal validate`/`close` was holding the lock to write. Scope
                // the locks to this orphan so the caller can still take them for
                // its own transaction afterwards.
                let _state_locks = acquire_managed_state_locks(
                    root,
                    transaction
                        .journal
                        .entries
                        .iter()
                        .map(|entry| entry.expected.path.clone()),
                )?;
                rollback_restore_transaction(root, &mut transaction).with_context(|| {
                    format!(
                        "orphan restore transaction 自动回滚不完整，已保留并拒绝继续: {}",
                        display_path(&path)
                    )
                })?;
            }
        }
        remove_restore_transaction(ws_dir, &path)?;
    }
    Ok(())
}

/// Reap a transaction directory that has no journal at all.  Returns whether the
/// directory was removed; `false` means a journal is present and normal orphan
/// recovery must run.
///
/// [`create_restore_transaction`] persists `journal.json` *before* the
/// transaction touches anything: the directory and its two empty subdirectories
/// are created, the journal lands (write + fsync + rename), and only then do
/// staging, backup and publish run.  A directory without a journal is therefore
/// either a transaction that died before it began, or one whose `remove_dir_all`
/// cleanup was interrupted after the journal had already been unlinked.  Neither
/// owns any workspace state.
///
/// Leaving those to `load_restore_transaction` made them fatal, and because both
/// `save` and `restore` run this recovery under the workspace lock, a single
/// half-created directory permanently broke every manual save/restore and every
/// autosave tick with no self-healing path short of deleting it by hand.
///
/// A journal that exists is never torn — `write_json` renames a fsynced temp —
/// so an unreadable or invalid one means corruption or tampering and still fails
/// closed below.  `backups/` is likewise still decisive: it can hold the only
/// copy of contents a publish already overwrote, and without a journal there is
/// no way to know which entries those are, so a non-empty one refuses to guess.
fn reap_journalless_restore_transaction(ws_dir: &Path, path: &Path) -> Result<bool> {
    let journal = path.join(RESTORE_JOURNAL_NAME);
    match fs::symlink_metadata(&journal) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error)
                .with_context(|| format!("无法检查 restore journal: {}", display_path(&journal)));
        }
        Ok(_) => return Ok(false),
    }
    if directory_contains_file(&path.join("backups"))? {
        // Fail closed: the backups/ subtree can be the only copy of content a publish
        // already overwrote, and without a journal there is no way to know which entries
        // those are, so we never guess. But name the concrete recovery path — this branch
        // otherwise blocks every save/restore/autosave until the user acts, and the old
        // message did not say how. Recovery is manual by design (not auto-reaped) so the
        // user decides what in backups/ is still needed before it is discarded.
        bail!(
            "orphan restore transaction 没有 journal 却仍存有备份文件，无从判断该回滚哪些目标；已保留供人工恢复。恢复步骤：检查该目录 backups/ 子目录中的原件、取回仍需要的文件，再删除整个目录以解除对 save/restore/autosave 的阻塞：{}",
            display_path(path)
        );
    }
    remove_restore_transaction(ws_dir, path).with_context(|| {
        format!(
            "无法清理没有 journal 的 orphan restore transaction: {}",
            display_path(path)
        )
    })?;
    Ok(true)
}

/// Whether `path` holds any entry that is not a real, empty-or-recursively-empty
/// directory.  A link/reparse point counts as content: it is never ours to
/// discard silently.
fn directory_contains_file(path: &Path) -> Result<bool> {
    let entries = match fs::read_dir(path) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(error).with_context(|| format!("无法扫描目录: {}", display_path(path)));
        }
    };
    for entry in entries {
        let entry = entry.with_context(|| format!("无法读取目录条目: {}", display_path(path)))?;
        let child = entry.path();
        let metadata = fs::symlink_metadata(&child)
            .with_context(|| format!("无法检查目录条目: {}", display_path(&child)))?;
        if metadata.file_type().is_dir() && !is_link_or_reparse(&metadata) {
            if directory_contains_file(&child)? {
                return Ok(true);
            }
        } else {
            return Ok(true);
        }
    }
    Ok(false)
}

fn load_restore_transaction(root: &Path, path: &Path) -> Result<RestoreTransaction> {
    ensure_real_directory(path)?;
    ensure_safe_file_under(path, Path::new(RESTORE_JOURNAL_NAME))?;
    let journal_path = path.join(RESTORE_JOURNAL_NAME);
    let journal = crate::file_io::read_json::<RestoreJournal>(&journal_path)?
        .ok_or_else(|| anyhow::anyhow!("restore transaction 缺少 journal.json"))?;
    validate_restore_journal(root, &journal)?;
    let staged = path.join("staged");
    let backups = path.join("backups");
    ensure_real_directory(&staged)?;
    ensure_real_directory(&backups)?;
    Ok(RestoreTransaction {
        path: path.to_path_buf(),
        staged,
        backups,
        journal,
    })
}

fn validate_restore_journal(root: &Path, journal: &RestoreJournal) -> Result<()> {
    if journal.schema != RESTORE_JOURNAL_SCHEMA || journal.version != RESTORE_JOURNAL_VERSION {
        bail!(
            "restore journal schema/version 不受支持: schema={:?} version={}",
            journal.schema,
            journal.version
        );
    }
    if !restore_journal_workspace_matches(root, &journal.workspace_root)? {
        bail!(
            "restore journal 工作区不匹配: recorded={:?} current={:?}",
            journal.workspace_root,
            display_path(root)
        );
    }
    let mut paths = HashSet::new();
    for entry in &journal.entries {
        validate_integrity_record(&entry.expected)?;
        let relative = manifest_relative_path(&entry.expected.path)?;
        if manifest_path_string(&relative)? != entry.expected.path {
            bail!("restore journal 路径未规范化: {}", entry.expected.path);
        }
        // De-duplicate on the case-folded key: on a case-insensitive filesystem
        // two entries differing only in case would fight over one destination.
        if !paths.insert(normalized_path_key(&relative)?) {
            bail!("restore journal 包含重复路径: {}", entry.expected.path);
        }
        if let Some(original) = &entry.original {
            validate_integrity_record(original)?;
            if original.path != entry.expected.path {
                bail!(
                    "restore journal original/expected 路径不一致: {} != {}",
                    original.path,
                    entry.expected.path
                );
            }
        }
        if entry.publish_attempted && !entry.destination_prepared {
            bail!(
                "restore journal 发布条目尚未预备目标: {}",
                entry.expected.path
            );
        }
        if entry.rollback_complete && !entry.publish_attempted {
            bail!(
                "restore journal 回滚完成条目从未发布: {}",
                entry.expected.path
            );
        }
    }
    match journal.phase {
        RestorePhase::Preparing if journal.entries.iter().any(|entry| entry.publish_attempted) => {
            bail!("restore journal preparing 阶段含 publish_attempted 条目");
        }
        RestorePhase::Committed
            if journal
                .entries
                .iter()
                .any(|entry| !entry.publish_attempted || entry.rollback_complete) =>
        {
            bail!("restore journal committed 阶段条目状态不完整");
        }
        _ => {}
    }

    let entry_paths = journal
        .entries
        .iter()
        .map(|entry| manifest_relative_path(&entry.expected.path))
        .collect::<Result<Vec<_>>>()?;
    let mut directories = HashSet::new();
    for directory in &journal.created_directories {
        let relative = manifest_relative_path(&directory.path)?;
        if manifest_path_string(&relative)? != directory.path {
            bail!("restore journal 新建目录路径未规范化: {}", directory.path);
        }
        if !directories.insert(normalized_path_key(&relative)?) {
            bail!("restore journal 包含重复新建目录: {}", directory.path);
        }
        if !entry_paths
            .iter()
            .any(|entry| entry != &relative && entry.starts_with(&relative))
        {
            bail!(
                "restore journal 新建目录不是任何恢复目标的祖先: {}",
                directory.path
            );
        }
    }
    Ok(())
}

fn restore_journal_workspace_matches(root: &Path, recorded: &str) -> Result<bool> {
    let recorded = Path::new(recorded);
    if !recorded.is_absolute() {
        return Ok(false);
    }
    let recorded = recorded
        .canonicalize()
        .with_context(|| format!("无法规范化工作区根: {}", display_path(recorded)))?;
    let current = root
        .canonicalize()
        .with_context(|| format!("无法规范化工作区根: {}", display_path(root)))?;

    #[cfg(windows)]
    {
        Ok(display_path(&recorded).eq_ignore_ascii_case(&display_path(&current)))
    }

    #[cfg(not(windows))]
    {
        Ok(recorded == current)
    }
}

fn remove_restore_transaction(ws_dir: &Path, transaction: &Path) -> Result<()> {
    ensure_real_directory(ws_dir)?;
    let name = transaction
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    if transaction.parent() != Some(ws_dir) || !name.starts_with(RESTORE_TRANSACTION_PREFIX) {
        bail!(
            "拒绝清理不属于工作区 checkpoint 的 transaction: {}",
            display_path(transaction)
        );
    }
    ensure_real_directory(transaction)?;
    fs::remove_dir_all(transaction).with_context(|| {
        format!(
            "无法清理 restore transaction: {}",
            display_path(transaction)
        )
    })?;
    sync_directory(ws_dir)
}

pub(super) fn manifest_relative_path(text: &str) -> Result<PathBuf> {
    let relative = PathBuf::from(text);
    if relative.as_os_str().is_empty()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        bail!("checkpoint manifest 含不安全相对路径: {text}");
    }
    Ok(relative)
}

/// The manifest form of a relative path: forward-slash separated, **original
/// case preserved**.
///
/// This is the only string a snapshot may record, because restore turns it back
/// into a real on-disk path (`root.join(relative)`).  Recording a case-folded
/// key here would recreate a deleted `README.md` as `readme.md` on Windows —
/// invisible locally thanks to `core.ignorecase`, but a corrupted tree for every
/// cross-platform clone and CI run.  Components are joined rather than
/// `\`-replaced so a Unix file name that legitimately contains a backslash is
/// not silently split into two components.
pub(super) fn manifest_path_string(relative: &Path) -> Result<String> {
    let mut parts = Vec::new();
    for component in relative.components() {
        let Component::Normal(name) = component else {
            bail!("checkpoint 路径含不安全组件: {}", relative.display());
        };
        parts.push(name.to_str().ok_or_else(|| {
            anyhow::anyhow!("checkpoint 路径不是有效 UTF-8: {}", relative.display())
        })?);
    }
    if parts.is_empty() {
        bail!("checkpoint 路径为空");
    }
    Ok(parts.join("/"))
}

/// Case-folding key for **comparison and de-duplication only**.
///
/// Never persist this and never join it onto a root: on a case-insensitive
/// filesystem two manifest entries differing only in case denote the same file,
/// but the bytes that must land on disk are the ones in
/// [`manifest_path_string`].
pub(super) fn normalized_path_key(relative: &Path) -> Result<String> {
    let text = manifest_path_string(relative)?;
    #[cfg(windows)]
    {
        Ok(text.to_ascii_lowercase())
    }
    #[cfg(not(windows))]
    {
        Ok(text)
    }
}

pub(super) fn integrity_for_file(root: &Path, relative: &Path) -> Result<FileIntegrity> {
    ensure_safe_file_under(root, relative)?;
    let path = root.join(relative);
    let before = fs::symlink_metadata(&path)
        .with_context(|| format!("无法读取文件元数据: {}", display_path(&path)))?;
    if is_link_or_reparse(&before) || !before.file_type().is_file() {
        bail!("拒绝非普通文件: {}", display_path(&path));
    }
    let before_permissions = permission_integrity(&before);
    let sha256 = hash::sha256_file(&path)?;
    let after = fs::symlink_metadata(&path)
        .with_context(|| format!("无法复查文件元数据: {}", display_path(&path)))?;
    let after_permissions = permission_integrity(&after);
    if is_link_or_reparse(&after)
        || !after.file_type().is_file()
        || after.len() != before.len()
        || after_permissions != before_permissions
    {
        bail!("文件在读取期间变更或变为链接: {}", display_path(&path));
    }
    Ok(FileIntegrity {
        path: manifest_path_string(relative)?,
        size: after.len(),
        sha256,
        readonly: Some(after_permissions.0),
        unix_mode: after_permissions.1,
    })
}

fn permission_integrity(metadata: &fs::Metadata) -> (bool, Option<u32>) {
    let readonly = metadata.permissions().readonly();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        (readonly, Some(metadata.permissions().mode() & 0o7777))
    }
    #[cfg(not(unix))]
    {
        (readonly, None)
    }
}

pub(super) fn validate_integrity_record(record: &FileIntegrity) -> Result<()> {
    if !is_valid_sha256(&record.sha256) {
        bail!("完整性记录含无效 SHA-256: {}", record.path);
    }
    if record.readonly.is_none() {
        bail!(
            "checkpoint 完整性记录缺少 readonly 权限证明（旧 content-only manifest），请新建 v3 checkpoint: {}",
            record.path
        );
    }
    #[cfg(unix)]
    if record.unix_mode.is_none() {
        bail!(
            "checkpoint 完整性记录缺少 Unix mode 权限证明，不能在 Unix 安全恢复: {}",
            record.path
        );
    }
    if record.unix_mode.is_some_and(|mode| mode > 0o7777) {
        bail!("checkpoint 完整性记录含无效 Unix mode: {}", record.path);
    }
    Ok(())
}

fn prepare_restore_destination_file(
    root: &Path,
    relative: &Path,
    transaction: &mut RestoreTransaction,
) -> Result<PathBuf> {
    ensure_real_directory(root)?;
    let components: Vec<_> = relative.components().collect();
    if components.is_empty()
        || components
            .iter()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        bail!("不安全的 checkpoint 目标路径: {}", relative.display());
    }
    let mut current = root.to_path_buf();
    for (index, component) in components.iter().enumerate() {
        let Component::Normal(name) = component else {
            bail!("不安全的 checkpoint 目标路径: {}", relative.display());
        };
        current.push(name);
        if index + 1 == components.len() {
            match fs::symlink_metadata(&current) {
                Ok(metadata) if is_link_or_reparse(&metadata) => {
                    bail!("拒绝链接/reparse 恢复目标: {}", display_path(&current));
                }
                Ok(metadata) if metadata.file_type().is_dir() => {
                    bail!("恢复目标是目录而非文件: {}", display_path(&current));
                }
                Ok(metadata) if !metadata.file_type().is_file() => {
                    bail!("恢复目标不是普通文件: {}", display_path(&current));
                }
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(error)
                        .with_context(|| format!("无法读取恢复目标: {}", display_path(&current)));
                }
            }
            continue;
        }

        match fs::symlink_metadata(&current) {
            Ok(_) => ensure_real_directory(&current)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let directory_relative = current.strip_prefix(root).map_err(|_| {
                    anyhow::anyhow!("restore 新建目录逃逸工作区: {}", display_path(&current))
                })?;
                let key = manifest_path_string(directory_relative)?;
                transaction
                    .journal
                    .created_directories
                    .push(RestoreCreatedDirectory {
                        path: key.clone(),
                        state: RestoreDirectoryState::Planned,
                    });
                persist_restore_journal(transaction)?;
                match fs::create_dir(&current) {
                    Ok(()) => {
                        ensure_real_directory(&current)?;
                        sync_directory(current.parent().unwrap_or(root))?;
                        let created = transaction
                            .journal
                            .created_directories
                            .iter_mut()
                            .rev()
                            .find(|directory| directory.path == key)
                            .ok_or_else(|| anyhow::anyhow!("restore 新建目录 journal 条目丢失"))?;
                        created.state = RestoreDirectoryState::Created;
                        persist_restore_journal(transaction)?;
                    }
                    Err(create_error) if create_error.kind() == io::ErrorKind::AlreadyExists => {
                        transaction
                            .journal
                            .created_directories
                            .retain(|directory| directory.path != key);
                        persist_restore_journal(transaction)?;
                        ensure_real_directory(&current)?;
                    }
                    Err(create_error) => {
                        return Err(create_error).with_context(|| {
                            format!("无法创建恢复目标目录: {}", display_path(&current))
                        });
                    }
                }
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("无法读取恢复目标目录: {}", display_path(&current)));
            }
        }
    }
    Ok(current)
}

pub(super) fn prepare_destination_file(root: &Path, relative: &Path) -> Result<PathBuf> {
    prepare_destination_file_inner(root, relative)
}

fn prepare_destination_file_inner(root: &Path, relative: &Path) -> Result<PathBuf> {
    ensure_real_directory(root)?;
    let components: Vec<_> = relative.components().collect();
    if components.is_empty()
        || components
            .iter()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        bail!("不安全的 checkpoint 目标路径: {}", relative.display());
    }
    let mut current = root.to_path_buf();
    for (index, component) in components.iter().enumerate() {
        let Component::Normal(name) = component else {
            bail!("不安全的 checkpoint 目标路径: {}", relative.display());
        };
        current.push(name);
        if index + 1 == components.len() {
            match fs::symlink_metadata(&current) {
                Ok(metadata) if is_link_or_reparse(&metadata) => {
                    bail!("拒绝链接/reparse 恢复目标: {}", display_path(&current));
                }
                Ok(metadata) if metadata.file_type().is_dir() => {
                    bail!("恢复目标是目录而非文件: {}", display_path(&current));
                }
                Ok(metadata) if !metadata.file_type().is_file() => {
                    bail!("恢复目标不是普通文件: {}", display_path(&current));
                }
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(error)
                        .with_context(|| format!("无法读取恢复目标: {}", display_path(&current)));
                }
            }
        } else {
            ensure_or_create_real_directory(&current)?;
        }
    }
    Ok(current)
}

#[cfg(windows)]
#[allow(clippy::permissions_set_readonly_false)] // Windows toggles a file attribute, not Unix mode bits.
fn prepare_atomic_replace_target(path: &Path) -> Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("无法读取原子替换目标权限: {}", display_path(path)));
        }
    };
    if is_link_or_reparse(&metadata) || !metadata.file_type().is_file() {
        bail!("原子替换目标不是安全普通文件: {}", display_path(path));
    }
    let mut permissions = metadata.permissions();
    if permissions.readonly() {
        permissions.set_readonly(false);
        fs::set_permissions(path, permissions)
            .with_context(|| format!("无法临时解除原子替换目标只读属性: {}", display_path(path)))?;
    }
    Ok(())
}

#[cfg(not(windows))]
fn prepare_atomic_replace_target(_path: &Path) -> Result<()> {
    Ok(())
}

fn ensure_or_create_real_directory(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => {
            ensure_real_directory(path)?;
            Ok(false)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let created = match fs::create_dir(path) {
                Ok(()) => true,
                Err(create_error) if create_error.kind() == io::ErrorKind::AlreadyExists => false,
                Err(create_error) => {
                    return Err(create_error)
                        .with_context(|| format!("无法创建恢复目标目录: {}", display_path(path)));
                }
            };
            ensure_real_directory(path)?;
            Ok(created)
        }
        Err(error) => {
            Err(error).with_context(|| format!("无法读取恢复目标目录: {}", display_path(path)))
        }
    }
}

pub(super) fn ensure_safe_file_under(root: &Path, relative: &Path) -> Result<()> {
    ensure_real_directory(root)?;
    let components: Vec<_> = relative.components().collect();
    if components.is_empty()
        || components
            .iter()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        bail!("不安全的 checkpoint 相对路径: {}", relative.display());
    }
    let mut current = root.to_path_buf();
    for (index, component) in components.iter().enumerate() {
        let Component::Normal(name) = component else {
            bail!("不安全的 checkpoint 相对路径: {}", relative.display());
        };
        current.push(name);
        let metadata = fs::symlink_metadata(&current)
            .with_context(|| format!("无法读取 checkpoint 路径: {}", display_path(&current)))?;
        if is_link_or_reparse(&metadata) {
            bail!("拒绝链接/reparse 路径: {}", display_path(&current));
        }
        if index + 1 == components.len() {
            if !metadata.file_type().is_file() {
                bail!("checkpoint 路径不是普通文件: {}", display_path(&current));
            }
        } else if !metadata.file_type().is_dir() {
            bail!("checkpoint 路径组件不是目录: {}", display_path(&current));
        }
    }
    Ok(())
}

pub(super) fn ensure_real_directory(path: &Path) -> Result<()> {
    crate::file_io::ensure_real_directory_labeled(path, "目录")
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<()> {
    fs::File::open(path)
        .and_then(|directory| directory.sync_all())
        .with_context(|| format!("无法同步目录: {}", display_path(path)))
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> Result<()> {
    Ok(())
}

/// Reject symlink/reparse components in a checkpoint root's existing ancestor
/// chain.  The final directory check alone would still allow a symlinked parent
/// to redirect checkpoint writes outside the requested root.
pub(super) fn ensure_real_directory_chain(path: &Path) -> Result<()> {
    let mut current = Some(path);
    while let Some(candidate) = current {
        match fs::symlink_metadata(candidate) {
            Ok(metadata) if is_link_or_reparse(&metadata) => {
                bail!(
                    "拒绝链接/reparse checkpoint 路径组件: {}",
                    display_path(candidate)
                );
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("无法读取 checkpoint 路径组件: {}", display_path(candidate))
                });
            }
        }
        current = candidate.parent();
    }
    Ok(())
}

pub(super) fn is_valid_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}
