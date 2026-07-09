//! Persistence and utility entrypoint for context/project-map state.

pub use crate::file_io::{read_json, write_json};
pub use crate::hash::sha256_file;
pub use crate::pathfmt::display_path;
pub use crate::timefmt::now_iso;

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn project_store_exposes_time_and_path_helpers() {
        assert!(now_iso().ends_with('Z'));
        assert!(display_path(Path::new("project")).contains("project"));
    }
}
