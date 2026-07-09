//! Compatibility exports for filesystem and persistence helpers.

use std::path::Path;

use chrono::{SecondsFormat, Utc};

pub use crate::file_io::{read_json, read_text, write_atomic, write_json};
pub use crate::hash::sha256_file;
pub use crate::path_guard::ensure_within;

pub fn display_path(path: &Path) -> String {
    let text = path.display().to_string();
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = text.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        text
    }
}

pub fn now_iso() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Micros, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compatibility_display_path_keeps_normal_paths() {
        let rendered = display_path(Path::new("relative/path.txt"));
        assert!(rendered.contains("relative"));
    }

    #[test]
    fn compatibility_now_iso_uses_utc_timestamp() {
        let timestamp = now_iso();
        assert!(timestamp.contains('T'));
        assert!(timestamp.ends_with('Z'));
    }
}
