//! Safe workspace-local state paths.
//!
//! All managed state lives below `.RaymanCodingSkill/`.  A normal `create_dir_all`
//! or `exists` call follows an ancestor symlink/junction before the caller gets a
//! chance to inspect the final path, which can turn a workspace-local write or
//! cleanup into an external one.  These helpers canonicalize the workspace root
//! once and then create/check every managed-state component without following a
//! symlink or Windows reparse point below that root.

use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use crate::file_io::is_link_or_reparse;
// Managed-state paths are canonicalized, so raw `display()` here leaked the
// Windows `\\?\` verbatim prefix into the write probe and into every diagnostic
// — the prefix the rest of the codebase strips by rule.
use crate::pathfmt::display_path;
use anyhow::{Context, Result, bail};

pub const STATE_DIR_NAME: &str = ".RaymanCodingSkill";

/// Return a verified managed-state root, creating it only when requested.
pub fn managed_state_root(root: &Path, create: bool) -> Result<Option<PathBuf>> {
    let workspace = canonical_workspace_root(root)?;
    let state = workspace.join(STATE_DIR_NAME);
    match fs::symlink_metadata(&state) {
        Ok(metadata) => {
            ensure_real_directory_metadata(&state, &metadata)?;
            ensure_real_directory_within(&workspace, &state)?;
            Ok(Some(state))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound && !create => Ok(None),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            create_real_directory(&state)?;
            ensure_real_directory_within(&workspace, &state)?;
            Ok(Some(state))
        }
        Err(error) => {
            Err(error).with_context(|| format!("无法读取受管状态根: {}", display_path(&state)))
        }
    }
}

/// Return a verified managed-state directory.  `relative` is relative to
/// `.RaymanCodingSkill`; if it is absent and `create` is false, return `None`.
pub fn managed_state_dir(root: &Path, relative: &Path, create: bool) -> Result<Option<PathBuf>> {
    let workspace = canonical_workspace_root(root)?;
    let Some(mut current) = managed_state_root(root, create)? else {
        return Ok(None);
    };
    for component in normal_components(relative)? {
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(metadata) => {
                ensure_real_directory_metadata(&current, &metadata)?;
                // `symlink_metadata(current)` only sees the final component.
                // Re-canonicalize after every step so an ancestor swapped to a
                // link between iterations cannot redirect the next child into
                // a directory outside this workspace.
                ensure_real_directory_within(&workspace, &current)?;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound && !create => return Ok(None),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                create_real_directory(&current)?;
                ensure_real_directory_within(&workspace, &current)?;
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("无法读取受管状态目录: {}", display_path(&current)));
            }
        }
    }
    Ok(Some(current))
}

/// Return a verified managed-state file path.  Existing files must not be a
/// symlink/reparse point.  Missing parent directories are created only when
/// `create_parents` is true.
pub fn managed_state_file(root: &Path, relative: &Path, create_parents: bool) -> Result<PathBuf> {
    let workspace = canonical_workspace_root(root)?;
    let components = normal_components(relative)?;
    let (name, parents) = components
        .split_last()
        .ok_or_else(|| anyhow::anyhow!("受管状态文件路径不能为空"))?;
    let parent_relative = parents.iter().fold(PathBuf::new(), |mut path, component| {
        path.push(*component);
        path
    });
    let parent = match managed_state_dir(root, &parent_relative, create_parents)? {
        Some(parent) => parent,
        None => canonical_workspace_root(root)?
            .join(STATE_DIR_NAME)
            .join(relative),
    };
    if parent.ends_with(relative) {
        // A missing ancestor is safe to represent lexically for a read; read_json
        // will report it as absent.  It must not be used as a writable parent.
        return Ok(parent);
    }
    ensure_real_directory_within(&workspace, &parent)?;
    let path = parent.join(*name);
    match fs::symlink_metadata(&path) {
        Ok(metadata) if is_link_or_reparse(&metadata) => {
            bail!("拒绝链接/reparse 受管状态文件: {}", display_path(&path));
        }
        Ok(metadata) if !metadata.file_type().is_file() => {
            bail!("受管状态路径不是普通文件: {}", display_path(&path));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error).with_context(|| format!("无法读取受管状态文件: {}", display_path(&path)));
        }
    }
    Ok(path)
}

/// Check an already-created directory immediately before a state transaction.
pub fn ensure_real_directory(path: &Path) -> Result<()> {
    crate::file_io::ensure_real_directory_labeled(path, "受管状态目录")
}

/// Result of the non-destructive state-write capability probe.
#[derive(Debug, serde::Serialize)]
pub struct StateWriteProbe {
    pub state_dir_present: bool,
    pub probed: bool,
    pub writable: bool,
    pub path: Option<String>,
    pub error: Option<String>,
}

/// Probe whether this process can write workspace state right now.
///
/// Restricted host sandboxes deny `.RaymanCodingSkill/` lock and state writes
/// with ACL errors that otherwise surface only mid-transaction. The probe
/// writes and removes one transient file inside an existing state root; a
/// workspace without a state root is reported unprobed instead of mutated.
pub fn state_write_probe(root: &Path) -> StateWriteProbe {
    match managed_state_root(root, false) {
        Ok(Some(_)) => {}
        Ok(None) => {
            return StateWriteProbe {
                state_dir_present: false,
                probed: false,
                writable: false,
                path: None,
                error: None,
            };
        }
        Err(error) => {
            return StateWriteProbe {
                state_dir_present: true,
                probed: false,
                writable: false,
                path: None,
                error: Some(format!("{error:#}")),
            };
        }
    }
    // Probe inside the managed `tmp` entry: `state audit` allows it, so a
    // probe file leaked by a failed cleanup can never fail that gate, and the
    // verified directory chain rejects link/reparse ancestors.
    let tmp = match managed_state_dir(root, Path::new("tmp"), true) {
        Ok(Some(tmp)) => tmp,
        Ok(None) => {
            return StateWriteProbe {
                state_dir_present: true,
                probed: true,
                writable: false,
                path: None,
                error: Some("managed tmp directory unavailable".into()),
            };
        }
        Err(error) => {
            return StateWriteProbe {
                state_dir_present: true,
                probed: true,
                writable: false,
                path: None,
                error: Some(format!("{error:#}")),
            };
        }
    };
    // The probe file name stays unique per process to avoid concurrent
    // clobbering; the reported path is the stable directory so identical
    // inspections stay byte-identical across runs and languages.
    //
    // `create_new` maps to POSIX `O_CREAT|O_EXCL`, which refuses a pre-planted
    // symlink at the final component even when it dangles. Win32 `CREATE_NEW`
    // gives no such guarantee — it resolves a reparse point first and can fail
    // with ALREADY_EXISTS pointing elsewhere. That is why the probe is only a
    // diagnostic: the authority for state-path safety is `managed_state_*`,
    // which verifies links and reparse points explicitly.
    let probe = tmp.join(format!(".rayman-state-probe-{}.tmp", std::process::id()));
    let write = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
        .and_then(|mut file| {
            use std::io::Write;
            file.write_all(b"rayman-state-probe")
        });
    match write {
        Ok(()) => {
            let cleanup = fs::remove_file(&probe);
            StateWriteProbe {
                state_dir_present: true,
                probed: true,
                writable: true,
                path: Some(display_path(&tmp)),
                error: cleanup.err().map(|error| error.to_string()),
            }
        }
        Err(error) => StateWriteProbe {
            state_dir_present: true,
            probed: true,
            writable: false,
            path: Some(display_path(&tmp)),
            error: Some(error.to_string()),
        },
    }
}

fn canonical_workspace_root(root: &Path) -> Result<PathBuf> {
    let workspace = root
        .canonicalize()
        .with_context(|| format!("无法规范化工作区根: {}", display_path(root)))?;
    ensure_real_directory(&workspace)?;
    Ok(workspace)
}

/// Confirm a real directory is still rooted in this workspace.  Checking only
/// the final component is insufficient after an attacker swaps an already
/// checked ancestor for a link: `symlink_metadata(child)` would then report a
/// perfectly ordinary external child.  Canonicalization turns that redirection
/// into an explicit escape instead.
fn ensure_real_directory_within(workspace: &Path, path: &Path) -> Result<()> {
    ensure_real_directory(path)?;
    let canonical = path
        .canonicalize()
        .with_context(|| format!("无法规范化受管状态目录: {}", display_path(path)))?;
    if !canonical.starts_with(workspace) {
        bail!(
            "受管状态目录逃逸工作区: {} -> {} (工作区: {})",
            display_path(path),
            display_path(&canonical),
            display_path(workspace)
        );
    }
    Ok(())
}

fn normal_components(relative: &Path) -> Result<Vec<&std::ffi::OsStr>> {
    let mut components = Vec::new();
    for component in relative.components() {
        let Component::Normal(part) = component else {
            bail!("不安全的受管状态相对路径: {}", display_path(relative));
        };
        components.push(part);
    }
    Ok(components)
}

fn create_real_directory(path: &Path) -> Result<()> {
    match fs::create_dir(path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => {
            return Err(error).with_context(|| format!("无法创建受管状态目录: {}", display_path(path)));
        }
    }
    ensure_real_directory(path)
}

fn ensure_real_directory_metadata(path: &Path, metadata: &fs::Metadata) -> Result<()> {
    if is_link_or_reparse(metadata) {
        bail!("拒绝链接/reparse 受管状态目录: {}", display_path(path));
    }
    if !metadata.file_type().is_dir() {
        bail!("受管状态路径不是目录: {}", display_path(path));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_and_returns_verified_state_paths() {
        let workspace = tempfile::tempdir().unwrap();
        let path =
            managed_state_file(workspace.path(), Path::new("context/index.json"), true).unwrap();
        assert!(path.ends_with(".RaymanCodingSkill/context/index.json"));
        assert!(path.parent().unwrap().is_dir());
    }

    #[test]
    fn state_write_probe_reports_a_writable_state_root_and_cleans_up() {
        let workspace = tempfile::tempdir().unwrap();
        fs::create_dir(workspace.path().join(STATE_DIR_NAME)).unwrap();

        let probe = state_write_probe(workspace.path());
        assert!(probe.state_dir_present);
        assert!(probe.probed);
        assert!(probe.writable);
        assert!(probe.error.is_none());
        // The probe works inside the state-audit-allowed `tmp` entry and must
        // leave it empty; nothing else may appear at the state root.
        let tmp = workspace.path().join(STATE_DIR_NAME).join("tmp");
        assert!(fs::read_dir(&tmp).unwrap().next().is_none());
        let root_entries: Vec<String> = fs::read_dir(workspace.path().join(STATE_DIR_NAME))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(root_entries, ["tmp"]);
    }

    #[cfg(unix)]
    #[test]
    fn state_write_probe_refuses_a_planted_link_at_the_probe_path() {
        use std::os::unix::fs::symlink;

        let workspace = tempfile::tempdir().unwrap();
        let tmp = workspace.path().join(STATE_DIR_NAME).join("tmp");
        fs::create_dir_all(&tmp).unwrap();
        let outside = tempfile::tempdir().unwrap();
        let victim = outside.path().join("victim.txt");
        fs::write(&victim, "user data").unwrap();
        symlink(
            &victim,
            tmp.join(format!(".rayman-state-probe-{}.tmp", std::process::id())),
        )
        .unwrap();

        let probe = state_write_probe(workspace.path());
        assert!(probe.probed);
        assert!(!probe.writable, "planted link must fail the probe closed");
        assert!(probe.error.is_some());
        assert_eq!(fs::read(&victim).unwrap(), b"user data");
    }

    #[test]
    fn state_write_probe_skips_a_workspace_without_a_state_root() {
        let workspace = tempfile::tempdir().unwrap();

        let probe = state_write_probe(workspace.path());
        assert!(!probe.state_dir_present);
        assert!(!probe.probed);
        assert!(!probe.writable);
        assert!(!workspace.path().join(STATE_DIR_NAME).exists());
    }

    #[cfg(unix)]
    #[test]
    fn state_write_probe_reports_a_denied_state_root_without_failing() {
        use std::os::unix::fs::PermissionsExt;

        let workspace = tempfile::tempdir().unwrap();
        let state = workspace.path().join(STATE_DIR_NAME);
        fs::create_dir(&state).unwrap();
        fs::set_permissions(&state, fs::Permissions::from_mode(0o555)).unwrap();

        let probe = state_write_probe(workspace.path());
        fs::set_permissions(&state, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(probe.state_dir_present);
        assert!(probe.probed);
        assert!(!probe.writable);
        assert!(probe.error.is_some());
    }

    #[test]
    fn rejects_a_non_directory_state_root_without_treating_it_as_missing() {
        let workspace = tempfile::tempdir().unwrap();
        fs::write(workspace.path().join(STATE_DIR_NAME), "not a directory").unwrap();
        assert!(managed_state_file(workspace.path(), Path::new("pending.json"), true).is_err());
        assert!(managed_state_root(workspace.path(), false).is_err());
    }

    #[test]
    fn rejects_a_directory_where_a_managed_state_file_is_expected() {
        let workspace = tempfile::tempdir().unwrap();
        let path = workspace.path().join(STATE_DIR_NAME).join("pending.json");
        fs::create_dir_all(&path).unwrap();

        assert!(managed_state_file(workspace.path(), Path::new("pending.json"), false).is_err());
    }

    #[test]
    fn verified_directory_must_stay_under_the_workspace_root() {
        let workspace = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let canonical_workspace = canonical_workspace_root(workspace.path()).unwrap();

        assert!(ensure_real_directory_within(&canonical_workspace, outside.path()).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_a_symlinked_state_ancestor_before_creating_children() {
        use std::os::unix::fs::symlink;

        let workspace = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        symlink(outside.path(), workspace.path().join(STATE_DIR_NAME)).unwrap();

        assert!(managed_state_file(workspace.path(), Path::new("tmp/new/file"), true).is_err());
        assert!(!outside.path().join("tmp/new/file").exists());
    }
}
