//! Exact Git snapshot reader. It runs fixed read-only plumbing inside the
//! single-process native boundary; it never stages, writes an object or a ref.
use super::*;
use anyhow::Context;
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
    Tree(&'a str),
    ObjectType(&'a str),
    ObjectBytes(&'a str, &'a str),
    HashObjectBytes(&'a str),
    Head,
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
    #[serde(default = "legacy_publication_version")]
    version: u32,
    #[serde(default)]
    metadata_ready: bool,
    request_sha256: String,
    registration_sha256: String,
    index_seed: String,
    index_identity: String,
    index_digest: String,
    ref_seed: String,
    ref_identity: String,
    ref_digest: String,
    phase: String,
    #[serde(default)]
    object_seeds: BTreeMap<String, ObjectSeed>,
    #[serde(default)]
    object_attempts: BTreeMap<String, u8>,
}

fn legacy_publication_version() -> u32 {
    1
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ObjectSeed {
    name: String,
    identity: String,
    digest: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StagingAttempts {
    request_sha256: String,
    registration_sha256: String,
    ids: Vec<String>,
}

fn staging_nonce() -> Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|e| anyhow::anyhow!("staging entropy unavailable: {e}"))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn publication_fault(fault: Option<&str>, point: &str) -> Result<()> {
    if fault == Some(point) {
        bail!("simulated publication interruption: {point}");
    }
    Ok(())
}

struct CandidateEnvironment<'a> {
    index: &'a Path,
    objects: &'a Path,
}

pub(super) struct LegacyRecovery<'a> {
    pub original_id: &'a str,
    pub original_digest: &'a str,
    pub candidate_sha256: &'a str,
    pub source_sha256: &'a str,
    pub identity: &'a CommitIdentity,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyAcceptance {
    request_sha256: String,
    registration_sha256: String,
    candidate_sha256: String,
    verified_source_sha256: String,
}

pub struct GitInspector<'a> {
    binding: GitBinding,
    workspace: PathBuf,
    isolation: PathBuf,
    _root: super::native::SourceDirectory,
    _git: super::native::SourceDirectory,
    _common: super::native::SourceDirectory,
    _executable: std::fs::File,
    _worktree_config: Option<super::native::SourceFile>,
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
            || (binding.branch_ref != "HEAD" && !binding.branch_ref.starts_with("refs/heads/"))
            || binding.branch_ref.contains(['\0', '\n', '\r'])
        {
            bail!("invalid fixed Git binding");
        }
        let root = super::native::SourceDirectory::open(workspace)?;
        let git = super::native::SourceDirectory::open(&binding.git_directory)?;
        let common = super::native::SourceDirectory::open(&binding.common_directory)?;
        super::relocation::check_git(isolation, registration, &root, &git, &common)?;
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
        let worktree_config = if git.path().join("config.worktree").try_exists()? {
            let pin = git.pin_file(Path::new("config.worktree"))?;
            validate_codex_worktree_metadata(&git.read_file(Path::new("config.worktree"), 65536)?)?;
            Some(pin)
        } else {
            None
        };
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
            _worktree_config: worktree_config,
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
        if self._worktree_config.is_none()
            && self._git.path().join("config.worktree").try_exists()?
        {
            bail!(
                "worktree configuration appeared during the operation; retry from a fresh binding"
            );
        }
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
            ReadCommand::Tree(oid)
            | ReadCommand::ObjectType(oid)
            | ReadCommand::ObjectBytes(_, oid)
                if !is_hex(oid, 40) && !is_hex(oid, 64) =>
            {
                bail!("invalid recovery object identity");
            }
            ReadCommand::Tree(oid) => vec![
                "ls-tree".into(),
                "-r".into(),
                "-z".into(),
                "--full-tree".into(),
                oid.into(),
            ],
            ReadCommand::ObjectType(oid) => vec!["cat-file".into(), "-t".into(), oid.into()],
            ReadCommand::ObjectBytes(kind, oid) => {
                if !matches!(kind, "blob" | "tree" | "commit") {
                    bail!("invalid recovery object type");
                }
                vec!["cat-file".into(), kind.into(), oid.into()]
            }
            ReadCommand::HashObjectBytes(kind) => {
                if !matches!(kind, "blob" | "tree" | "commit") {
                    bail!("invalid recovery hash type");
                }
                vec![
                    "hash-object".into(),
                    "-t".into(),
                    kind.into(),
                    "--stdin".into(),
                ]
            }
            ReadCommand::Head => vec!["rev-parse".into(), "--verify".into(), "HEAD".into()],
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

    #[cfg(test)]
    pub(super) fn test_publish_cut(
        &self,
        request: &Request,
        registration: &Registration,
        point: &str,
    ) -> Result<CommitCandidate> {
        self.publish_commit_inner(request, registration, Some(point))
    }

    fn publish_commit_inner(
        &self,
        request: &Request,
        registration: &Registration,
        fault: Option<&str>,
    ) -> Result<CommitCandidate> {
        validate_request_structure(request, registration)?;
        let Operation::Commit {
            expected_head,
            expected_index_sha256,
            ..
        } = &request.operation
        else {
            bail!("not a commit request");
        };
        self.publish_bound_commit(
            &request.request_id,
            &request.digest()?,
            Some((expected_head, expected_index_sha256)),
            registration,
            fault,
            None,
        )
    }

    pub(super) fn recover_legacy_prepared(
        &self,
        binding: LegacyRecovery<'_>,
        registration: &Registration,
    ) -> Result<CommitCandidate> {
        if !is_id(binding.original_id)
            || !is_sha256(binding.original_digest)
            || !is_sha256(binding.candidate_sha256)
            || !is_sha256(binding.source_sha256)
        {
            bail!("invalid legacy recovery binding");
        }
        self.publish_bound_commit(
            binding.original_id,
            binding.original_digest,
            None,
            registration,
            None,
            Some(&binding),
        )
    }

    fn publish_bound_commit(
        &self,
        request_id: &str,
        request_sha256: &str,
        expected: Option<(&str, &str)>,
        registration: &Registration,
        fault: Option<&str>,
        legacy: Option<&LegacyRecovery<'_>>,
    ) -> Result<CommitCandidate> {
        use super::publication::PublicationSlot;
        let _common_lock = crate::state_lock::acquire_state_lock(
            &self
                .isolation
                .join(format!("git-common-{}", registration.git_common_identity)),
        )?;
        let directory = self.isolation.join(format!("candidate-{}", request_id));
        let candidate: CommitCandidate =
            crate::file_io::read_json(&directory.join("candidate.json"))?
                .ok_or_else(|| anyhow::anyhow!("verified commit candidate is missing"))?;
        if candidate.request_sha256 != request_sha256
            || candidate.registration_sha256 != registration.digest()?
            || expected.is_some_and(|(head, index)| {
                candidate.parent != head || candidate.original_index_sha256 != index
            })
        {
            bail!("commit candidate binding changed");
        }
        if let Some(binding) = legacy {
            self.accept_legacy_candidate(&directory, &candidate, registration, binding, true)?;
        }
        self.verify_head_binding(registration)?;
        validate_relative_path(&self.binding.branch_ref)?;
        // Detached HEAD belongs to this worktree's private Git directory,
        // never to the common directory or the parent checkout's HEAD.
        let ref_root = if self.binding.branch_ref == "HEAD" {
            &self._git
        } else {
            &self._common
        };
        let ref_path = ref_root.path().join(&self.binding.branch_ref);
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
            let raw_ref = ref_root.read_file(Path::new(&self.binding.branch_ref), 4096)?;
            if std::str::from_utf8(&raw_ref)?.trim() != candidate.parent {
                bail!("publication requires the exact registered reference value");
            }
            let index_bytes = std::fs::read(directory.join("index"))?;
            if crate::hash::sha256_bytes(&index_bytes) != candidate.candidate_index_sha256 {
                bail!("candidate index changed");
            }
            let attempts_path = directory.join("staging-attempts.json");
            let mut attempts: StagingAttempts = crate::file_io::read_json(&attempts_path)?
                .unwrap_or(StagingAttempts {
                    request_sha256: request_sha256.into(),
                    registration_sha256: registration.digest()?,
                    ids: vec![],
                });
            if attempts.request_sha256 != request_sha256
                || attempts.registration_sha256 != registration.digest()?
                || attempts.ids.len() >= 8
                || attempts.ids.iter().any(|id| !is_id(id))
                || attempts
                    .ids
                    .iter()
                    .collect::<std::collections::BTreeSet<_>>()
                    .len()
                    != attempts.ids.len()
            {
                bail!(
                    "staging attempt binding invalid or retry limit reached; retain orphan evidence"
                );
            }
            let attempt = staging_nonce()?;
            if attempts.ids.contains(&attempt) {
                bail!("staging attempt identity collision");
            }
            attempts.ids.push(attempt.clone());
            crate::file_io::write_json(&attempts_path, &attempts)?;
            publication_fault(fault, "attempt_recorded")?;
            // A seed whose identity was never journaled is retained, never
            // adopted by its name or contents on a later attempt.
            let index_seed = format!(".rayman-index-{}-{attempt}", request_id);
            let ref_seed = format!(".rayman-ref-{}-{attempt}", request_id);
            let index_slot =
                PublicationSlot::create(&self.binding.git_directory, &index_seed, &index_bytes)?;
            publication_fault(fault, "index_seed_created")?;
            let ref_slot = PublicationSlot::create(
                ref_parent,
                &ref_seed,
                format!("{}\n", candidate.commit).as_bytes(),
            )?;
            publication_fault(fault, "ref_seed_created")?;
            let record = PublicationRecord {
                version: 2,
                metadata_ready: false,
                request_sha256: request_sha256.into(),
                registration_sha256: registration.digest()?,
                index_seed,
                index_identity: index_slot.identity().into(),
                index_digest: index_slot.digest().into(),
                ref_seed,
                ref_identity: ref_slot.identity().into(),
                ref_digest: ref_slot.digest().into(),
                phase: "seeds_recorded".into(),
                object_seeds: BTreeMap::new(),
                object_attempts: BTreeMap::new(),
            };
            crate::file_io::write_json(&journal_path, &record)?;
            publication_fault(fault, "seeds_recorded")?;
            drop(index_slot);
            drop(ref_slot);
            record
        };
        if !matches!(record.version, 1 | 2)
            || record.request_sha256 != request_sha256
            || record.registration_sha256 != registration.digest()?
            || record.index_digest != candidate.candidate_index_sha256
            || record.ref_digest
                != crate::hash::sha256_bytes(format!("{}\n", candidate.commit).as_bytes())
        {
            bail!("publication journal binding changed");
        }
        if !matches!(
            record.phase.as_str(),
            "seeds_recorded" | "objects_published" | "ref_published" | "complete"
        ) || record
            .object_attempts
            .iter()
            .any(|(key, count)| !candidate.objects.contains_key(key) || !(1..=8).contains(count))
            || record.object_seeds.iter().any(|(key, seed)| {
                candidate.objects.get(key) != Some(&seed.digest)
                    || !record.object_attempts.contains_key(key)
                    || !is_sha256(&seed.identity)
                    || !seed
                        .name
                        .starts_with(&format!(".rayman-object-{}-", request_id))
            })
        {
            bail!("invalid publication phase or object staging record");
        }
        if record.version == 1 {
            // v1 persisted its journal only after both metadata copies.
            record.version = 2;
            record.metadata_ready = true;
            crate::file_io::write_json(&journal_path, &record)?;
        }
        if record.version == 2 && !record.metadata_ready {
            if record.phase != "seeds_recorded" {
                bail!("publication advanced without verified metadata");
            }
            let index = PublicationSlot::resume(
                &self.binding.git_directory,
                &record.index_seed,
                &record.index_identity,
                &record.index_digest,
            )?;
            let reference = PublicationSlot::resume(
                ref_parent,
                &record.ref_seed,
                &record.ref_identity,
                &record.ref_digest,
            )?;
            index
                .preserve_access_from(&self.binding.git_directory.join("index"))
                .context("preserve Git index access policy")?;
            publication_fault(fault, "index_metadata_prepared")?;
            reference
                .preserve_access_from(&ref_path)
                .context("preserve Git ref access policy")?;
            publication_fault(fault, "ref_metadata_prepared")?;
            record.metadata_ready = true;
            crate::file_io::write_json(&journal_path, &record)?;
            publication_fault(fault, "metadata_recorded")?;
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
        publication_fault(fault, "index_lock_claimed")?;
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
        publication_fault(fault, "ref_lock_claimed")?;
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
                let mut slot = if let Some(seed) = record.object_seeds.get(relative) {
                    PublicationSlot::resume(&destination, &seed.name, &seed.identity, &seed.digest)?
                } else {
                    let count = record.object_attempts.get(relative).copied().unwrap_or(0);
                    if count >= 8 {
                        bail!("object staging retry limit reached; retain orphan evidence");
                    }
                    record.object_attempts.insert(relative.clone(), count + 1);
                    crate::file_io::write_json(&journal_path, &record)?;
                    publication_fault(fault, "object_attempt_recorded")?;
                    let seed = format!(".rayman-object-{}-{}-{name}", request_id, staging_nonce()?);
                    let slot = PublicationSlot::create(&destination, &seed, &bytes)?;
                    publication_fault(fault, "object_seed_created")?;
                    record.object_seeds.insert(
                        relative.clone(),
                        ObjectSeed {
                            name: seed,
                            identity: slot.identity().into(),
                            digest: slot.digest().into(),
                        },
                    );
                    crate::file_io::write_json(&journal_path, &record)?;
                    publication_fault(fault, "object_seed_recorded")?;
                    slot
                };
                slot.rename(name, false)?;
            }
            publication_fault(fault, "object_published")?;
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
        publication_fault(fault, "index_published")?;
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
        publication_fault(fault, "complete")?;
        Ok(candidate)
    }

    pub(super) fn preview_legacy_candidate(
        &self,
        directory: &Path,
        candidate: &CommitCandidate,
        registration: &Registration,
        binding: &LegacyRecovery<'_>,
    ) -> Result<()> {
        self.accept_legacy_candidate(directory, candidate, registration, binding, false)
    }

    fn accept_legacy_candidate(
        &self,
        directory: &Path,
        candidate: &CommitCandidate,
        registration: &Registration,
        binding: &LegacyRecovery<'_>,
        persist_acceptance: bool,
    ) -> Result<()> {
        if self.binding.hook_policy.is_some() {
            bail!("legacy recovery without the original request cannot attest a hook receipt");
        }
        let protected = super::native::SourceDirectory::open(directory)?;
        let raw = protected.read_file(Path::new("candidate.json"), 64 * 1024 * 1024)?;
        if crate::hash::sha256_bytes(&raw) != binding.candidate_sha256
            || crate::source_fingerprint(&self.workspace)? != binding.source_sha256
            || !is_hex(&candidate.commit, registration.object_id_length)
            || !is_hex(&candidate.tree, registration.object_id_length)
            || !is_hex(&candidate.parent, registration.object_id_length)
            || crate::hash::sha256_bytes(
                &protected.read_file(Path::new("index"), 64 * 1024 * 1024)?,
            ) != candidate.candidate_index_sha256
            || crate::hash::sha256_bytes(
                &protected.read_file(Path::new("index-before"), 64 * 1024 * 1024)?,
            ) != candidate.original_index_sha256
        {
            bail!("legacy candidate/source/index evidence differs");
        }
        let acceptance_path = directory.join("legacy-recovery.json");
        if let Some(accepted) = crate::file_io::read_json::<LegacyAcceptance>(&acceptance_path)? {
            if accepted.request_sha256 != binding.original_digest
                || accepted.registration_sha256 != registration.digest()?
                || accepted.candidate_sha256 != binding.candidate_sha256
                || accepted.verified_source_sha256 != binding.source_sha256
            {
                bail!("legacy recovery acceptance differs");
            }
            return Ok(());
        }
        if directory.join("publication.json").try_exists()? {
            bail!("legacy publication journal has no verified recovery acceptance");
        }
        let snapshot = self.capture(registration)?;
        if snapshot.head != candidate.parent
            || snapshot.index_sha256 != candidate.original_index_sha256
            || crate::hash::sha256_bytes(&serde_json::to_vec(&snapshot.changes)?)
                != candidate.changes_sha256
            || candidate.paths.is_empty()
            || candidate.paths.len() > 1024
            || candidate.paths.windows(2).any(|pair| pair[0] >= pair[1])
        {
            bail!("legacy recovery requires the original complete Git snapshot");
        }
        let mut expected = parse_entries(
            &self.run(ReadCommand::HeadTree, &[])?,
            true,
            registration.object_id_length,
        )?;
        let mut folded = std::collections::BTreeSet::new();
        for path in &candidate.paths {
            validate_relative_path(path)?;
            if !folded.insert(path.to_lowercase()) {
                bail!("legacy selected paths alias each other");
            }
            let change = snapshot
                .changes
                .iter()
                .find(|change| &change.path == path)
                .ok_or_else(|| {
                    anyhow::anyhow!("legacy selected path is absent from the snapshot")
                })?;
            change.validate(registration)?;
            match &change.after {
                Some(after) => {
                    expected.insert(path.clone(), (after.mode, after.git_blob_oid.clone()));
                }
                None => {
                    expected.remove(path);
                }
            }
        }
        let preserved: Vec<_> = snapshot
            .changes
            .iter()
            .filter(|change| candidate.paths.binary_search(&change.path).is_err())
            .collect();
        if preserved
            .iter()
            .map(|change| change.path.clone())
            .collect::<Vec<_>>()
            != candidate.preserved_paths
            || crate::hash::sha256_bytes(&serde_json::to_vec(&preserved)?)
                != candidate.preserved_changes_sha256
        {
            bail!("legacy preserved changes differ");
        }
        let index = directory.join("index");
        let objects = directory.join("objects");
        let environment = CandidateEnvironment {
            index: &index,
            objects: &objects,
        };
        let entries = parse_entries(
            &self.run_in(ReadCommand::Index, &[], Some(&environment))?,
            false,
            registration.object_id_length,
        )?;
        let tree = parse_entries(
            &self.run_in(ReadCommand::Tree(&candidate.tree), &[], Some(&environment))?,
            true,
            registration.object_id_length,
        )?;
        if entries != expected || tree != expected {
            bail!("legacy tree/index contains changes outside the selected paths");
        }
        let mut oids =
            std::collections::BTreeSet::from([candidate.commit.clone(), candidate.tree.clone()]);
        for (relative, digest) in &candidate.objects {
            let Some((prefix, name)) = relative.split_once('/') else {
                bail!("invalid legacy object path");
            };
            if !is_hex(prefix, 2)
                || !is_hex(name, registration.object_id_length - 2)
                || !is_sha256(digest)
            {
                bail!("invalid legacy object binding");
            }
            let parent = super::native::SourceDirectory::open(&objects.join(prefix))?;
            if crate::hash::sha256_bytes(&parent.read_file(Path::new(name), 64 * 1024 * 1024)?)
                != *digest
            {
                bail!("legacy object bytes differ");
            }
            oids.insert(format!("{prefix}{name}"));
        }
        let mut commit = None;
        for oid in oids {
            let kind = String::from_utf8(self.run_in(
                ReadCommand::ObjectType(&oid),
                &[],
                Some(&environment),
            )?)?
            .trim()
            .to_owned();
            let bytes = self.run_in(
                ReadCommand::ObjectBytes(&kind, &oid),
                &[],
                Some(&environment),
            )?;
            let actual = String::from_utf8(self.run_in(
                ReadCommand::HashObjectBytes(&kind),
                &bytes,
                Some(&environment),
            )?)?;
            if actual.trim() != oid {
                bail!("legacy object name/content hash differs");
            }
            if oid == candidate.commit {
                if kind != "commit" {
                    bail!("legacy commit object has the wrong type");
                }
                commit = Some(bytes);
            }
        }
        let commit =
            String::from_utf8(commit.ok_or_else(|| anyhow::anyhow!("legacy commit missing"))?)?;
        let (header, message) = commit
            .split_once("\n\n")
            .ok_or_else(|| anyhow::anyhow!("invalid legacy commit header"))?;
        let headers: Vec<_> = header.lines().collect();
        let author = format!(
            "author {} <{}> ",
            binding.identity.name, binding.identity.email
        );
        let committer = format!(
            "committer {} <{}> ",
            binding.identity.name, binding.identity.email
        );
        if headers.len() != 4
            || headers[0] != format!("tree {}", candidate.tree)
            || headers[1] != format!("parent {}", candidate.parent)
            || !headers[2].starts_with(&author)
            || !headers[3].starts_with(&committer)
            || headers[2].strip_prefix(&author) != headers[3].strip_prefix(&committer)
        {
            bail!("legacy commit parent/tree/identity differs");
        }
        let timestamp = headers[2].strip_prefix(&author).unwrap();
        if !timestamp
            .strip_suffix(" +0000")
            .is_some_and(|text| text.parse::<i64>().is_ok())
        {
            bail!("invalid legacy commit timestamp");
        }
        let message = message
            .strip_suffix('\n')
            .ok_or_else(|| anyhow::anyhow!("invalid legacy commit message ending"))?;
        if message.is_empty()
            || message.trim() != message
            || message.chars().count() > 200
            || message.chars().any(char::is_control)
        {
            bail!("invalid legacy commit message");
        }
        if crate::source_fingerprint(&self.workspace)? != binding.source_sha256 {
            bail!("legacy source changed during verification");
        }
        // This records a NEW recovery verification, not a reconstructed old Request.
        if persist_acceptance {
            crate::file_io::write_json(
                &acceptance_path,
                &LegacyAcceptance {
                    request_sha256: binding.original_digest.into(),
                    registration_sha256: registration.digest()?,
                    candidate_sha256: binding.candidate_sha256.into(),
                    verified_source_sha256: binding.source_sha256.into(),
                },
            )?;
        }
        Ok(())
    }

    fn verify_head_binding(&self, registration: &Registration) -> Result<()> {
        let bytes = self._git.read_file(Path::new("HEAD"), 4096)?;
        let head = std::str::from_utf8(&bytes)?.trim();
        if self.binding.branch_ref == "HEAD" {
            if !is_hex(head, registration.object_id_length) {
                bail!("registered detached HEAD is no longer detached");
            }
        } else if head.strip_prefix("ref: ") != Some(self.binding.branch_ref.as_str()) {
            bail!("registered branch changed");
        }
        Ok(())
    }

    pub fn capture(&self, registration: &Registration) -> Result<CommitSnapshot> {
        self.verify_head_binding(registration)?;
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
            validate_observed_git_path(&path)?;
            if is_protected_workflow_path(&path) {
                continue;
            }
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

fn validate_codex_worktree_metadata(bytes: &[u8]) -> Result<()> {
    let text = std::str::from_utf8(bytes)?;
    let mut section = false;
    let mut value_seen = false;
    for line in text.lines().map(str::trim) {
        if line.is_empty() || line.starts_with(['#', ';']) {
            continue;
        }
        if line.contains('\0') || line.ends_with('\\') {
            bail!("unsupported worktree config continuation");
        }
        if line.eq_ignore_ascii_case("[codex]") && !section {
            section = true;
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            bail!("unsupported worktree Git configuration");
        };
        if !section
            || value_seen
            || !key
                .trim()
                .eq_ignore_ascii_case("localEnvironmentConfigPath")
            || value.trim().is_empty()
        {
            bail!("only Codex localEnvironmentConfigPath metadata is allowed in worktree config");
        }
        value_seen = true;
    }
    Ok(())
}

fn is_protected_workflow_path(path: &str) -> bool {
    let first = path.split('/').next().unwrap_or_default();
    first.eq_ignore_ascii_case(".agent-checkpoints")
        || (first.eq_ignore_ascii_case(".RaymanCodingSkill")
            && path != ".RaymanCodingSkill/quality.json")
}

// Observation does not grant permission to include state files in a commit.
fn validate_observed_git_path(path: &str) -> Result<()> {
    if is_protected_workflow_path(path) {
        let (_, suffix) = path
            .split_once('/')
            .ok_or_else(|| anyhow::anyhow!("workflow state entry must name a file"))?;
        validate_relative_path(&format!("observed-state/{suffix}"))
    } else {
        validate_relative_path(path)
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
        validate_observed_git_path(path)?;
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
    fn worktree_metadata_rejects_git_execution_and_include_settings() {
        for value in [
            "[codex]\nlocalEnvironmentConfigPath = __none__\n",
            "[codex]\r\n\tlocalEnvironmentConfigPath = .codex/environments/environment.toml\r\n",
            "# comment\n",
        ] {
            validate_codex_worktree_metadata(value.as_bytes()).unwrap();
        }
        for value in [
            "[core]\nfsmonitor = !helper",
            "[include]\npath = elsewhere",
            "[codex]\nlocalEnvironmentConfigPath=x\n[core]\nworktree=y",
            "[codex]\nlocalEnvironmentConfigPath=x\\\n",
            "[codex]\nlocalEnvironmentConfigPath=x\nlocalEnvironmentConfigPath=y",
            "[codex]\nother=value",
        ] {
            assert!(
                validate_codex_worktree_metadata(value.as_bytes()).is_err(),
                "{value}"
            );
        }
    }
    #[cfg(windows)]
    #[test]
    fn detached_linked_worktree_commit_preserves_parent_and_recovers_interruption() {
        let base = tempfile::tempdir().unwrap();
        let repo = base.path().join("parent");
        let linked = base.path().join("linked");
        let trusted = base.path().join("trusted");
        std::fs::create_dir(&repo).unwrap();
        std::fs::create_dir(&trusted).unwrap();
        let program = PathBuf::from("C:/Program Files/Git/mingw64/bin/git.exe");
        let run = |cwd: &Path, args: &[&str]| {
            let out = std::process::Command::new(&program)
                .args(args)
                .current_dir(cwd)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            String::from_utf8(out.stdout).unwrap().trim().to_owned()
        };
        run(&repo, &["init", "-b", "main"]);
        run(&repo, &["config", "user.name", "Fixture"]);
        run(&repo, &["config", "user.email", "fixture@example.invalid"]);
        std::fs::write(repo.join(".gitattributes"), b"* text eol=lf\n").unwrap();
        std::fs::write(repo.join("selected.txt"), b"old\n").unwrap();
        std::fs::write(repo.join("preserved.txt"), b"old\n").unwrap();
        run(&repo, &["add", "."]);
        run(
            &repo,
            &["-c", "commit.gpgsign=false", "commit", "-m", "baseline"],
        );
        let parent_head = run(&repo, &["rev-parse", "HEAD"]);
        let parent_index = std::fs::read(repo.join(".git/index")).unwrap();
        let parent_ref = std::fs::read(repo.join(".git/refs/heads/main")).unwrap();
        run(&repo, &["config", "extensions.worktreeConfig", "true"]);
        run(
            &repo,
            &[
                "worktree",
                "add",
                "--detach",
                linked.to_str().unwrap(),
                "HEAD",
            ],
        );
        let linked_gitdir = PathBuf::from(run(&linked, &["rev-parse", "--absolute-git-dir"]));
        std::fs::write(
            linked_gitdir.join("config.worktree"),
            b"[codex]\r\n\tlocalEnvironmentConfigPath = __none__\r\n",
        )
        .unwrap();
        std::fs::write(linked.join("selected.txt"), b"selected change\r\n").unwrap();
        std::fs::write(linked.join("preserved.txt"), b"unrelated change\r\n").unwrap();
        let sid = crate::execution_context::execution_context_probe()
            .principal_sid
            .unwrap();
        super::super::native::protect_fixture_directory(&trusted, &sid, "").unwrap();
        let isolation = ProtectedDirectory::open(&trusted, &sid).unwrap();
        std::fs::write(trusted.join("worker.exe"), b"fixture worker").unwrap();
        std::fs::write(trusted.join("client.exe"), b"fixture client").unwrap();
        let installation = Installation {
            schema_version: 1,
            installation_id: "a".repeat(32),
            owner_sid: sid,
            root_identity: isolation.identity().into(),
            worker_sha256: crate::hash::sha256_bytes(b"fixture worker"),
            client_sha256: crate::hash::sha256_bytes(b"fixture client"),
            source_project_id: "b".repeat(32),
        };
        crate::file_io::write_json(&trusted.join("installation.json"), &installation).unwrap();
        let enrolled = enroll(
            &trusted,
            &linked,
            Some((&program, "Fixture", "fixture@example.invalid")),
            true,
            true,
        )
        .expect("detached linked worktree enrollment must succeed without attaching a branch");
        let binding = enrolled.git.as_ref().unwrap();
        assert_eq!(binding.branch_ref, "HEAD");
        let inspector =
            GitInspector::open(&linked, binding, &enrolled.registration, &isolation).unwrap();
        let snapshot = inspector.capture(&enrolled.registration).unwrap();
        let mut request = super::super::tests::request(&enrolled.registration);
        request.source_sha256 = crate::source_fingerprint(&linked).unwrap();
        request.operation = Operation::Commit {
            expected_head: snapshot.head,
            expected_index_sha256: snapshot.index_sha256,
            message: "detached worktree fixture".into(),
            changes: snapshot
                .changes
                .into_iter()
                .filter(|c| c.path == "selected.txt")
                .collect(),
            hook_receipt: None,
        };
        let candidate = inspector
            .prepare_commit(
                &request,
                &enrolled.registration,
                1000,
                "Fixture",
                "fixture@example.invalid",
            )
            .unwrap();
        assert!(
            inspector
                .test_publish_cut(&request, &enrolled.registration, "ref_published")
                .is_err()
        );
        let published = inspector
            .publish_commit(&request, &enrolled.registration, 1000)
            .unwrap();
        assert_eq!(published.commit, candidate.commit);
        assert_eq!(run(&linked, &["rev-parse", "HEAD"]), candidate.commit);
        assert_eq!(run(&repo, &["rev-parse", "HEAD"]), parent_head);
        assert_eq!(
            std::fs::read(repo.join(".git/index")).unwrap(),
            parent_index
        );
        assert_eq!(
            std::fs::read(repo.join(".git/refs/heads/main")).unwrap(),
            parent_ref
        );
        assert_eq!(
            std::fs::read(linked.join("selected.txt")).unwrap(),
            b"selected change\r\n"
        );
        assert_eq!(
            std::fs::read(linked.join("preserved.txt")).unwrap(),
            b"unrelated change\r\n"
        );
        run(&linked, &["switch", "-c", "later-branch"]);
        assert!(inspector.capture(&enrolled.registration).is_err());
    }
    fn with_modify_only_commit_fixture(
        check: impl FnOnce(&GitInspector<'_>, &Request, &Registration, &CommitCandidate, &Path),
    ) {
        let repo = tempfile::tempdir().unwrap();
        let trusted = tempfile::tempdir().unwrap();
        let sid = crate::execution_context::execution_context_probe()
            .principal_sid
            .unwrap();
        super::super::native::protect_fixture_directory(trusted.path(), &sid, "").unwrap();
        let isolation = ProtectedDirectory::open(trusted.path(), &sid).unwrap();
        let program = PathBuf::from("C:/Program Files/Git/mingw64/bin/git.exe");
        let run = |args: &[&str]| {
            let output = std::process::Command::new(&program)
                .args(args)
                .current_dir(repo.path())
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        run(&["init", "-b", "main"]);
        run(&["config", "user.name", "Fixture"]);
        run(&["config", "user.email", "fixture@example.invalid"]);
        std::fs::write(repo.path().join(".gitattributes"), b"* text=auto eol=lf\n").unwrap();
        std::fs::write(repo.path().join("selected.txt"), b"old\n").unwrap();
        std::fs::write(repo.path().join("preserved.txt"), b"old\n").unwrap();
        run(&["add", "."]);
        run(&["-c", "commit.gpgsign=false", "commit", "-m", "fixture"]);
        std::fs::write(repo.path().join("selected.txt"), b"selected change\r\n").unwrap();
        std::fs::write(repo.path().join("preserved.txt"), b"unrelated change\r\n").unwrap();
        let gitdir = repo.path().join(".git");
        let ref_parent = gitdir.join("refs/heads");
        // Fresh seeds inherit Modify, while the original files carry an
        // explicit protected DACL. This forces metadata preservation instead
        // of the already-equal fast path, without impersonating another SID.
        for parent in [&gitdir, &ref_parent] {
            super::super::native::set_fixture_dacl(
                parent,
                &format!("D:P(A;OICI;0x1301bf;;;{sid})"),
            )
            .unwrap();
        }
        for file in [gitdir.join("index"), ref_parent.join("main")] {
            super::super::native::set_fixture_dacl(&file, &format!("D:P(A;;0x1301bf;;;{sid})"))
                .unwrap();
        }
        let mut registration = super::super::tests::registration();
        registration.owner_sid = sid;
        registration.root_identity = super::super::native::SourceDirectory::open(repo.path())
            .unwrap()
            .identity()
            .into();
        registration.git_common_identity = super::super::native::SourceDirectory::open(&gitdir)
            .unwrap()
            .identity()
            .into();
        let binding = GitBinding {
            content_policy: GitContentPolicy::capture(&program, repo.path()).unwrap(),
            hook_policy: GitHookPolicy::capture(&program, repo.path()).unwrap(),
            executable_sha256: crate::hash::sha256_file(&program).unwrap(),
            executable: program,
            git_directory: gitdir.clone(),
            common_directory: gitdir.clone(),
            branch_ref: "refs/heads/main".into(),
            config_sha256: crate::hash::sha256_file(&gitdir.join("config")).unwrap(),
        };
        let git = GitInspector::open(repo.path(), &binding, &registration, &isolation).unwrap();
        let snapshot = git.capture(&registration).unwrap();
        let mut request = super::super::tests::request(&registration);
        request.source_sha256 = crate::source_fingerprint(repo.path()).unwrap();
        request.operation = Operation::Commit {
            expected_head: snapshot.head,
            expected_index_sha256: snapshot.index_sha256,
            message: "metadata policy fixture".into(),
            changes: snapshot
                .changes
                .into_iter()
                .filter(|x| x.path == "selected.txt")
                .collect(),
            hook_receipt: None,
        };
        let candidate = git
            .prepare_commit(
                &request,
                &registration,
                1000,
                "Fixture",
                "fixture@example.invalid",
            )
            .unwrap();
        check(&git, &request, &registration, &candidate, repo.path());
    }

    #[cfg(windows)]
    #[test]
    fn git_publication_preserves_access_under_modify_only_metadata() {
        with_modify_only_commit_fixture(|git, request, registration, candidate, repo| {
            let published = git.publish_commit(request, registration, 1000);
            assert!(
                published.is_ok(),
                "Modify-only metadata publication failed: {published:?}"
            );
            let after = git.capture(registration).unwrap();
            assert_eq!(after.head, candidate.commit);
            assert_eq!(after.index_sha256, candidate.candidate_index_sha256);
            assert_eq!(candidate.paths, vec!["selected.txt"]);
            assert_eq!(
                after
                    .changes
                    .iter()
                    .map(|x| x.path.as_str())
                    .collect::<Vec<_>>(),
                vec!["preserved.txt"]
            );
            assert_eq!(
                std::fs::read(repo.join("selected.txt")).unwrap(),
                b"selected change\r\n"
            );
            assert_eq!(
                std::fs::read(repo.join("preserved.txt")).unwrap(),
                b"unrelated change\r\n"
            );
        });
    }

    #[cfg(windows)]
    #[test]
    fn git_publication_recovers_seed_metadata_object_and_ref_interruptions() {
        for point in [
            "attempt_recorded",
            "index_seed_created",
            "ref_seed_created",
            "seeds_recorded",
            "index_metadata_prepared",
            "ref_metadata_prepared",
            "metadata_recorded",
            "index_lock_claimed",
            "ref_lock_claimed",
            "object_attempt_recorded",
            "object_seed_created",
            "object_seed_recorded",
            "object_published",
            "objects_published",
            "ref_published",
            "index_published",
            "complete",
        ] {
            with_modify_only_commit_fixture(|git, request, registration, candidate, repo| {
                let result = git.publish_commit_inner(request, registration, Some(point));
                assert!(result.is_err(), "fault point was not reached: {point}");
                let mut orphans = Vec::new();
                if matches!(
                    point,
                    "index_seed_created" | "ref_seed_created" | "object_seed_created"
                ) {
                    let mut parents = vec![repo.join(".git"), repo.join(".git/refs/heads")];
                    for entry in std::fs::read_dir(repo.join(".git/objects")).unwrap() {
                        let entry = entry.unwrap();
                        if entry.file_type().unwrap().is_dir()
                            && entry.file_name().to_string_lossy().len() == 2
                        {
                            parents.push(entry.path());
                        }
                    }
                    for parent in parents {
                        for entry in std::fs::read_dir(parent).unwrap() {
                            let entry = entry.unwrap();
                            if entry.file_name().to_string_lossy().starts_with(".rayman-") {
                                let path = entry.path();
                                let identity = super::super::publication::identity(
                                    &std::fs::File::open(&path).unwrap(),
                                    &path,
                                )
                                .unwrap();
                                orphans.push((
                                    path.clone(),
                                    identity,
                                    std::fs::read(path).unwrap(),
                                ));
                            }
                        }
                    }
                    assert!(!orphans.is_empty(), "no unjournaled seed at {point}");
                }
                let recovered = git
                    .recover_admitted_commit(request, registration)
                    .unwrap_or_else(|e| panic!("recovery failed at {point}: {e:#}"));
                assert_eq!(recovered.commit, candidate.commit);
                let after = git.capture(registration).unwrap();
                assert_eq!(after.head, candidate.commit);
                assert_eq!(after.index_sha256, candidate.candidate_index_sha256);
                assert_eq!(
                    after
                        .changes
                        .iter()
                        .map(|x| x.path.as_str())
                        .collect::<Vec<_>>(),
                    vec!["preserved.txt"]
                );
                assert!(!repo.join(".git/index.lock").exists());
                assert!(!repo.join(".git/refs/heads/main.lock").exists());
                assert_eq!(
                    std::fs::read(repo.join("selected.txt")).unwrap(),
                    b"selected change\r\n"
                );
                assert_eq!(
                    std::fs::read(repo.join("preserved.txt")).unwrap(),
                    b"unrelated change\r\n"
                );
                for (path, identity, bytes) in orphans {
                    assert_eq!(
                        std::fs::read(&path).unwrap(),
                        bytes,
                        "unknown seed changed at {point}"
                    );
                    assert_eq!(
                        super::super::publication::identity(
                            &std::fs::File::open(&path).unwrap(),
                            &path
                        )
                        .unwrap(),
                        identity,
                        "unknown seed adopted at {point}"
                    );
                }
            });
        }
    }

    #[cfg(windows)]
    #[test]
    fn git_recovery_refuses_same_bytes_with_a_replaced_seed_identity() {
        with_modify_only_commit_fixture(|git, request, registration, candidate, repo| {
            assert!(
                git.publish_commit_inner(request, registration, Some("seeds_recorded"))
                    .is_err()
            );
            let journal = git
                .isolation
                .join(format!("candidate-{}/publication.json", request.request_id));
            let record: PublicationRecord = crate::file_io::read_json(&journal).unwrap().unwrap();
            let seed = repo.join(".git").join(record.index_seed);
            let bytes = std::fs::read(&seed).unwrap();
            std::fs::remove_file(&seed).unwrap();
            std::fs::write(&seed, bytes).unwrap();
            let replacement =
                super::super::publication::identity(&std::fs::File::open(&seed).unwrap(), &seed)
                    .unwrap();
            assert_ne!(replacement, record.index_identity);
            assert!(git.recover_admitted_commit(request, registration).is_err());
            let current = git.capture(registration).unwrap();
            assert_eq!(current.head, candidate.parent);
            assert_eq!(current.index_sha256, candidate.original_index_sha256);
        });
    }

    #[cfg(windows)]
    #[test]
    fn git_recovery_preserves_a_foreign_standard_lock() {
        with_modify_only_commit_fixture(|git, request, registration, candidate, repo| {
            let lock = repo.join(".git/index.lock");
            std::fs::write(&lock, b"another Git writer owns this lock").unwrap();
            assert!(git.publish_commit(request, registration, 1000).is_err());
            assert_eq!(
                std::fs::read(&lock).unwrap(),
                b"another Git writer owns this lock"
            );
            assert_eq!(git.capture(registration).unwrap().head, candidate.parent);
            // The fixture's other writer releases its own lock. The publisher
            // must never perform this deletion on its behalf.
            std::fs::remove_file(lock).unwrap();
            assert_eq!(
                git.recover_admitted_commit(request, registration)
                    .unwrap()
                    .commit,
                candidate.commit
            );
        });
    }

    #[cfg(windows)]
    #[test]
    fn git_recovery_reads_legacy_post_metadata_journals() {
        with_modify_only_commit_fixture(|git, request, registration, candidate, _| {
            assert!(
                git.publish_commit_inner(request, registration, Some("metadata_recorded"))
                    .is_err()
            );
            let journal = git
                .isolation
                .join(format!("candidate-{}/publication.json", request.request_id));
            let mut legacy: serde_json::Value =
                crate::file_io::read_json(&journal).unwrap().unwrap();
            for key in [
                "version",
                "metadata_ready",
                "object_seeds",
                "object_attempts",
            ] {
                legacy.as_object_mut().unwrap().remove(key);
            }
            crate::file_io::write_json(&journal, &legacy).unwrap();
            assert_eq!(
                git.recover_admitted_commit(request, registration)
                    .unwrap()
                    .commit,
                candidate.commit
            );
        });
    }

    #[cfg(windows)]
    #[test]
    fn git_recovery_bounds_unjournaled_staging_attempts() {
        with_modify_only_commit_fixture(|git, request, registration, candidate, _| {
            for _ in 0..8 {
                assert!(
                    git.publish_commit_inner(request, registration, Some("index_seed_created"))
                        .is_err()
                );
            }
            let error = git
                .recover_admitted_commit(request, registration)
                .unwrap_err();
            assert!(error.to_string().contains("retry limit reached"));
            let current = git.capture(registration).unwrap();
            assert_eq!(current.head, candidate.parent);
            assert_eq!(current.index_sha256, candidate.original_index_sha256);
        });
    }

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
        std::fs::create_dir(repo.path().join(".RaymanCodingSkill")).unwrap();
        std::fs::write(
            repo.path().join(".RaymanCodingSkill/workspace_skill.yaml"),
            b"original state\n",
        )
        .unwrap();
        std::fs::create_dir(repo.path().join(".agent-checkpoints")).unwrap();
        std::fs::write(
            repo.path().join(".agent-checkpoints/tracked-state"),
            b"checkpoint state\n",
        )
        .unwrap();
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
        std::fs::write(
            repo.path().join(".RaymanCodingSkill/workspace_skill.yaml"),
            b"preserved dirty state\n",
        )
        .unwrap();
        std::fs::write(
            repo.path().join(".agent-checkpoints/untracked-lock"),
            b"preserved lock\n",
        )
        .unwrap();
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
        assert_eq!(
            std::fs::read(repo.path().join(".RaymanCodingSkill/workspace_skill.yaml")).unwrap(),
            b"preserved dirty state\n"
        );
        assert_eq!(
            std::fs::read(repo.path().join(".agent-checkpoints/untracked-lock")).unwrap(),
            b"preserved lock\n"
        );
        assert!(validate_relative_path(".RaymanCodingSkill/workspace_skill.yaml").is_err());
        assert!(validate_relative_path(".agent-checkpoints/tracked-state").is_err());

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
