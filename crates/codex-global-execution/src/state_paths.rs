use anyhow::{Result, bail};
use std::path::{Component, Path, PathBuf};

pub(crate) fn ensure_real_directory(path: &Path) -> Result<()> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut current = PathBuf::new();
    let mut root = false;
    for component in path.components() {
        if matches!(component, Component::ParentDir) {
            bail!("parent traversal is not allowed in state paths");
        }
        current.push(component);
        root |= matches!(component, Component::RootDir);
        if root {
            let metadata = std::fs::symlink_metadata(&current)?;
            if !metadata.is_dir() || crate::file_io::is_link_or_reparse(&metadata) {
                bail!("state ancestor is not an ordinary directory");
            }
        }
    }
    Ok(())
}

#[derive(Debug, serde::Serialize)]
pub struct StateWriteProbe {
    pub state_dir_present: bool,
    pub probed: bool,
    pub writable: bool,
    pub path: Option<String>,
    pub error: Option<String>,
}

pub fn state_write_probe_not_requested(root: &Path) -> StateWriteProbe {
    let state = root.join(".RaymanCodingSkill");
    let present = state.exists();
    let error = if present {
        ensure_real_directory(&state).err().map(|e| e.to_string())
    } else {
        None
    };
    StateWriteProbe {
        state_dir_present: present,
        probed: false,
        writable: false,
        path: None,
        error,
    }
}
pub fn state_write_probe(root: &Path) -> StateWriteProbe {
    use std::io::Write;
    let mut report = state_write_probe_not_requested(root);
    if !report.state_dir_present || report.error.is_some() {
        return report;
    }
    let temp = root.join(".RaymanCodingSkill/tmp");
    report.path = Some(crate::pathfmt::display_path(&temp));
    report.probed = true;
    let result = (|| -> Result<()> {
        match std::fs::create_dir(&temp) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.into()),
        };
        ensure_real_directory(&temp)?;
        let mut random = [0u8; 16];
        getrandom::fill(&mut random).map_err(|e| anyhow::anyhow!("probe entropy failed: {e}"))?;
        let suffix: String = random.iter().map(|b| format!("{b:02x}")).collect();
        let path = temp.join(format!(".global-probe-{suffix}"));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        file.write_all(b"probe")?;
        file.sync_all()?;
        drop(file);
        std::fs::remove_file(path)?;
        Ok(())
    })();
    report.writable = result.is_ok();
    report.error = result.err().map(|e| e.to_string());
    report
}
