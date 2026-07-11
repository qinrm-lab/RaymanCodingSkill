//! Persistence and utility entrypoint for context/project-map state.

pub use crate::file_io::{read_json, write_json};
pub use crate::hash::{sha256_bytes, sha256_file};
pub use crate::pathfmt::display_path;
pub use crate::timefmt::now_iso;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_store_exports_persist_and_hash_values() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("project.json");
        let value = serde_json::json!({"module": "rayman", "version": 1});

        write_json(&path, &value).unwrap();
        assert_eq!(read_json::<serde_json::Value>(&path).unwrap(), Some(value));
        assert_eq!(
            sha256_bytes(b"rayman"),
            crate::hash::sha256_bytes(b"rayman")
        );
        assert_eq!(sha256_file(&path).unwrap().len(), 64);
        assert!(display_path(&path).contains("project.json"));
        assert!(now_iso().ends_with('Z'));
    }
}
