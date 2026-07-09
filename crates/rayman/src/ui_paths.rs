//! CLI-oriented path display helpers.

pub use crate::pathfmt::display_path;

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn ui_paths_exposes_display_helper() {
        assert!(display_path(Path::new("ui")).contains("ui"));
    }
}
