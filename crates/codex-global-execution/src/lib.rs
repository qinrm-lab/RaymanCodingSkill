//! Framework-independent local execution. No application workflow is loaded
//! into the desktop-user worker, and no application validation is implied.
pub mod cli;
mod cli_handler;
pub mod execution_context;
mod file_io;
pub mod global_execution;
pub mod hash;
pub mod pathfmt;
mod state_lock;
mod state_paths;

use anyhow::{Result, bail};
use std::{collections::BTreeMap, path::Path};

pub fn source_fingerprint(root: &Path) -> Result<String> {
    let root = std::fs::canonicalize(root)?;
    #[cfg(windows)]
    let source = global_execution::native::SourceDirectory::open(&root)?;
    let mut files = BTreeMap::new();
    let walker = ignore::WalkBuilder::new(&root)
        .hidden(false)
        .follow_links(false)
        .filter_entry(|entry| {
            entry.depth() == 0
                || !matches!(
                    entry.file_name().to_str(),
                    Some(
                        ".git"
                            | ".RaymanCodingSkill"
                            | ".agent-checkpoints"
                            | "target"
                            | "node_modules"
                            | "__pycache__"
                            | ".venv"
                    )
                )
        })
        .build();
    for entry in walker {
        let entry = entry?;
        if entry.depth() == 0 {
            continue;
        }
        let metadata = std::fs::symlink_metadata(entry.path())?;
        if file_io::is_link_or_reparse(&metadata) {
            bail!("source fingerprint refuses linked entries");
        }
        if metadata.is_dir() {
            continue;
        }
        if !metadata.is_file() || metadata.len() > 64 * 1024 * 1024 {
            bail!("source entry is not a bounded ordinary file");
        }
        let key = entry
            .path()
            .strip_prefix(&root)?
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("source path is not Unicode"))?
            .replace('\\', "/");
        #[cfg(windows)]
        let digest = hash::sha256_bytes(&source.read_file(Path::new(&key), 64 * 1024 * 1024)?);
        #[cfg(not(windows))]
        let digest = {
            let (bytes, _) =
                file_io::read_handle_bound_file(entry.path(), "source fingerprint input")?;
            hash::sha256_bytes(&bytes)
        };
        files.insert(key, digest);
        if files.len() > 100_000 {
            bail!("source inventory exceeds limit");
        }
    }
    // Tracked policy remains source even though operational state is excluded.
    let policy = root.join(".RaymanCodingSkill/quality.json");
    let policy_present = match std::fs::symlink_metadata(&policy) {
        Ok(_) => true,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        Err(e) => return Err(e.into()),
    };
    if policy_present {
        #[cfg(windows)]
        let bytes = source.read_file(
            Path::new(".RaymanCodingSkill/quality.json"),
            64 * 1024 * 1024,
        )?;
        #[cfg(not(windows))]
        let (bytes, _) = file_io::read_handle_bound_file(&policy, "source quality policy")?;
        files.insert(
            ".RaymanCodingSkill/quality.json".into(),
            hash::sha256_bytes(&bytes),
        );
    }
    Ok(hash::sha256_bytes(&serde_json::to_vec(&files)?))
}

pub fn run_cli() -> i32 {
    use clap::Parser;
    let args = cli::Cli::parse();
    let result = cli_handler::run(
        matches!(args.format, cli::Format::Json),
        &cli::GlobalExecutionCmd {
            action: args.action,
        },
    );
    match result {
        Ok(()) => 0,
        Err(error) => {
            use std::io::Write;
            let _ = writeln!(std::io::stderr(), "{error:#}");
            1
        }
    }
}

#[cfg(test)]
mod source_tests {
    use super::*;
    #[test]
    fn global_source_fingerprint_tracks_bytes_and_preserves_state_exclusion() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("source.txt"), b"hello\n").unwrap();
        let before = source_fingerprint(root.path()).unwrap();
        std::fs::create_dir(root.path().join(".agent-checkpoints")).unwrap();
        std::fs::write(root.path().join(".agent-checkpoints/state"), b"state").unwrap();
        assert_eq!(before, source_fingerprint(root.path()).unwrap());
        std::fs::write(root.path().join("source.txt"), b"hello\r\n").unwrap();
        assert_ne!(before, source_fingerprint(root.path()).unwrap());
        std::fs::create_dir(root.path().join(".RaymanCodingSkill")).unwrap();
        std::fs::write(root.path().join(".RaymanCodingSkill/quality.json"), b"{}").unwrap();
        let policy = source_fingerprint(root.path()).unwrap();
        std::fs::write(
            root.path().join(".RaymanCodingSkill/quality.json"),
            b"{\"changed\":true}",
        )
        .unwrap();
        assert_ne!(policy, source_fingerprint(root.path()).unwrap());
    }
}
