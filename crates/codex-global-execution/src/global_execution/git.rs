//! Exact Git snapshot reader. It runs fixed read-only plumbing inside the
//! single-process native boundary; it never stages, writes an object or a ref.
use super::*;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

#[derive(Debug, Serialize)]
pub struct CommitSnapshot {
    pub head: String,
    pub index_sha256: String,
    pub changes: Vec<Change>,
}

enum ReadCommand<'a> {
    Head,
    Branch,
    Index,
    HeadTree,
    Status,
    Blob(&'a str),
    Filtered(&'a str),
    FilterAttribute(&'a str),
    HashBlob(&'a str),
    HashCommit,
    ReadTree(&'a str),
    IndexInfo,
    WriteTree,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommitCandidate {
    pub request_sha256: String,
    pub registration_sha256: String,
    pub parent: String,
    pub commit: String,
    pub tree: String,
    pub original_index_sha256: String,
    pub candidate_index_sha256: String,
    pub paths: Vec<String>,
    pub preserved_paths: Vec<String>,
    pub objects: BTreeMap<String, String>,
    pub changes_sha256: String,
    pub preserved_changes_sha256: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PublicationRecord {
    request_sha256: String,
    registration_sha256: String,
    index_seed: String,
    index_identity: String,
    index_digest: String,
    ref_seed: String,
    ref_identity: String,
    ref_digest: String,
    phase: String,
}

struct CandidateEnvironment<'a> {
    index: &'a Path,
    objects: &'a Path,
}

pub struct GitInspector<'a> {
    binding: GitBinding,
    workspace: PathBuf,
    isolation: PathBuf,
    _root: super::native::SourceDirectory,
    _git: super::native::SourceDirectory,
    _common: super::native::SourceDirectory,
    _executable: std::fs::File,
    _protected: &'a ProtectedDirectory,
}

impl<'a> GitInspector<'a> {
    pub fn open(
        workspace: &Path,
        binding: &GitBinding,
        registration: &Registration,
        isolation: &'a ProtectedDirectory,
    ) -> Result<Self> {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ,
        };
        if !binding.executable.is_absolute()
            || !is_sha256(&binding.executable_sha256)
            || !is_sha256(&binding.config_sha256)
            || !binding.branch_ref.starts_with("refs/heads/")
            || binding.branch_ref.contains(['\0', '\n', '\r'])
        {
            bail!("invalid fixed Git binding");
        }
        let root = super::native::SourceDirectory::open(workspace)?;
        let git = super::native::SourceDirectory::open(&binding.git_directory)?;
        let common = super::native::SourceDirectory::open(&binding.common_directory)?;
        if root.identity() != registration.root_identity
            || common.identity() != registration.git_common_identity
        {
            bail!("Git project/common directory identity changed");
        }
        let marker = workspace.join(".git");
        let marker_metadata = std::fs::symlink_metadata(&marker)?;
        if crate::file_io::is_link_or_reparse(&marker_metadata) {
            bail!("Git marker cannot be a reparse point");
        }
        let actual_git = if marker_metadata.is_dir() {
            std::fs::canonicalize(&marker)?
        } else {
            let bytes = root.read_file(Path::new(".git"), 4096)?;
            let text = std::str::from_utf8(&bytes)?.trim();
            let target = text
                .strip_prefix("gitdir: ")
                .ok_or_else(|| anyhow::anyhow!("invalid worktree Git marker"))?;
            std::fs::canonicalize(workspace.join(target))?
        };
        if actual_git != git.path() {
            bail!("worktree Git marker no longer names the enrolled Git directory");
        }
        let commondir = binding.git_directory.join("commondir");
        let actual_common = if commondir.try_exists()? {
            let bytes = git.read_file(Path::new("commondir"), 4096)?;
            std::fs::canonicalize(
                binding
                    .git_directory
                    .join(std::str::from_utf8(&bytes)?.trim()),
            )?
        } else {
            git.path().to_path_buf()
        };
        if actual_common != common.path() {
            bail!("Git common directory relationship changed");
        }
        for relative in [
            "MERGE_HEAD",
            "CHERRY_PICK_HEAD",
            "REVERT_HEAD",
            "rebase-apply",
            "rebase-merge",
            "sequencer",
            "BISECT_LOG",
            "shallow",
            "info/sparse-checkout",
            "objects/info/alternates",
            "refs/replace",
        ] {
            if git.path().join(relative).try_exists()?
                || common.path().join(relative).try_exists()?
            {
                bail!(
                    "Git operation, sparse or alternate state requires a separate adapter: {relative}"
                );
            }
        }
        if marker_metadata.is_file() {
            let expected_parent = common.path().join("worktrees");
            if git.path().parent() != Some(expected_parent.as_path()) {
                bail!(
                    "separate Git directories require an explicit adapter; only linked worktrees are supported"
                );
            }
            let backlink = git.read_file(Path::new("gitdir"), 4096)?;
            if std::fs::canonicalize(std::str::from_utf8(&backlink)?.trim())?
                != std::fs::canonicalize(&marker)?
            {
                bail!("linked worktree backlink does not match enrolled workspace");
            }
        }
        if git.path().join("config.worktree").try_exists()? {
            bail!("per-worktree Git configuration requires a reviewed adapter");
        }
        let config = common.read_file(Path::new("config"), 1024 * 1024)?;
        if crate::hash::sha256_bytes(&config) != binding.config_sha256 {
            bail!("registered Git config changed");
        }
        for line in std::str::from_utf8(&config)?.lines() {
            let line = line.trim().to_ascii_lowercase();
            if line.starts_with('#') || line.starts_with(';') {
                continue;
            }
            if line.starts_with("[include") || line.starts_with("[filter") {
                bail!(
                    "Git includes, filters or custom hooks require a separately reviewed adapter"
                );
            }
        }
        let hooks = binding.common_directory.join("hooks");
        if hooks.try_exists()? {
            let pinned = super::native::SourceDirectory::open(&hooks)?;
            for entry in std::fs::read_dir(pinned.path())? {
                let entry = entry?;
                if !entry.file_name().to_string_lossy().ends_with(".sample") {
                    bail!(
                        "active Git hooks require a separately reviewed sandbox validation route"
                    );
                }
            }
        }
        let file = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(&binding.executable)?;
        let metadata = file.metadata()?;
        if !metadata.is_file()
            || crate::file_io::is_link_or_reparse(&metadata)
            || metadata.len() > 128 * 1024 * 1024
        {
            bail!("Git executable is not a bounded ordinary file");
        }
        let bytes = crate::file_io::read_bytes_from_handle(
            &file,
            metadata.len(),
            &binding.executable,
            "registered Git executable",
        )?;
        if crate::hash::sha256_bytes(&bytes) != binding.executable_sha256 {
            bail!("registered Git executable hash changed");
        }
        Ok(Self {
            binding: binding.clone(),
            workspace: root.path().to_path_buf(),
            isolation: isolation.path().to_path_buf(),
            _root: root,
            _git: git,
            _common: common,
            _executable: file,
            _protected: isolation,
        })
    }

    /// The protected desktop-user worker re-reads the effective user/system
    /// content policy immediately before a commit. Sandbox-side previews use
    /// only the policy already sealed into enrollment because that identity
    /// may legitimately be unable to read the desktop user's profile.
    pub(super) fn open_for_worker(
        workspace: &Path,
        binding: &GitBinding,
        registration: &Registration,
        isolation: &'a ProtectedDirectory,
    ) -> Result<Self> {
        let inspector = Self::open(workspace, binding, registration, isolation)?;
        binding
            .content_policy
            .verify(&binding.executable, inspector._root.path())?;
        Ok(inspector)
    }

    fn run(&self, command: ReadCommand<'_>, input: &[u8]) -> Result<Vec<u8>> {
        self.run_in(command, input, None)
    }

    fn run_in(
        &self,
        command: ReadCommand<'_>,
        input: &[u8],
        candidate: Option<&CandidateEnvironment<'_>>,
    ) -> Result<Vec<u8>> {
        let common = [
            "core.hooksPath",
            "core.fsmonitor",
            "commit.gpgSign",
            "tag.gpgSign",
            "maintenance.auto",
            "gc.auto",
            "core.pager",
            "color.ui",
        ];
        let git_isolation = crate::pathfmt::display_path(&self.isolation);
        let values = [
            git_isolation.as_str(),
            "false",
            "false",
            "false",
            "false",
            "0",
            "cat",
            "false",
        ];
        let mut args = Vec::new();
        args.extend([
            "-c".into(),
            format!(
                "safe.directory={}",
                crate::pathfmt::display_path(&self.workspace)
            ),
        ]);
        for (key, value) in common.into_iter().zip(values) {
            args.push("-c".into());
            args.push(format!("{key}={value}"));
        }
        for (key, value) in &self.binding.content_policy.settings {
            args.extend(["-c".into(), format!("{key}={value}")]);
        }
        let specific: Vec<String> = match command {
            ReadCommand::Head => vec!["rev-parse".into(), "--verify".into(), "HEAD".into()],
            ReadCommand::Branch => vec!["symbolic-ref".into(), "-q".into(), "HEAD".into()],
            ReadCommand::Index => vec!["ls-files".into(), "--stage".into(), "-z".into()],
            ReadCommand::HeadTree => vec![
                "ls-tree".into(),
                "-r".into(),
                "-z".into(),
                "--full-tree".into(),
                "HEAD".into(),
            ],
            ReadCommand::Status => vec![
                "status".into(),
                "--porcelain=v1".into(),
                "-z".into(),
                "--untracked-files=all".into(),
                "--ignore-submodules=none".into(),
            ],
            ReadCommand::Blob(oid) => {
                if !is_hex(oid, 40) && !is_hex(oid, 64) {
                    bail!("invalid blob identity");
                }
                vec!["cat-file".into(), "blob".into(), oid.into()]
            }
            ReadCommand::Filtered(path) => {
                validate_relative_path(path)?;
                vec![
                    "hash-object".into(),
                    format!("--path={path}"),
                    "--stdin".into(),
                ]
            }
            ReadCommand::FilterAttribute(path) => {
                validate_relative_path(path)?;
                vec![
                    "check-attr".into(),
                    "-z".into(),
                    "filter".into(),
                    "--".into(),
                    path.into(),
                ]
            }
            ReadCommand::HashBlob(path) => {
                validate_relative_path(path)?;
                if candidate.is_none() {
                    bail!("Git writes require a candidate environment");
                }
                vec![
                    "hash-object".into(),
                    "-w".into(),
                    format!("--path={path}"),
                    "--stdin".into(),
                ]
            }
            ReadCommand::HashCommit => {
                if candidate.is_none() {
                    bail!("Git writes require a candidate environment");
                }
                vec![
                    "hash-object".into(),
                    "-w".into(),
                    "-t".into(),
                    "commit".into(),
                    "--stdin".into(),
                ]
            }
            ReadCommand::ReadTree(oid) => {
                if candidate.is_none() || (!is_hex(oid, 40) && !is_hex(oid, 64)) {
                    bail!("invalid candidate tree initialization");
                }
                vec!["read-tree".into(), oid.into()]
            }
            ReadCommand::IndexInfo => {
                if candidate.is_none() {
                    bail!("Git writes require a candidate environment");
                }
                vec!["update-index".into(), "-z".into(), "--index-info".into()]
            }
            ReadCommand::WriteTree => {
                if candidate.is_none() {
                    bail!("Git writes require a candidate environment");
                }
                vec!["write-tree".into()]
            }
        };
        args.extend(specific);
        let mut env = BTreeMap::new();
        for (k, v) in [
            ("GIT_CONFIG_NOSYSTEM", "1"),
            ("GIT_CONFIG_SYSTEM", "NUL"),
            ("GIT_CONFIG_GLOBAL", "NUL"),
            ("GIT_CONFIG_COUNT", "0"),
            ("GIT_ATTR_NOSYSTEM", "1"),
            ("GIT_TERMINAL_PROMPT", "0"),
            ("GCM_INTERACTIVE", "Never"),
            ("GIT_NO_LAZY_FETCH", "1"),
            ("GIT_NO_REPLACE_OBJECTS", "1"),
            ("GIT_OPTIONAL_LOCKS", "0"),
            ("GIT_DISCOVERY_ACROSS_FILESYSTEM", "0"),
            ("LC_ALL", "C"),
            ("LANG", "C"),
            ("PATH", ""),
        ] {
            env.insert(k.into(), v.into());
        }
        for (key, value) in [
            ("GIT_DIR", &self.binding.git_directory),
            ("GIT_WORK_TREE", &self.workspace),
            ("HOME", &self.isolation),
            ("USERPROFILE", &self.isolation),
            ("TEMP", &self.isolation),
            ("TMP", &self.isolation),
        ] {
            env.insert(key.into(), crate::pathfmt::display_path(value));
        }
        // This is a bound native installation, not an environment-selected Git.
        env.insert("SystemRoot".into(), "C:\\Windows".into());
        if let Some(candidate) = candidate {
            env.insert(
                "GIT_INDEX_FILE".into(),
                crate::pathfmt::display_path(candidate.index),
            );
            env.insert(
                "GIT_OBJECT_DIRECTORY".into(),
                crate::pathfmt::display_path(candidate.objects),
            );
            env.insert(
                "GIT_ALTERNATE_OBJECT_DIRECTORIES".into(),
                crate::pathfmt::display_path(&self.binding.common_directory.join("objects")),
            );
        }
        let out = super::process::run_single_process(
            &self.binding.executable,
            &args,
            &self.isolation,
            &env,
            input,
            30_000,
            32 * 1024 * 1024,
        )?;
        if out.exit_code != 0 {
            bail!(
                "fixed Git read failed ({}): {}",
                out.exit_code,
                String::from_utf8_lossy(&out.stderr)
            );
        }
        Ok(out.stdout)
    }

    pub(super) fn run_hook_preflight(
        &self,
        request: &Request,
        registration: &Registration,
        policy: &GitHookPolicy,
    ) -> Result<HookReceipt> {
        validate_request(request, registration, request.created_at)?;
        let Operation::Commit {
            expected_head,
            changes,
            ..
        } = &request.operation
        else {
            bail!("hook preflight requires a commit request");
        };
        let hook_root = policy.verify(&self.workspace)?;
        let temp = tempfile::Builder::new().prefix("global-hook-").tempdir()?;
        let index = temp.path().join("index");
        let objects = temp.path().join("objects");
        std::fs::create_dir(&objects)?;
        let candidate = CandidateEnvironment {
            index: &index,
            objects: &objects,
        };
        self.run_in(ReadCommand::ReadTree(expected_head), &[], Some(&candidate))?;
        let mut index_info = Vec::new();
        for change in changes {
            match &change.after {
                Some(after) => {
                    let bytes = self
                        ._root
                        .read_file(Path::new(&change.path), 32 * 1024 * 1024)?;
                    if crate::hash::sha256_bytes(&bytes) != after.raw_sha256 {
                        bail!("hook candidate source changed");
                    }
                    let oid = String::from_utf8(self.run_in(
                        ReadCommand::HashBlob(&change.path),
                        &bytes,
                        Some(&candidate),
                    )?)?;
                    let oid = oid.trim();
                    if oid != after.git_blob_oid {
                        bail!("hook candidate filtered blob differs");
                    }
                    index_info
                        .extend(format!("{:o} {oid}\t{}\0", after.mode, change.path).as_bytes());
                }
                None => index_info.extend(
                    format!(
                        "0 {}\t{}\0",
                        "0".repeat(registration.object_id_length),
                        change.path
                    )
                    .as_bytes(),
                ),
            }
        }
        self.run_in(ReadCommand::IndexInfo, &index_info, Some(&candidate))?;
        let candidate_tree_oid =
            String::from_utf8(self.run_in(ReadCommand::WriteTree, &[], Some(&candidate))?)?
                .trim()
                .to_owned();
        if !is_hex(&candidate_tree_oid, registration.object_id_length) {
            bail!("sandbox hook candidate tree identity is invalid");
        }
        let mut environment: BTreeMap<String, String> = std::env::vars()
            .filter(|(key, _)| {
                let upper = key.to_ascii_uppercase();
                !upper.starts_with("GIT_")
                    && !upper.contains("SECRET")
                    && !upper.contains("TOKEN")
                    && !upper.ends_with("_KEY")
            })
            .collect();
        for (key, value) in [
            (
                "GIT_DIR",
                crate::pathfmt::display_path(&self.binding.git_directory),
            ),
            (
                "GIT_WORK_TREE",
                crate::pathfmt::display_path(&self.workspace),
            ),
            ("GIT_INDEX_FILE", crate::pathfmt::display_path(&index)),
            (
                "GIT_OBJECT_DIRECTORY",
                crate::pathfmt::display_path(&objects),
            ),
            (
                "GIT_ALTERNATE_OBJECT_DIRECTORIES",
                crate::pathfmt::display_path(&self.binding.common_directory.join("objects")),
            ),
            ("GIT_TERMINAL_PROMPT", "0".into()),
        ] {
            environment.insert(key.into(), value);
        }
        let environment_sha256 = crate::hash::sha256_bytes(&serde_json::to_vec(&environment)?);
        let args = vec![
            "-c".into(),
            format!(
                "core.hooksPath={}",
                crate::pathfmt::display_path(&hook_root)
            ),
            "hook".into(),
            "run".into(),
            "pre-commit".into(),
        ];
        let output = super::process::run_sandbox_process_tree(
            &self.binding.executable,
            &args,
            &self.workspace,
            &environment,
            &[],
            600_000,
            8 * 1024 * 1024,
        )?;
        if output.exit_code != 0 {
            bail!(
                "sandbox pre-commit hook refused candidate ({}): {}",
                output.exit_code,
                String::from_utf8_lossy(&output.stderr)
            );
        }
        if policy.verify(&self.workspace)? != hook_root
            || crate::source_fingerprint(&self.workspace)? != request.source_sha256
        {
            bail!("source or hook policy changed during sandbox preflight");
        }
        Ok(HookReceipt {
            policy_sha256: policy.digest()?,
            candidate_tree_oid,
            source_sha256: request.source_sha256.clone(),
            exit_code: output.exit_code,
            stdout_sha256: crate::hash::sha256_bytes(&output.stdout),
            stderr_sha256: crate::hash::sha256_bytes(&output.stderr),
            environment_sha256,
        })
    }

    /// Prepare an exact commit exclusively in a worker-owned object/index
    /// directory. Live HEAD, objects and index are not modified by preparation.
    pub fn prepare_commit(
        &self,
        request: &Request,
        registration: &Registration,
        now: i64,
        author_name: &str,
        author_email: &str,
    ) -> Result<CommitCandidate> {
        validate_request(request, registration, now)?;
        for field in [author_name, author_email] {
            if field.is_empty() || field.len() > 200 || field.contains(['\n', '\r', '\0', '<', '>'])
            {
                bail!("invalid registered commit identity");
            }
        }
        let Operation::Commit {
            expected_head,
            expected_index_sha256,
            message,
            changes,
            hook_receipt,
        } = &request.operation
        else {
            bail!("not a commit request");
        };
        let before = self.capture(registration)?;
        if &before.head != expected_head || &before.index_sha256 != expected_index_sha256 {
            bail!("commit snapshot changed before preparation");
        }
        let all: BTreeMap<_, _> = before
            .changes
            .iter()
            .map(|c| (c.path.as_str(), c))
            .collect();
        for change in changes {
            let actual = all
                .get(change.path.as_str())
                .ok_or_else(|| anyhow::anyhow!("requested commit path is not changed"))?;
            if actual.before != change.before || actual.after != change.after {
                bail!("requested commit file bytes changed");
            }
        }
        let directory = self
            .isolation
            .join(format!("candidate-{}", request.request_id));
        std::fs::create_dir(&directory)?; // Exclusive: never adopt a prior attempt.
        let objects = directory.join("objects");
        std::fs::create_dir(&objects)?;
        let index = directory.join("index");
        let environment = CandidateEnvironment {
            index: &index,
            objects: &objects,
        };
        self.run_in(
            ReadCommand::ReadTree(expected_head),
            &[],
            Some(&environment),
        )?;
        let mut index_info = Vec::new();
        for change in changes {
            match &change.after {
                Some(after) => {
                    let bytes = self
                        ._root
                        .read_file(Path::new(&change.path), 32 * 1024 * 1024)?;
                    if crate::hash::sha256_bytes(&bytes) != after.raw_sha256 {
                        bail!("source changed before candidate blob write");
                    }
                    let oid = String::from_utf8(self.run_in(
                        ReadCommand::HashBlob(&change.path),
                        &bytes,
                        Some(&environment),
                    )?)?
                    .trim()
                    .to_owned();
                    if oid != after.git_blob_oid {
                        bail!("filtered candidate blob differs from request");
                    }
                    index_info
                        .extend(format!("{:o} {oid}\t{}\0", after.mode, change.path).as_bytes());
                }
                None => index_info.extend(
                    format!(
                        "0 {}\t{}\0",
                        "0".repeat(registration.object_id_length),
                        change.path
                    )
                    .as_bytes(),
                ),
            }
        }
        self.run_in(ReadCommand::IndexInfo, &index_info, Some(&environment))?;
        let tree =
            String::from_utf8(self.run_in(ReadCommand::WriteTree, &[], Some(&environment))?)?
                .trim()
                .to_owned();
        if !is_hex(&tree, registration.object_id_length) {
            bail!("invalid candidate tree identity");
        }
        let commit_bytes = format!(
            "tree {tree}\nparent {expected_head}\nauthor {author_name} <{author_email}> {now} +0000\ncommitter {author_name} <{author_email}> {now} +0000\n\n{message}\n"
        );
        let commit = String::from_utf8(self.run_in(
            ReadCommand::HashCommit,
            commit_bytes.as_bytes(),
            Some(&environment),
        )?)?
        .trim()
        .to_owned();
        if !is_hex(&commit, registration.object_id_length) {
            bail!("invalid candidate commit identity");
        }
        let after = self.capture(registration)?;
        if serde_json::to_vec(&before)? != serde_json::to_vec(&after)? {
            bail!("source or Git changed during candidate generation");
        }
        let paths: Vec<_> = changes.iter().map(|c| c.path.clone()).collect();
        let mut object_hashes = BTreeMap::new();
        for prefix in std::fs::read_dir(&objects)? {
            let prefix = prefix?;
            let name = prefix.file_name().to_string_lossy().into_owned();
            if !is_hex(&name, 2) || !prefix.file_type()?.is_dir() {
                bail!("unexpected candidate object directory");
            }
            let pin = super::native::SourceDirectory::open(&prefix.path())?;
            for leaf in std::fs::read_dir(pin.path())? {
                let leaf = leaf?;
                let name = leaf.file_name().to_string_lossy().into_owned();
                if !is_hex(&name, registration.object_id_length - 2) {
                    bail!("unexpected candidate object filename");
                }
                let bytes = pin.read_file(Path::new(&name), 64 * 1024 * 1024)?;
                object_hashes.insert(
                    format!("{}/{}", prefix.file_name().to_string_lossy(), name),
                    crate::hash::sha256_bytes(&bytes),
                );
            }
        }
        let result = CommitCandidate {
            request_sha256: request.digest()?,
            registration_sha256: registration.digest()?,
            parent: expected_head.clone(),
            commit,
            tree,
            original_index_sha256: expected_index_sha256.clone(),
            candidate_index_sha256: crate::hash::sha256_file(&index)?,
            preserved_paths: before
                .changes
                .iter()
                .filter(|c| paths.binary_search(&c.path).is_err())
                .map(|c| c.path.clone())
                .collect(),
            paths,
            objects: object_hashes,
            changes_sha256: crate::hash::sha256_bytes(&serde_json::to_vec(&before.changes)?),
            preserved_changes_sha256: crate::hash::sha256_bytes(&serde_json::to_vec(
                &before
                    .changes
                    .iter()
                    .filter(|c| !changes.iter().any(|selected| selected.path == c.path))
                    .collect::<Vec<_>>(),
            )?),
        };
        match (&self.binding.hook_policy, hook_receipt) {
            (None, None) => {}
            (Some(policy), Some(receipt))
                if receipt.policy_sha256 == policy.digest()?
                    && receipt.candidate_tree_oid == result.tree
                    && receipt.source_sha256 == request.source_sha256
                    && receipt.exit_code == 0 => {}
            _ => bail!("sandbox hook receipt is missing or differs from the exact candidate"),
        }
        let original_index = self._git.read_file(Path::new("index"), 64 * 1024 * 1024)?;
        if crate::hash::sha256_bytes(&original_index) != result.original_index_sha256 {
            bail!("original index changed before candidate completion");
        }
        std::fs::write(directory.join("index-before"), original_index)?;
        crate::file_io::write_json(&directory.join("candidate.json"), &result)?;
        Ok(result)
    }

    /// Publish an already prepared exact candidate. Each standard Git lock is
    /// claimed from an identity-journaled seed in the same target directory.
    pub fn publish_commit(
        &self,
        request: &Request,
        registration: &Registration,
        now: i64,
    ) -> Result<CommitCandidate> {
        validate_request(request, registration, now)?;
        self.publish_commit_inner(request, registration, None)
    }

    pub(super) fn recover_admitted_commit(
        &self,
        request: &Request,
        registration: &Registration,
    ) -> Result<CommitCandidate> {
        self.publish_commit_inner(request, registration, None)
    }

    fn publish_commit_inner(
        &self,
        request: &Request,
        registration: &Registration,
        fault: Option<&str>,
    ) -> Result<CommitCandidate> {
        use super::publication::PublicationSlot;
        validate_request_structure(request, registration)?;
        let Operation::Commit {
            expected_head,
            expected_index_sha256,
            ..
        } = &request.operation
        else {
            bail!("not a commit request");
        };
        let _common_lock = crate::state_lock::acquire_state_lock(
            &self
                .isolation
                .join(format!("git-common-{}", registration.git_common_identity)),
        )?;
        let directory = self
            .isolation
            .join(format!("candidate-{}", request.request_id));
        let candidate: CommitCandidate =
            crate::file_io::read_json(&directory.join("candidate.json"))?
                .ok_or_else(|| anyhow::anyhow!("verified commit candidate is missing"))?;
        if candidate.request_sha256 != request.digest()?
            || candidate.registration_sha256 != registration.digest()?
            || &candidate.parent != expected_head
            || &candidate.original_index_sha256 != expected_index_sha256
        {
            bail!("commit candidate binding changed");
        }
        validate_relative_path(&self.binding.branch_ref)?;
        let ref_path = self.binding.common_directory.join(&self.binding.branch_ref);
        let ref_parent = ref_path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("branch parent missing"))?;
        let ref_name = ref_path
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| anyhow::anyhow!("invalid branch filename"))?;
        let ref_lock_name = format!("{ref_name}.lock");
        let journal_path = directory.join("publication.json");
        let mut record: PublicationRecord = if let Some(record) =
            crate::file_io::read_json(&journal_path)?
        {
            record
        } else {
            let current = self.capture(registration)?;
            if current.head != candidate.parent
                || current.index_sha256 != candidate.original_index_sha256
                || crate::hash::sha256_bytes(&serde_json::to_vec(&current.changes)?)
                    != candidate.changes_sha256
            {
                bail!("HEAD/index changed before publication");
            }
            let raw_ref = self
                ._common
                .read_file(Path::new(&self.binding.branch_ref), 4096)?;
            if std::str::from_utf8(&raw_ref)?.trim() != candidate.parent {
                bail!("publication requires the exact registered loose branch ref");
            }
            let index_bytes = std::fs::read(directory.join("index"))?;
            if crate::hash::sha256_bytes(&index_bytes) != candidate.candidate_index_sha256 {
                bail!("candidate index changed");
            }
            let index_seed = format!(".rayman-index-{}", request.request_id);
            let ref_seed = format!(".rayman-ref-{}", request.request_id);
            let index_slot =
                PublicationSlot::create(&self.binding.git_directory, &index_seed, &index_bytes)?;
            let ref_slot = PublicationSlot::create(
                ref_parent,
                &ref_seed,
                format!("{}\n", candidate.commit).as_bytes(),
            )?;
            index_slot.preserve_security_from(&self.binding.git_directory.join("index"))?;
            ref_slot.preserve_security_from(&ref_path)?;
            let record = PublicationRecord {
                request_sha256: request.digest()?,
                registration_sha256: registration.digest()?,
                index_seed,
                index_identity: index_slot.identity().into(),
                index_digest: index_slot.digest().into(),
                ref_seed,
                ref_identity: ref_slot.identity().into(),
                ref_digest: ref_slot.digest().into(),
                phase: "seeds_recorded".into(),
            };
            crate::file_io::write_json(&journal_path, &record)?;
            drop(index_slot);
            drop(ref_slot);
            record
        };
        if record.request_sha256 != request.digest()?
            || record.registration_sha256 != registration.digest()?
            || record.index_digest != candidate.candidate_index_sha256
            || record.ref_digest
                != crate::hash::sha256_bytes(format!("{}\n", candidate.commit).as_bytes())
        {
            bail!("publication journal binding changed");
        }
        if record.phase == "complete" {
            let current = self.capture(registration)?;
            if current.head != candidate.commit
                || current.index_sha256 != candidate.candidate_index_sha256
            {
                bail!("completed publication has since changed");
            }
            return Ok(candidate);
        }
        let claim = |parent: &Path,
                     seed: &str,
                     lock: &str,
                     id: &str,
                     digest: &str|
         -> Result<PublicationSlot> {
            if parent.join(lock).try_exists()? {
                PublicationSlot::resume(parent, lock, id, digest)
            } else {
                let mut slot = PublicationSlot::resume(parent, seed, id, digest)?;
                slot.rename(lock, false)?;
                Ok(slot)
            }
        };
        let current_head = String::from_utf8(self.run(ReadCommand::Head, &[])?)?
            .trim()
            .to_owned();
        let current_index = self._git.read_file(Path::new("index"), 64 * 1024 * 1024)?;
        if current_head != candidate.parent && current_head != candidate.commit {
            bail!("publication recovery refuses a later unrelated HEAD");
        }
        if crate::hash::sha256_bytes(&current_index) != candidate.original_index_sha256
            && crate::hash::sha256_bytes(&current_index) != candidate.candidate_index_sha256
        {
            bail!("publication recovery refuses a later index");
        }
        let mut index_slot =
            if crate::hash::sha256_bytes(&current_index) == candidate.candidate_index_sha256 {
                None
            } else {
                Some(claim(
                    &self.binding.git_directory,
                    &record.index_seed,
                    "index.lock",
                    &record.index_identity,
                    &record.index_digest,
                )?)
            };
        let mut ref_slot = if current_head == candidate.commit {
            None
        } else {
            Some(claim(
                ref_parent,
                &record.ref_seed,
                &ref_lock_name,
                &record.ref_identity,
                &record.ref_digest,
            )?)
        };
        if ref_slot.is_some() {
            let current = self.capture(registration)?;
            if current.head != candidate.parent
                || current.index_sha256 != candidate.original_index_sha256
                || crate::hash::sha256_bytes(&serde_json::to_vec(&current.changes)?)
                    != candidate.changes_sha256
            {
                bail!("Git state changed while acquiring standard locks");
            }
        }
        // Native no-replace object publication never lets Git choose a write
        // path under the source repository from untrusted configuration.
        let object_root = self.binding.common_directory.join("objects");
        let _object_pin = super::native::SourceDirectory::open(&object_root)?;
        for (relative, digest) in &candidate.objects {
            let (prefix, name) = relative
                .split_once('/')
                .ok_or_else(|| anyhow::anyhow!("invalid candidate object key"))?;
            if !is_hex(prefix, 2)
                || !is_hex(name, registration.object_id_length - 2)
                || !is_sha256(digest)
            {
                bail!("invalid candidate object identity");
            }
            let object = directory.join("objects").join(relative);
            let source = super::native::SourceDirectory::open(object.parent().unwrap())?;
            let bytes = source.read_file(Path::new(name), 64 * 1024 * 1024)?;
            if crate::hash::sha256_bytes(&bytes) != *digest {
                bail!("candidate object bytes changed");
            }
            let destination = object_root.join(prefix);
            match std::fs::create_dir(&destination) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(e.into()),
            };
            let target = super::native::SourceDirectory::open(&destination)?;
            if destination.join(name).try_exists()? {
                if crate::hash::sha256_bytes(&target.read_file(Path::new(name), 64 * 1024 * 1024)?)
                    != *digest
                {
                    bail!("existing object differs; preserve evidence");
                }
            } else {
                let seed = format!(".rayman-object-{}-{name}", request.request_id);
                let mut slot = PublicationSlot::create(&destination, &seed, &bytes)?;
                slot.rename(name, false)?;
            }
        }
        record.phase = "objects_published".into();
        crate::file_io::write_json(&journal_path, &record)?;
        if fault == Some("objects_published") {
            bail!("simulated interruption after objects publication");
        }
        if let Some(mut slot) = ref_slot.take() {
            slot.rename(ref_name, true)?;
            drop(slot);
        }
        record.phase = "ref_published".into();
        crate::file_io::write_json(&journal_path, &record)?;
        if fault == Some("ref_published") {
            bail!("simulated interruption after ref publication");
        }
        if let Some(mut slot) = index_slot.take() {
            slot.rename("index", true)?;
            drop(slot);
        }
        let terminal = self.capture(registration)?;
        if terminal.head != candidate.commit
            || terminal.index_sha256 != candidate.candidate_index_sha256
            || crate::hash::sha256_bytes(&serde_json::to_vec(&terminal.changes)?)
                != candidate.preserved_changes_sha256
            || terminal
                .changes
                .iter()
                .map(|c| c.path.clone())
                .collect::<Vec<_>>()
                != candidate.preserved_paths
        {
            bail!("publication terminal verification failed; retain journal");
        }
        record.phase = "complete".into();
        crate::file_io::write_json(&journal_path, &record)?;
        Ok(candidate)
    }

    pub fn capture(&self, registration: &Registration) -> Result<CommitSnapshot> {
        let branch = String::from_utf8(self.run(ReadCommand::Branch, &[])?)?;
        if branch.trim() != self.binding.branch_ref {
            bail!("registered branch changed");
        }
        let head = String::from_utf8(self.run(ReadCommand::Head, &[])?)?
            .trim()
            .to_string();
        if !is_hex(&head, registration.object_id_length) {
            bail!("Git object format changed");
        }
        let index_bytes = self._git.read_file(Path::new("index"), 64 * 1024 * 1024)?;
        let index_sha256 = crate::hash::sha256_bytes(&index_bytes);
        let index_entries = parse_entries(
            &self.run(ReadCommand::Index, &[])?,
            false,
            registration.object_id_length,
        )?;
        let head_entries = parse_entries(
            &self.run(ReadCommand::HeadTree, &[])?,
            true,
            registration.object_id_length,
        )?;
        if index_entries != head_entries {
            bail!("pre-staged or conflicted Git index requires separate handling");
        }
        let status = self.run(ReadCommand::Status, &[])?;
        let mut changes = Vec::new();
        for record in status.split(|b| *b == 0).filter(|r| !r.is_empty()) {
            if record.len() < 4 || record[2] != b' ' {
                bail!("malformed Git status");
            }
            let code = &record[..2];
            let path = std::str::from_utf8(&record[3..])?.to_string();
            validate_relative_path(&path)?;
            if !matches!(code, b"??" | b" M" | b" D") {
                bail!("Git status requires unsupported rename/type/staged handling");
            }
            let before = if let Some((mode, oid)) = head_entries.get(&path) {
                Some(FileVersion {
                    mode: *mode,
                    git_blob_oid: oid.clone(),
                    raw_sha256: crate::hash::sha256_bytes(&self.run(ReadCommand::Blob(oid), &[])?),
                })
            } else {
                None
            };
            let after = if code == b" D" {
                None
            } else {
                let attribute = self.run(ReadCommand::FilterAttribute(&path), &[])?;
                let fields: Vec<_> = attribute.split(|byte| *byte == 0).collect();
                if fields.len() != 4
                    || fields[0] != path.as_bytes()
                    || fields[1] != b"filter"
                    || fields[2] != b"unspecified"
                    || !fields[3].is_empty()
                {
                    bail!("active Git clean filter requires a dedicated sandbox adapter: {path}");
                }
                let bytes = self._root.read_file(Path::new(&path), 32 * 1024 * 1024)?;
                let oid = String::from_utf8(self.run(ReadCommand::Filtered(&path), &bytes)?)?
                    .trim()
                    .to_string();
                Some(FileVersion {
                    raw_sha256: crate::hash::sha256_bytes(&bytes),
                    git_blob_oid: oid,
                    mode: before.as_ref().map_or(0o100644, |v| v.mode),
                })
            };
            let change = Change {
                path,
                before,
                after,
            };
            change.validate(registration)?;
            changes.push(change);
        }
        changes.sort_by(|a, b| a.path.cmp(&b.path));
        let observed = String::from_utf8(self.run(ReadCommand::Head, &[])?)?
            .trim()
            .to_string();
        let observed_index = self._git.read_file(Path::new("index"), 64 * 1024 * 1024)?;
        if observed != head || crate::hash::sha256_bytes(&observed_index) != index_sha256 {
            bail!("Git HEAD or index changed during snapshot");
        }
        Ok(CommitSnapshot {
            head,
            index_sha256,
            changes,
        })
    }
}

fn parse_entries(
    bytes: &[u8],
    tree: bool,
    oid_len: usize,
) -> Result<BTreeMap<String, (u32, String)>> {
    let mut entries = BTreeMap::new();
    let mut folded = BTreeSet::new();
    for record in bytes.split(|b| *b == 0).filter(|r| !r.is_empty()) {
        let text = std::str::from_utf8(record)?;
        let (header, path) = text
            .split_once('\t')
            .ok_or_else(|| anyhow::anyhow!("malformed Git entry"))?;
        validate_relative_path(path)?;
        let fields: Vec<_> = header.split(' ').collect();
        if fields.len() != 3 {
            bail!("malformed Git entry fields");
        }
        let mode = u32::from_str_radix(fields[0], 8)?;
        let oid = if tree {
            if fields[1] != "blob" {
                bail!("Gitlinks are not supported");
            }
            fields[2]
        } else {
            if fields[2] != "0" {
                bail!("conflicted index");
            }
            fields[1]
        };
        if !matches!(mode, 0o100644 | 0o100755)
            || !is_hex(oid, oid_len)
            || !folded.insert(path.to_lowercase())
        {
            bail!("unsafe or duplicate Git entry");
        }
        if entries.insert(path.into(), (mode, oid.into())).is_some() {
            bail!("duplicate Git entry");
        }
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(windows)]
    #[test]
    fn git_tree_and_index_parsers_reject_conflicts_aliases_and_nonfiles() {
        let oid = "a".repeat(40);
        let index = format!("100644 {oid} 0\tsrc/模块.rs\0");
        let tree = format!("100644 blob {oid}\tsrc/模块.rs\0");
        assert_eq!(
            parse_entries(index.as_bytes(), false, 40).unwrap(),
            parse_entries(tree.as_bytes(), true, 40).unwrap()
        );
        for text in [
            format!("100644 {oid} 1\ta\0"),
            format!("120000 {oid} 0\ta\0"),
            format!("100644 {oid} 0\t../a\0"),
            format!("100644 {oid} 0\ta\0").repeat(2),
            format!("100644 {oid} 0\tA\0") + &format!("100644 {oid} 0\ta\0"),
        ] {
            assert!(parse_entries(text.as_bytes(), false, 40).is_err());
        }
    }

    #[cfg(windows)]
    #[test]
    fn real_git_snapshot_preserves_source_and_index_with_added_crlf_and_deleted_files() {
        let repo = tempfile::tempdir().unwrap();
        let trusted = tempfile::tempdir().unwrap();
        let sid = crate::execution_context::execution_context_probe()
            .principal_sid
            .unwrap();
        super::super::native::protect_fixture_directory(trusted.path(), &sid, "").unwrap();
        let isolation = ProtectedDirectory::open(trusted.path(), &sid).unwrap();
        let program = PathBuf::from("C:/Program Files/Git/mingw64/bin/git.exe");
        let run = |args: &[&str]| {
            let out = std::process::Command::new(&program)
                .args(args)
                .current_dir(repo.path())
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        run(&["init", "-b", "main"]);
        run(&["config", "user.name", "Fixture"]);
        run(&["config", "user.email", "fixture@example.invalid"]);
        std::fs::write(repo.path().join(".gitattributes"), b"* text=auto eol=lf\n").unwrap();
        std::fs::write(repo.path().join("old.txt"), b"before\n").unwrap();
        std::fs::write(repo.path().join("deleted.txt"), b"delete\n").unwrap();
        run(&["add", "."]);
        run(&["-c", "commit.gpgsign=false", "commit", "-m", "fixture"]);
        std::fs::create_dir(repo.path().join(".githooks")).unwrap();
        std::fs::write(
            repo.path().join(".githooks/pre-commit"),
            b"#!/bin/sh\nset -e\ngit diff --cached --name-only | grep -Fx added.txt >/dev/null\ngit diff --cached --name-only | grep -Fx deleted.txt >/dev/null\nif git diff --cached --name-only | grep -Fx old.txt >/dev/null; then exit 9; fi\n",
        )
        .unwrap();
        run(&["add", ".githooks/pre-commit"]);
        run(&["-c", "commit.gpgsign=false", "commit", "-m", "tracked hook"]);
        run(&["config", "core.hooksPath", ".githooks"]);
        std::fs::write(repo.path().join("old.txt"), b"after\r\n").unwrap();
        std::fs::write(repo.path().join("added.txt"), b"added\r\n").unwrap();
        std::fs::remove_file(repo.path().join("deleted.txt")).unwrap();
        let gitdir = repo.path().join(".git");
        let old_index = std::fs::read(gitdir.join("index")).unwrap();
        let mut r = super::super::tests::registration();
        r.root_identity = super::super::native::SourceDirectory::open(repo.path())
            .unwrap()
            .identity()
            .into();
        r.git_common_identity = super::super::native::SourceDirectory::open(&gitdir)
            .unwrap()
            .identity()
            .into();
        let binding = GitBinding {
            content_policy: GitContentPolicy::capture(&program, repo.path()).unwrap(),
            hook_policy: GitHookPolicy::capture(&program, repo.path()).unwrap(),
            executable: program.clone(),
            executable_sha256: crate::hash::sha256_file(&program).unwrap(),
            git_directory: gitdir.clone(),
            common_directory: gitdir.clone(),
            branch_ref: "refs/heads/main".into(),
            config_sha256: crate::hash::sha256_file(&gitdir.join("config")).unwrap(),
        };
        let reader = GitInspector::open(repo.path(), &binding, &r, &isolation).unwrap();
        let snapshot = reader.capture(&r).unwrap();
        assert_eq!(
            snapshot
                .changes
                .iter()
                .map(|c| c.path.as_str())
                .collect::<Vec<_>>(),
            vec!["added.txt", "deleted.txt", "old.txt"]
        );
        assert_eq!(
            snapshot.changes[0].after.as_ref().unwrap().raw_sha256,
            crate::hash::sha256_bytes(b"added\r\n")
        );
        assert_eq!(
            std::fs::read(repo.path().join("added.txt")).unwrap(),
            b"added\r\n"
        );
        assert_eq!(std::fs::read(gitdir.join("index")).unwrap(), old_index);
        let mut q = super::super::tests::request(&r);
        q.source_sha256 = crate::source_fingerprint(repo.path()).unwrap();
        q.operation = Operation::Commit {
            expected_head: snapshot.head.clone(),
            expected_index_sha256: snapshot.index_sha256.clone(),
            message: "exact candidate".into(),
            changes: snapshot
                .changes
                .into_iter()
                .filter(|c| c.path != "old.txt")
                .collect(),
            hook_receipt: None,
        };
        let receipt = reader
            .run_hook_preflight(&q, &r, binding.hook_policy.as_ref().unwrap())
            .unwrap();
        let Operation::Commit { hook_receipt, .. } = &mut q.operation else {
            unreachable!()
        };
        *hook_receipt = Some(receipt);
        let candidate = reader
            .prepare_commit(&q, &r, 1000, "Fixture", "fixture@example.invalid")
            .unwrap();
        assert_eq!(candidate.paths, vec!["added.txt", "deleted.txt"]);
        let mut without_hook = q.clone();
        without_hook.request_id = "8".repeat(32);
        let Operation::Commit { hook_receipt, .. } = &mut without_hook.operation else {
            unreachable!()
        };
        *hook_receipt = None;
        assert!(
            reader
                .prepare_commit(
                    &without_hook,
                    &r,
                    1000,
                    "Fixture",
                    "fixture@example.invalid"
                )
                .is_err()
        );
        assert_eq!(candidate.preserved_paths, vec!["old.txt"]);
        assert_eq!(std::fs::read(gitdir.join("index")).unwrap(), old_index);
        assert_eq!(reader.capture(&r).unwrap().head, candidate.parent);
        assert!(
            reader
                .prepare_commit(&q, &r, 1000, "Fixture", "fixture@example.invalid")
                .is_err()
        );
        std::fs::write(repo.path().join("added.txt"), b"later edit\r\n").unwrap();
        assert!(reader.publish_commit(&q, &r, 1001).is_err());
        assert_eq!(reader.capture(&r).unwrap().head, candidate.parent);
        assert!(!gitdir.join("index.lock").exists());
        std::fs::write(repo.path().join("added.txt"), b"added\r\n").unwrap();
        let fault = reader
            .publish_commit_inner(&q, &r, Some("ref_published"))
            .err()
            .unwrap();
        assert!(
            fault.to_string().contains("simulated interruption"),
            "{fault:#}"
        );
        assert!(gitdir.join("index.lock").exists());
        drop(reader);
        let reader = GitInspector::open(repo.path(), &binding, &r, &isolation).unwrap();
        let published = reader.publish_commit(&q, &r, 1002).unwrap();
        assert!(!gitdir.join("index.lock").exists());
        assert_eq!(reader.capture(&r).unwrap().head, published.commit);
        assert_eq!(
            reader
                .capture(&r)
                .unwrap()
                .changes
                .iter()
                .map(|c| c.path.as_str())
                .collect::<Vec<_>>(),
            vec!["old.txt"]
        );
        assert_eq!(
            reader.publish_commit(&q, &r, 1002).unwrap().commit,
            published.commit
        );
        drop(reader);
        run(&["add", "old.txt"]);
        let reader = GitInspector::open(repo.path(), &binding, &r, &isolation).unwrap();
        assert!(reader.capture(&r).is_err());
    }
}
