//! Bind the user's effective content conversion settings without letting Git
//! spawn helpers in the desktop-user worker.
use super::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
#[cfg(windows)]
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitContentPolicy {
    pub environment: BTreeMap<String, String>,
    pub settings: BTreeMap<String, String>,
}
impl GitContentPolicy {
    #[cfg(windows)]
    pub fn capture(program: &Path, workspace: &Path) -> Result<Self> {
        let mut environment = BTreeMap::new();
        for key in [
            "SystemRoot",
            "USERPROFILE",
            "HOME",
            "HOMEDRIVE",
            "HOMEPATH",
            "XDG_CONFIG_HOME",
            "GIT_CONFIG_GLOBAL",
            "GIT_CONFIG_SYSTEM",
            "GIT_CONFIG_NOSYSTEM",
        ] {
            if let Ok(value) = std::env::var(key) {
                environment.insert(key.into(), value);
            }
        }
        Self::read(program, workspace, environment)
    }
    #[cfg(windows)]
    pub fn verify(&self, program: &Path, workspace: &Path) -> Result<()> {
        if Self::read(program, workspace, self.environment.clone())? != *self {
            bail!(
                "registered Git content settings changed; refresh owner enrollment before committing"
            );
        }
        Ok(())
    }
    #[cfg(windows)]
    fn read(
        program: &Path,
        workspace: &Path,
        mut environment: BTreeMap<String, String>,
    ) -> Result<Self> {
        let allowed = [
            "SystemRoot",
            "USERPROFILE",
            "HOME",
            "HOMEDRIVE",
            "HOMEPATH",
            "XDG_CONFIG_HOME",
            "GIT_CONFIG_GLOBAL",
            "GIT_CONFIG_SYSTEM",
            "GIT_CONFIG_NOSYSTEM",
        ];
        if environment
            .keys()
            .any(|key| !allowed.contains(&key.as_str()))
        {
            bail!("unsupported Git policy environment");
        }
        let preserved = environment.clone();
        environment.insert("GIT_CONFIG_COUNT".into(), "0".into());
        environment.insert("GIT_TERMINAL_PROMPT".into(), "0".into());
        environment.insert("PATH".into(), String::new());
        let args=vec!["-c".into(),format!("safe.directory={}",crate::pathfmt::display_path(workspace)),"config".into(),"--no-includes".into(),"--null".into(),"--get-regexp".into(),"^(core\\.(autocrlf|eol|safecrlf|attributesfile)|commit\\.gpgsign|i18n\\.commitencoding|include(if\\..*)?\\.path)$".into()];
        let result = super::process::run_single_process(
            program,
            &args,
            workspace,
            &environment,
            b"",
            10000,
            65536,
        )?;
        if !matches!(result.exit_code, 0 | 1) {
            bail!(
                "cannot read effective Git content settings: {}",
                String::from_utf8_lossy(&result.stderr)
            );
        }
        let settings = Self::parse(&result.stdout)?;
        Ok(Self {
            environment: preserved,
            settings,
        })
    }
    #[cfg(any(windows, test))]
    fn parse(bytes: &[u8]) -> Result<BTreeMap<String, String>> {
        let mut settings = BTreeMap::new();
        for item in bytes.split(|b| *b == 0).filter(|item| !item.is_empty()) {
            let item = std::str::from_utf8(item)?;
            let (key, value) = item
                .split_once('\n')
                .ok_or_else(|| anyhow::anyhow!("malformed Git policy output"))?;
            let value = value.trim().to_ascii_lowercase();
            let valid = match key {
                "core.autocrlf" => matches!(value.as_str(), "true" | "false" | "input"),
                "core.eol" => matches!(value.as_str(), "lf" | "crlf" | "native"),
                "core.safecrlf" => matches!(value.as_str(), "true" | "false" | "warn"),
                "commit.gpgsign" => matches!(value.as_str(), "false" | "no" | "off" | "0"),
                "i18n.commitencoding" => matches!(value.as_str(), "utf-8" | "utf8"),
                _ => false,
            };
            if !valid {
                bail!(
                    "Git policy {key} needs a dedicated adapter; it will not be silently ignored"
                );
            }
            settings.insert(key.into(), value);
        }
        Ok(settings)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitHookPolicy {
    pub relative_root: String,
    pub tree_sha256: String,
}

#[cfg(windows)]
fn hook_config_command(program: &Path, workspace: &Path, local: bool) -> std::process::Command {
    let mut command = std::process::Command::new(program);
    command
        .arg("-c")
        .arg(format!(
            "safe.directory={}",
            crate::pathfmt::display_path(workspace)
        ))
        .arg("config");
    if local {
        command.arg("--local");
    }
    command
        .args(["--get-all", "core.hooksPath"])
        .current_dir(workspace)
        .env("GIT_TERMINAL_PROMPT", "0");
    command
}

#[cfg(windows)]
impl GitHookPolicy {
    pub fn capture(program: &Path, workspace: &Path) -> Result<Option<Self>> {
        let local_output = hook_config_command(program, workspace, true).output()?;
        if !local_output.status.success() && local_output.status.code() != Some(1) {
            bail!("cannot inspect repository hook policy");
        }
        let effective_output = hook_config_command(program, workspace, false).output()?;
        if !effective_output.status.success() && effective_output.status.code() != Some(1) {
            bail!("cannot inspect effective repository hook policy");
        }
        if local_output.stdout != effective_output.stdout {
            bail!("effective hook path is not owned by local repository config");
        }
        if local_output.status.code() == Some(1) && local_output.stdout.is_empty() {
            return Ok(None);
        }
        let text = std::str::from_utf8(&local_output.stdout)?;
        let values: Vec<_> = text.lines().collect();
        if values.len() != 1 {
            bail!("repository hook path must have exactly one local value");
        }
        validate_relative_path(values[0])?;
        let root = workspace.join(values[0]);
        if !root.is_dir() {
            bail!("configured repository hook directory is missing");
        }
        let policy = Self {
            relative_root: values[0].replace('\\', "/"),
            tree_sha256: hook_tree_sha256(&root)?,
        };
        if !root.join("pre-commit").is_file() {
            bail!("configured repository hook directory has no pre-commit hook");
        }
        Ok(Some(policy))
    }

    pub fn verify(&self, workspace: &Path) -> Result<std::path::PathBuf> {
        validate_relative_path(&self.relative_root)?;
        let root = workspace.join(&self.relative_root);
        if hook_tree_sha256(&root)? != self.tree_sha256 {
            bail!("repository hook bytes changed; refresh owner enrollment");
        }
        Ok(root)
    }

    pub fn digest(&self) -> Result<String> {
        Ok(crate::hash::sha256_bytes(&serde_json::to_vec(self)?))
    }
}

#[cfg(windows)]
fn hook_tree_sha256(root: &Path) -> Result<String> {
    let pin = super::native::SourceDirectory::open(root)?;
    let mut files = BTreeMap::new();
    for entry in ignore::WalkBuilder::new(pin.path())
        .hidden(false)
        .follow_links(false)
        .build()
    {
        let entry = entry?;
        if entry.depth() == 0 {
            continue;
        }
        let metadata = std::fs::symlink_metadata(entry.path())?;
        if crate::file_io::is_link_or_reparse(&metadata) {
            bail!("repository hook tree contains a link or reparse point");
        }
        if metadata.is_dir() {
            continue;
        }
        if !metadata.is_file() || metadata.len() > 16 * 1024 * 1024 {
            bail!("repository hook entry is not a bounded ordinary file");
        }
        let relative = entry
            .path()
            .strip_prefix(pin.path())?
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("repository hook path is not Unicode"))?
            .replace('\\', "/");
        files.insert(
            relative.clone(),
            crate::hash::sha256_bytes(&pin.read_file(Path::new(&relative), 16 * 1024 * 1024)?),
        );
        if files.len() > 128 {
            bail!("repository hook tree exceeds entry limit");
        }
    }
    Ok(crate::hash::sha256_bytes(&serde_json::to_vec(&files)?))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn git_content_policy_preserves_precedence_and_rejects_silent_signing_changes() {
        let parsed = GitContentPolicy::parse(
            b"core.autocrlf\ntrue\0core.eol\nlf\0core.autocrlf\ninput\0core.safecrlf\nwarn\0",
        )
        .unwrap();
        assert_eq!(parsed["core.autocrlf"], "input");
        assert_eq!(parsed["core.eol"], "lf");
        assert!(GitContentPolicy::parse(b"commit.gpgsign\ntrue\0").is_err());
        assert!(GitContentPolicy::parse(b"include.path\nother.conf\0").is_err());
        assert!(GitContentPolicy::parse(b"core.autocrlf\nbogus\0").is_err());
    }
    #[cfg(windows)]
    #[test]
    fn global_autocrlf_matches_native_git_without_changing_worktree_bytes() {
        let project = tempfile::tempdir().unwrap();
        let trusted = tempfile::tempdir().unwrap();
        let profile = tempfile::tempdir().unwrap();
        let config = profile.path().join("gitconfig");
        std::fs::write(&config, b"[core]\nautocrlf = true\n").unwrap();
        let program = std::path::PathBuf::from("C:/Program Files/Git/mingw64/bin/git.exe");
        let env = BTreeMap::from([
            ("GIT_CONFIG_GLOBAL".into(), config.to_string_lossy().into()),
            ("GIT_CONFIG_SYSTEM".into(), "NUL".into()),
            ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
            (
                "USERPROFILE".into(),
                profile.path().to_string_lossy().into(),
            ),
            ("HOME".into(), profile.path().to_string_lossy().into()),
            ("SystemRoot".into(), std::env::var("SystemRoot").unwrap()),
        ]);
        let git = |args: &[&str]| {
            let out = std::process::Command::new(&program)
                .args(args)
                .current_dir(project.path())
                .envs(&env)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            out.stdout
        };
        git(&["init", "-b", "main"]);
        git(&["config", "user.name", "Fixture"]);
        git(&["config", "user.email", "fixture@example.invalid"]);
        std::fs::write(project.path().join("file.txt"), b"old\r\n").unwrap();
        git(&["add", "file.txt"]);
        git(&["-c", "commit.gpgsign=false", "commit", "-m", "base"]);
        std::fs::write(project.path().join("file.txt"), b"new\r\n").unwrap();
        let native =
            String::from_utf8(git(&["hash-object", "--path=file.txt", "file.txt"])).unwrap();
        let sid = crate::execution_context::execution_context_probe()
            .principal_sid
            .unwrap();
        super::super::native::protect_fixture_directory(trusted.path(), &sid, "").unwrap();
        let store = ProtectedDirectory::open(trusted.path(), &sid).unwrap();
        let dir = project.path().join(".git");
        let mut r = super::super::tests::registration();
        r.owner_sid = sid;
        r.root_identity = super::super::native::SourceDirectory::open(project.path())
            .unwrap()
            .identity()
            .into();
        r.git_common_identity = super::super::native::SourceDirectory::open(&dir)
            .unwrap()
            .identity()
            .into();
        let policy = GitContentPolicy::read(&program, project.path(), env).unwrap();
        assert_eq!(policy.settings["core.autocrlf"], "true");
        let binding = GitBinding {
            content_policy: policy,
            hook_policy: None,
            executable: program.clone(),
            executable_sha256: crate::hash::sha256_file(&program).unwrap(),
            git_directory: dir.clone(),
            common_directory: dir.clone(),
            branch_ref: "refs/heads/main".into(),
            config_sha256: crate::hash::sha256_file(&dir.join("config")).unwrap(),
        };
        let reader = GitInspector::open(project.path(), &binding, &r, &store).unwrap();
        let snapshot = reader.capture(&r).unwrap();
        assert_eq!(
            snapshot.changes[0].after.as_ref().unwrap().git_blob_oid,
            native.trim()
        );
        assert_eq!(
            std::fs::read(project.path().join("file.txt")).unwrap(),
            b"new\r\n"
        );
        drop(reader);
        std::fs::write(config, b"[core]\nautocrlf = false\n").unwrap();
        assert!(GitInspector::open_for_worker(project.path(), &binding, &r, &store).is_err());
    }
}

#[cfg(all(test, windows))]
mod ownership_tests {
    use super::*;
    #[cfg(windows)]
    #[test]
    fn hook_config_trust_is_scoped_to_the_requested_repository() {
        let repo = tempfile::tempdir().unwrap();
        let git = Path::new("C:/Program Files/Git/mingw64/bin/git.exe");
        assert!(
            std::process::Command::new(git)
                .args(["init", "-b", "main"])
                .current_dir(repo.path())
                .output()
                .unwrap()
                .status
                .success()
        );
        let baseline = std::process::Command::new(git)
            .args([
                "-c",
                "safe.directory=",
                "config",
                "--local",
                "--get-all",
                "core.hooksPath",
            ])
            .env("GIT_TEST_ASSUME_DIFFERENT_OWNER", "1")
            .current_dir(repo.path())
            .output()
            .unwrap();
        assert!(!matches!(baseline.status.code(), Some(0 | 1)));
        for local in [true, false] {
            let result = hook_config_command(git, repo.path(), local)
                .env("GIT_TEST_ASSUME_DIFFERENT_OWNER", "1")
                .output()
                .unwrap();
            assert_eq!(
                result.status.code(),
                Some(1),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
        }
        assert!(!repo.path().join(".git/index.lock").exists());
    }
}
