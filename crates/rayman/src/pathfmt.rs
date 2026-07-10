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
        assert_eq!(
            display_path(Path::new("relative/path.txt")),
            Path::new("relative/path.txt").display().to_string()
        );
    }

    #[test]
    fn display_path_strips_verbatim_prefix() {
        assert_eq!(
            display_path(Path::new(r"\\?\E:\ws\file.txt")),
            r"E:\ws\file.txt"
        );
    }

    #[test]
    fn display_path_rewrites_verbatim_unc_prefix() {
        assert_eq!(
            display_path(Path::new(r"\\?\UNC\server\share\file.txt")),
            r"\\server\share\file.txt"
        );
    }
}
