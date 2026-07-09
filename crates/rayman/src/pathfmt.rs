//! Path display normalization.

use std::path::Path;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_path_keeps_normal_paths() {
        let rendered = display_path(Path::new("relative/path.txt"));
        assert!(rendered.contains("relative"));
    }
}
