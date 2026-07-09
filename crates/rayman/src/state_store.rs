//! Persistence and utility entrypoint for goal/autosave/checkpoint state.

pub use crate::file_io::{read_json, write_json};
pub use crate::pathfmt::display_path;
pub use crate::timefmt::now_iso;

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn state_store_exposes_time_and_path_helpers() {
        assert!(now_iso().ends_with('Z'));
        assert!(display_path(Path::new("state")).contains("state"));
    }
}
