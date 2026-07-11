//! Compatibility exports for filesystem and persistence helpers.
//! 只做转发，不复制实现——同一函数只允许一份实现体。

pub use crate::file_io::{read_json, read_text, write_atomic, write_json};
pub use crate::hash::sha256_file;
pub use crate::pathfmt::display_path;
pub use crate::timefmt::now_iso;

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn compatibility_exports_roundtrip_text_json_and_hashes() {
        let dir = tempfile::tempdir().unwrap();
        let text_path = dir.path().join("state.txt");
        let json_path = dir.path().join("state.json");

        write_atomic(&text_path, "hello").unwrap();
        assert_eq!(read_text(&text_path).unwrap(), "hello");

        let value = serde_json::json!({"ready": true, "count": 2});
        write_json(&json_path, &value).unwrap();
        assert_eq!(
            read_json::<serde_json::Value>(&json_path).unwrap(),
            Some(value)
        );

        let expected = sha256_file(&text_path).unwrap();
        assert_eq!(expected, crate::hash::sha256_bytes(b"hello"));
        assert_eq!(display_path(&text_path), text_path.display().to_string());
        assert!(now_iso().ends_with('Z'));

        fs::remove_file(text_path).unwrap();
    }
}
