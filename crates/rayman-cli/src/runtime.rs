use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};

pub(crate) fn root() -> Result<PathBuf> {
    std::env::current_dir().context("无法读取当前工作区")
}

pub(crate) fn load_dotenv() {
    let _ = dotenvy::dotenv();
}

pub(crate) fn write_or_print(path: Option<&PathBuf>, label: &str, content: &str) -> Result<()> {
    if let Some(path) = path {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, content)?;
        println!("{label}: {}", path.display());
    } else {
        println!("{content}");
    }
    Ok(())
}
