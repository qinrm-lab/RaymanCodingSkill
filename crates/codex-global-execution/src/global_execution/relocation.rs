//! Explicit owner confirmation of a copied disk's physical identities.
//! Logical registrations, policy digests, ledgers and checkpoint rows stay intact.
use super::*;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct DirectoryBinding {
    path: PathBuf,
    identity: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct RebindIntent {
    schema: String,
    installation_id: String,
    worktree_id: String,
    registration_sha256: String,
    enrollment_sha256: String,
    previous_sha256: Option<String>,
    source_sha256: String,
    workspace: DirectoryBinding,
    git: Option<DirectoryBinding>,
    common: Option<DirectoryBinding>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RebindRecord {
    intent: RebindIntent,
    authorization_sha256: String,
}

fn same_path(a: &Path, b: &Path) -> bool {
    crate::pathfmt::display_path(a)
        .replace('/', "\\")
        .eq_ignore_ascii_case(&crate::pathfmt::display_path(b).replace('/', "\\"))
}

fn name(id: &str) -> Result<String> {
    if !is_id(id) {
        bail!("invalid physical rebind identity");
    }
    Ok(format!("physical-rebind-{id}.json"))
}

fn intent_digest(intent: &RebindIntent) -> Result<String> {
    Ok(crate::hash::sha256_bytes(&serde_json::to_vec(intent)?))
}

fn load(
    root: &ProtectedDirectory,
    registration: &Registration,
    workspace: &Path,
) -> Result<Option<RebindRecord>> {
    let leaf = name(&registration.worktree_id)?;
    if !root.path().join(&leaf).try_exists()? {
        return Ok(None);
    }
    let record: RebindRecord = decode_json(&root.read_file(&leaf, MAX_REQUEST_BYTES as u64)?)?;
    let install: Installation =
        decode_json(&root.read_file("installation.json", MAX_REQUEST_BYTES as u64)?)?;
    install.validate()?;
    let old_bytes = root.read_file(
        &format!("project-{}.json", registration.worktree_id),
        MAX_REQUEST_BYTES as u64,
    )?;
    let enrollment: Enrollment = decode_json(&old_bytes)?;
    enrollment.validate(&install)?;
    let intent = &record.intent;
    if intent.schema != "rayman.physical-rebind.v1"
        || intent.installation_id != install.installation_id
        || install.root_identity != root.identity()
        || intent.worktree_id != registration.worktree_id
        || intent.registration_sha256 != registration.digest()?
        || enrollment.registration.digest()? != registration.digest()?
        || intent.enrollment_sha256 != crate::hash::sha256_bytes(&old_bytes)
        || !same_path(&intent.workspace.path, workspace)
        || !same_path(&enrollment.workspace, workspace)
        || !is_sha256(&intent.workspace.identity)
        || !is_sha256(&intent.source_sha256)
        || intent
            .previous_sha256
            .as_ref()
            .is_some_and(|h| !is_sha256(h))
        || intent_digest(intent)? != record.authorization_sha256
    {
        bail!("physical rebind does not match the original protected registration");
    }
    match (&enrollment.git, &intent.git, &intent.common) {
        (None, None, None) => {}
        (Some(git), Some(private), Some(common))
            if same_path(&git.git_directory, &private.path)
                && same_path(&git.common_directory, &common.path)
                && is_sha256(&private.identity)
                && is_sha256(&common.identity) => {}
        _ => bail!("physical rebind Git paths differ from the original registration"),
    }
    Ok(Some(record))
}

pub(super) fn check_root(
    root: &ProtectedDirectory,
    registration: &Registration,
    source: &native::SourceDirectory,
) -> Result<()> {
    let binding = load(root, registration, source.path())?;
    let expected = binding
        .as_ref()
        .map(|r| r.intent.workspace.identity.as_str())
        .unwrap_or(&registration.root_identity);
    if source.identity() != expected {
        bail!("registered workspace was replaced");
    }
    Ok(())
}

pub(super) fn check_git(
    root: &ProtectedDirectory,
    registration: &Registration,
    source: &native::SourceDirectory,
    git: &native::SourceDirectory,
    common: &native::SourceDirectory,
) -> Result<()> {
    check_root(root, registration, source)?;
    if let Some(record) = load(root, registration, source.path())? {
        let private = record
            .intent
            .git
            .ok_or_else(|| anyhow::anyhow!("rebind lacks Git identity"))?;
        let shared = record
            .intent
            .common
            .ok_or_else(|| anyhow::anyhow!("rebind lacks common Git identity"))?;
        if !same_path(git.path(), &private.path)
            || git.identity() != private.identity
            || !same_path(common.path(), &shared.path)
            || common.identity() != shared.identity
        {
            bail!("rebound Git directory was replaced");
        }
    } else if common.identity() != registration.git_common_identity
        && logical_common_identity(root, common)? != registration.git_common_identity
    {
        bail!("registered Git common directory was replaced");
    }
    Ok(())
}

fn registrations(root: &ProtectedDirectory) -> Result<Vec<Enrollment>> {
    let install: Installation =
        decode_json(&root.read_file("installation.json", MAX_REQUEST_BYTES as u64)?)?;
    install.validate()?;
    let mut values = Vec::new();
    for entry in std::fs::read_dir(root.path())? {
        let leaf = entry?.file_name().to_string_lossy().into_owned();
        let Some(id) = leaf
            .strip_prefix("project-")
            .and_then(|s| s.strip_suffix(".json"))
        else {
            continue;
        };
        if !is_id(id) || values.len() >= 1024 {
            bail!("invalid or excessive protected registrations");
        }
        let value: Enrollment = decode_json(&root.read_file(&leaf, MAX_REQUEST_BYTES as u64)?)?;
        value.validate(&install)?;
        if value.registration.worktree_id != id {
            bail!("registration filename differs");
        }
        values.push(value);
    }
    Ok(values)
}

pub(super) fn rebound_enrollment(
    root: &ProtectedDirectory,
    workspace: &Path,
) -> Result<Option<Enrollment>> {
    let mut found = None;
    for enrollment in registrations(root)? {
        if !same_path(&enrollment.workspace, workspace) {
            continue;
        }
        if found.is_some() {
            bail!("ambiguous existing workspace registrations");
        }
        found = Some(enrollment);
    }
    let Some(enrollment) = found else {
        return Ok(None);
    };
    if load(root, &enrollment.registration, workspace)?.is_none() {
        return Ok(None);
    }
    let source = native::SourceDirectory::open(workspace)?;
    check_root(root, &enrollment.registration, &source)?;
    if let Some(binding) = &enrollment.git {
        let private = native::SourceDirectory::open(&binding.git_directory)?;
        let common = native::SourceDirectory::open(&binding.common_directory)?;
        check_git(root, &enrollment.registration, &source, &private, &common)?;
    }
    Ok(Some(enrollment))
}

pub(super) fn logical_common_identity(
    root: &ProtectedDirectory,
    common: &native::SourceDirectory,
) -> Result<String> {
    let mut logical = None;
    for enrollment in registrations(root)? {
        let Some(git) = &enrollment.git else {
            continue;
        };
        if !same_path(&git.common_directory, common.path()) {
            continue;
        }
        let Some(record) = load(root, &enrollment.registration, &enrollment.workspace)? else {
            continue;
        };
        let expected = record
            .intent
            .common
            .ok_or_else(|| anyhow::anyhow!("rebind common identity missing"))?;
        if expected.identity != common.identity() {
            bail!("rebound common Git directory was replaced");
        }
        if logical
            .as_ref()
            .is_some_and(|id| id != &enrollment.registration.git_common_identity)
        {
            bail!("ambiguous logical identity for the copied Git repository");
        }
        logical = Some(enrollment.registration.git_common_identity);
    }
    Ok(logical.unwrap_or_else(|| common.identity().into()))
}

fn observe_git(
    enrollment: &Enrollment,
    source: &native::SourceDirectory,
) -> Result<(
    Option<DirectoryBinding>,
    Option<DirectoryBinding>,
    Vec<native::SourceDirectory>,
)> {
    let marker = source.path().join(".git");
    let Some(binding) = &enrollment.git else {
        return Ok((None, None, Vec::new()));
    };
    let metadata = std::fs::symlink_metadata(&marker)?;
    if crate::file_io::is_link_or_reparse(&metadata) {
        bail!("copied Git marker is a reparse point");
    }
    let actual_git = if metadata.is_dir() {
        std::fs::canonicalize(&marker)?
    } else {
        let bytes = source.read_file(Path::new(".git"), 4096)?;
        let text = std::str::from_utf8(&bytes)?
            .trim()
            .strip_prefix("gitdir: ")
            .ok_or_else(|| anyhow::anyhow!("invalid copied Git marker"))?;
        std::fs::canonicalize(source.path().join(text))?
    };
    if !same_path(&actual_git, &binding.git_directory) {
        bail!("copied private Git path changed");
    }
    let git = native::SourceDirectory::open(&binding.git_directory)?;
    let common_path = if git.path().join("commondir").try_exists()? {
        let bytes = git.read_file(Path::new("commondir"), 4096)?;
        std::fs::canonicalize(git.path().join(std::str::from_utf8(&bytes)?.trim()))?
    } else {
        git.path().to_path_buf()
    };
    if !same_path(&common_path, &binding.common_directory) {
        bail!("copied common Git path changed");
    }
    let common = native::SourceDirectory::open(&binding.common_directory)?;
    if metadata.is_file() {
        if git.path().parent() != Some(common.path().join("worktrees").as_path()) {
            bail!("copied linked worktree is outside its common Git directory");
        }
        let bytes = git.read_file(Path::new("gitdir"), 4096)?;
        if std::fs::canonicalize(std::str::from_utf8(&bytes)?.trim())?
            != std::fs::canonicalize(&marker)?
        {
            bail!("copied worktree backlink changed");
        }
    }
    let head = git.read_file(Path::new("HEAD"), 4096)?;
    let head = std::str::from_utf8(&head)?.trim();
    if (binding.branch_ref == "HEAD" && !is_hex(head, enrollment.registration.object_id_length))
        || (binding.branch_ref != "HEAD"
            && head.strip_prefix("ref: ") != Some(binding.branch_ref.as_str()))
        || crate::hash::sha256_bytes(&common.read_file(Path::new("config"), 1024 * 1024)?)
            != binding.config_sha256
        || crate::hash::sha256_file(&binding.executable)? != binding.executable_sha256
    {
        bail!(
            "copied Git branch, configuration or executable differs; not an identity-only migration"
        );
    }
    let private = DirectoryBinding {
        path: git.path().into(),
        identity: git.identity().into(),
    };
    let shared = DirectoryBinding {
        path: common.path().into(),
        identity: common.identity().into(),
    };
    Ok((Some(private), Some(shared), vec![git, common]))
}

fn require_paused_worker(root: &ProtectedDirectory, install: &Installation) -> Result<()> {
    let marker: serde_json::Value =
        decode_json(&root.read_file("kernel-maintenance.json", MAX_REQUEST_BYTES as u64)?)?;
    let heartbeat: serde_json::Value =
        decode_json(&root.read_file("heartbeat.json", MAX_REQUEST_BYTES as u64)?)?;
    let observed = heartbeat["observed_at"]
        .as_i64()
        .ok_or_else(|| anyhow::anyhow!("worker heartbeat clock missing"))?;
    let age = chrono::Utc::now().timestamp() - observed;
    if marker["schema"] != "rayman.global-kernel-maintenance.v1"
        || marker["installation_id"] != install.installation_id
        || !marker["worker_sha256"]
            .as_array()
            .is_some_and(|v| v.iter().any(|h| h == &install.worker_sha256))
        || heartbeat["installation_id"] != install.installation_id
        || heartbeat["executor_sid"] != install.owner_sid
        || heartbeat["maintenance"] != true
        || heartbeat["activity"]["stage"] != "maintenance"
        || !heartbeat["activity"]["request_id"].is_null()
        || !heartbeat["error"].is_null()
        || !(0..=15).contains(&age)
    {
        bail!("physical rebinding requires fresh worker acknowledgement of kernel maintenance");
    }
    if std::fs::read_dir(root.path().join("requests"))?.any(|e| {
        e.map(|entry| entry.file_name().to_string_lossy().ends_with(".json"))
            .unwrap_or(true)
    }) {
        bail!("preserve queued requests; physical rebinding requires an empty paused queue");
    }
    if super::install::pending_request(root)?.is_some() {
        bail!("complete pending installation recovery before physical rebinding");
    }
    Ok(())
}

pub fn rebind_workspace(
    root: &Path,
    workspace: &Path,
    expected_sha256: Option<&str>,
    publish: bool,
) -> Result<serde_json::Value> {
    let client = Client::open(root)?;
    let status = client.status()?;
    let owner = status["owner_sid"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("installation owner missing"))?;
    let root = ProtectedDirectory::open(root, owner)?;
    let install: Installation =
        decode_json(&root.read_file("installation.json", MAX_REQUEST_BYTES as u64)?)?;
    if publish {
        native::require_unelevated_worker()?;
        if crate::execution_context::execution_context_probe()
            .principal_sid
            .as_deref()
            != Some(owner)
        {
            bail!("physical rebinding requires the registered desktop owner");
        }
        #[cfg(not(test))]
        if std::fs::canonicalize(std::env::current_exe()?)? != root.path().join("client.exe") {
            bail!("publish physical bindings only through the installed protected client");
        }
    }
    let _lock = if publish {
        Some(crate::state_lock::acquire_state_lock(
            &root.path().join("registry"),
        )?)
    } else {
        None
    };
    let source = native::SourceDirectory::open(workspace)?;
    let matches: Vec<_> = registrations(&root)?
        .into_iter()
        .filter(|e| same_path(&e.workspace, source.path()))
        .collect();
    if matches.len() != 1 {
        bail!("physical rebinding requires exactly one original registration at this path");
    }
    let enrollment = &matches[0];
    let registration = &enrollment.registration;
    let leaf = name(&registration.worktree_id)?;
    let previous = if root.path().join(&leaf).try_exists()? {
        Some(root.read_file(&leaf, MAX_REQUEST_BYTES as u64)?)
    } else {
        None
    };
    let existing = load(&root, registration, source.path())?;
    let (git, common, _pins) = observe_git(enrollment, &source)?;
    if let Some(record) = &existing
        && record.intent.workspace.identity == source.identity()
        && record.intent.git == git
        && record.intent.common == common
    {
        if publish && expected_sha256 != Some(record.authorization_sha256.as_str()) {
            bail!("expected physical binding authorization differs");
        }
        return Ok(
            serde_json::json!({"preview":!publish,"applied":publish,"already_current":true,
                "authorization_sha256":record.authorization_sha256,"intent":record.intent,"history_rewritten":false}),
        );
    }
    let old_bytes = root.read_file(
        &format!("project-{}.json", registration.worktree_id),
        MAX_REQUEST_BYTES as u64,
    )?;
    let intent = RebindIntent {
        schema: "rayman.physical-rebind.v1".into(),
        installation_id: install.installation_id.clone(),
        worktree_id: registration.worktree_id.clone(),
        registration_sha256: registration.digest()?,
        enrollment_sha256: crate::hash::sha256_bytes(&old_bytes),
        previous_sha256: previous.as_ref().map(|b| crate::hash::sha256_bytes(b)),
        source_sha256: crate::source_fingerprint(source.path())?,
        workspace: DirectoryBinding {
            path: source.path().into(),
            identity: source.identity().into(),
        },
        git,
        common,
    };
    let record = RebindRecord {
        authorization_sha256: intent_digest(&intent)?,
        intent,
    };
    if publish {
        if expected_sha256 != Some(record.authorization_sha256.as_str()) {
            bail!("physical rebind preview changed; review a fresh plan");
        }
        require_paused_worker(&root, &install)?;
        if crate::source_fingerprint(source.path())? != record.intent.source_sha256 {
            bail!("source changed while confirming physical rebind");
        }
        let mut nonce = [0u8; 16];
        getrandom::fill(&mut nonce).map_err(|e| anyhow::anyhow!("rebind entropy failed: {e}"))?;
        let seed = format!(
            "physical-rebind-{}.tmp",
            nonce.iter().map(|v| format!("{v:02x}")).collect::<String>()
        );
        let bytes = serde_json::to_vec_pretty(&record)?;
        let mut stage = publication::PublicationSlot::create(root.path(), &seed, &bytes)?;
        if previous.is_some() {
            stage.preserve_access_from(&root.path().join(&leaf))?;
        }
        stage.publish_reviewed(&leaf, previous.as_deref())?;
        drop(stage);
        if root.read_file(&leaf, MAX_REQUEST_BYTES as u64)? != bytes {
            bail!("physical binding readback differs; leave maintenance active");
        }
        check_root(&root, registration, &source)?;
    }
    Ok(
        serde_json::json!({"preview":!publish,"applied":publish,"already_current":false,
        "authorization_sha256":record.authorization_sha256,"intent":record.intent,
        "history_rewritten":false,"capabilities_changed":false,"maintenance_required":true,
        "maintenance_released":false,"runtime_validation_required":true}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn copy_fixture_tree(from: &Path, to: &Path) {
        std::fs::create_dir(to).unwrap();
        for item in std::fs::read_dir(from).unwrap() {
            let item = item.unwrap();
            if item.file_type().unwrap().is_dir() {
                copy_fixture_tree(&item.path(), &to.join(item.file_name()));
            } else {
                std::fs::copy(item.path(), to.join(item.file_name())).unwrap();
            }
        }
    }

    #[test]
    fn disk_copy_rebinding_preserves_registration_and_rejects_stale_or_replaced_sources() {
        check_disk_copy(false);
        check_disk_copy(true);
    }

    fn check_disk_copy(with_git: bool) {
        let temp = tempfile::tempdir().unwrap();
        let backend = temp.path().join("backend");
        let workspace = temp.path().join("workspace");
        std::fs::create_dir(&backend).unwrap();
        std::fs::create_dir(&workspace).unwrap();
        std::fs::create_dir(backend.join("requests")).unwrap();
        std::fs::write(workspace.join("data.txt"), b"kept bytes\r\n").unwrap();
        let sid = crate::execution_context::execution_context_probe()
            .principal_sid
            .unwrap();
        native::protect_fixture_directory(&backend, &sid, "").unwrap();
        let root = ProtectedDirectory::open(&backend, &sid).unwrap();
        std::fs::write(backend.join("worker.exe"), b"worker").unwrap();
        std::fs::write(backend.join("client.exe"), b"client").unwrap();
        let install = Installation {
            schema_version: 1,
            installation_id: "a".repeat(32),
            owner_sid: sid.clone(),
            root_identity: root.identity().into(),
            worker_sha256: crate::hash::sha256_bytes(b"worker"),
            client_sha256: crate::hash::sha256_bytes(b"client"),
            source_project_id: "b".repeat(32),
        };
        crate::file_io::write_json(&backend.join("installation.json"), &install).unwrap();
        let program = Path::new("C:/Program Files/Git/mingw64/bin/git.exe");
        if with_git {
            for args in [
                vec!["init", "-b", "main"],
                vec!["config", "user.name", "Fixture"],
                vec!["config", "user.email", "fixture@example.invalid"],
                vec!["add", "data.txt"],
                vec!["-c", "commit.gpgsign=false", "commit", "-m", "baseline"],
            ] {
                let output = std::process::Command::new(program)
                    .current_dir(&workspace)
                    .args(args)
                    .output()
                    .unwrap();
                assert!(
                    output.status.success(),
                    "{}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
        }
        let commit = with_git.then_some((program, "Fixture", "fixture@example.invalid"));
        let old = enroll(&backend, &workspace, commit, true, true).unwrap();
        let registration_path =
            backend.join(format!("project-{}.json", old.registration.worktree_id));
        let original = std::fs::read(&registration_path).unwrap();
        let checkpoint = backend.join(format!(
            "checkpoint-{}.sqlite3",
            old.registration.worktree_id
        ));
        std::fs::write(&checkpoint, b"unchanged checkpoint history").unwrap();
        let storage = StateStorage::at_existing_root(&backend, &old.registration).unwrap();
        let mut request = super::super::tests::request(&old.registration);
        request.operation = Operation::State {
            task_id: "d".repeat(32),
            expected_revision: 0,
            mutation: StateMutation::Begin {
                title: "existing task".into(),
            },
        };
        storage.accept(&request, &old.registration, 1000).unwrap();
        let ledger = backend.join(format!("ledger-{}.json", old.registration.worktree_id));
        let history = std::fs::read(&ledger).unwrap();
        std::fs::rename(&workspace, temp.path().join("old-disk")).unwrap();
        copy_fixture_tree(&temp.path().join("old-disk"), &workspace);
        assert!(
            Client::open(&backend)
                .unwrap()
                .enrollment(&workspace)
                .is_err()
        );
        let preview = rebind_workspace(&backend, &workspace, None, false).unwrap();
        let digest = preview["authorization_sha256"].as_str().unwrap();
        assert!(
            !backend
                .join(name(&old.registration.worktree_id).unwrap())
                .exists()
        );
        assert!(rebind_workspace(&backend, &workspace, Some(digest), true).is_err());
        let heartbeat = serde_json::json!({"installation_id":install.installation_id,"executor_sid":sid,
            "observed_at":chrono::Utc::now().timestamp(),"maintenance":true,"error":null,
            "activity":{"stage":"maintenance","request_id":null}});
        crate::file_io::write_json(&backend.join("heartbeat.json"), &heartbeat).unwrap();
        crate::file_io::write_json(&backend.join("kernel-maintenance.json"), &serde_json::json!({
            "schema":"rayman.global-kernel-maintenance.v1","installation_id":install.installation_id,
            "transaction_id":"c".repeat(32),"worker_sha256":[install.worker_sha256]})).unwrap();
        std::fs::write(workspace.join("data.txt"), b"newer live bytes").unwrap();
        assert!(rebind_workspace(&backend, &workspace, Some(digest), true).is_err());
        let preview = rebind_workspace(&backend, &workspace, None, false).unwrap();
        let digest = preview["authorization_sha256"].as_str().unwrap();
        std::fs::write(backend.join("requests/queued.json"), b"retained request").unwrap();
        assert!(rebind_workspace(&backend, &workspace, Some(digest), true).is_err());
        std::fs::remove_file(backend.join("requests/queued.json")).unwrap();
        rebind_workspace(&backend, &workspace, Some(digest), true).unwrap();
        let rebound = Client::open(&backend)
            .unwrap()
            .enrollment(&workspace)
            .unwrap();
        assert_eq!(
            rebound.registration.digest().unwrap(),
            old.registration.digest().unwrap()
        );
        assert_eq!(
            enroll(&backend, &workspace, commit, true, true)
                .unwrap()
                .registration
                .digest()
                .unwrap(),
            old.registration.digest().unwrap()
        );
        assert_eq!(std::fs::read(&registration_path).unwrap(), original);
        assert_eq!(std::fs::read(&ledger).unwrap(), history);
        let physical_path = backend.join(name(&old.registration.worktree_id).unwrap());
        let physical_bytes = std::fs::read(&physical_path).unwrap();
        let mut forged: serde_json::Value = serde_json::from_slice(&physical_bytes).unwrap();
        forged["intent"]["registration_sha256"] = "0".repeat(64).into();
        crate::file_io::write_json(&physical_path, &forged).unwrap();
        assert!(
            Client::open(&backend)
                .unwrap()
                .enrollment(&workspace)
                .is_err()
        );
        std::fs::write(&physical_path, &physical_bytes).unwrap();
        assert_eq!(
            std::fs::read(&checkpoint).unwrap(),
            b"unchanged checkpoint history"
        );
        assert!(
            storage
                .recorded_result(&request, &old.registration)
                .unwrap()
                .is_some()
        );
        assert!(
            rebind_workspace(&backend, &workspace, Some(digest), true).unwrap()["already_current"]
                == true
        );
        if let Some(binding) = &old.git {
            let inspector =
                GitInspector::open(&workspace, binding, &old.registration, &root).unwrap();
            let snapshot = inspector.capture(&old.registration).unwrap();
            assert!(!snapshot.head.is_empty());
            drop(inspector);
            let common = native::SourceDirectory::open(&binding.common_directory).unwrap();
            assert_eq!(
                logical_common_identity(&root, &common).unwrap(),
                old.registration.git_common_identity
            );
            drop(common);
            let config = workspace.join(".git/config");
            let original_config = std::fs::read(&config).unwrap();
            std::fs::write(&config, b"[core]\nfsmonitor = bad\n").unwrap();
            assert!(rebind_workspace(&backend, &workspace, None, false).is_err());
            std::fs::write(&config, original_config).unwrap();
            let linked = temp.path().join("linked");
            let output = std::process::Command::new(program)
                .current_dir(&workspace)
                .args(["worktree", "add", "--detach"])
                .arg(&linked)
                .arg("HEAD")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let child = enroll(&backend, &linked, commit, true, true).unwrap();
            assert_eq!(child.registration.project_id, old.registration.project_id);
            assert_eq!(
                child.registration.git_common_identity,
                old.registration.git_common_identity
            );
            assert_ne!(child.registration.worktree_id, old.registration.worktree_id);
            let inspector = GitInspector::open(
                &linked,
                child.git.as_ref().unwrap(),
                &child.registration,
                &root,
            )
            .unwrap();
            inspector.capture(&child.registration).unwrap();
            drop(inspector);
        }
        std::fs::rename(&workspace, temp.path().join("second-old-disk")).unwrap();
        copy_fixture_tree(&temp.path().join("second-old-disk"), &workspace);
        assert!(
            Client::open(&backend)
                .unwrap()
                .enrollment(&workspace)
                .is_err()
        );
        assert!(rebind_workspace(&backend, &workspace, Some(digest), true).is_err());
    }
}
