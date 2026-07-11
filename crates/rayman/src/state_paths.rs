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
            Err(error).with_context(|| format!("无法读取受管状态根: {}", state.display()))
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
                    .with_context(|| format!("无法读取受管状态目录: {}", current.display()));
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
            bail!("拒绝链接/reparse 受管状态文件: {}", path.display());
        }
        Ok(metadata) if !metadata.file_type().is_file() => {
            bail!("受管状态路径不是普通文件: {}", path.display());
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error).with_context(|| format!("无法读取受管状态文件: {}", path.display()));
        }
    }
    Ok(path)
}

/// Check an already-created directory immediately before a state transaction.
pub fn ensure_real_directory(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("无法读取受管状态目录元数据: {}", path.display()))?;
    ensure_real_directory_metadata(path, &metadata)
}

fn canonical_workspace_root(root: &Path) -> Result<PathBuf> {
    let workspace = root
        .canonicalize()
        .with_context(|| format!("无法规范化工作区根: {}", root.display()))?;
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
        .with_context(|| format!("无法规范化受管状态目录: {}", path.display()))?;
    if !canonical.starts_with(workspace) {
        bail!(
            "受管状态目录逃逸工作区: {} -> {} (工作区: {})",
            path.display(),
            canonical.display(),
            workspace.display()
        );
    }
    Ok(())
}

fn normal_components(relative: &Path) -> Result<Vec<&std::ffi::OsStr>> {
    let mut components = Vec::new();
    for component in relative.components() {
        let Component::Normal(part) = component else {
            bail!("不安全的受管状态相对路径: {}", relative.display());
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
            return Err(error).with_context(|| format!("无法创建受管状态目录: {}", path.display()));
        }
    }
    ensure_real_directory(path)
}

fn ensure_real_directory_metadata(path: &Path, metadata: &fs::Metadata) -> Result<()> {
    if is_link_or_reparse(metadata) {
        bail!("拒绝链接/reparse 受管状态目录: {}", path.display());
    }
    if !metadata.file_type().is_dir() {
        bail!("受管状态路径不是目录: {}", path.display());
    }
    Ok(())
}

fn is_link_or_reparse(metadata: &fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    false
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
