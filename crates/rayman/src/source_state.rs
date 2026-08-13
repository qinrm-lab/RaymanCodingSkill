use std::path::Path;
use std::process::Command;

use serde::Serialize;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SourceState {
    pub kind: String,
    pub available: bool,
    pub clean: Option<bool>,
    pub head: Option<String>,
    pub tracked_dirty: usize,
    pub untracked: usize,
    pub path_encoding_lossy: bool,
    pub changed_paths: Vec<String>,
    /// Hash of the exact `git status --porcelain=v1 -z` bytes. Counts and
    /// paths alone cannot distinguish staged from unstaged XY state.
    pub porcelain_sha256: Option<String>,
    pub error: Option<String>,
}

pub fn inspect(root: &Path) -> SourceState {
    let inside = git_text(root, &["rev-parse", "--is-inside-work-tree"]);
    match inside {
        Ok(value) if value.trim() == "true" => {}
        Ok(_) => return unavailable("not_repository"),
        Err(error) if error.contains("not a git repository") => {
            return unavailable("not_repository");
        }
        Err(error) => return failed(error),
    }

    let head_before = match git_text(root, &["rev-parse", "HEAD"]) {
        Ok(value) => value.trim().to_string(),
        Err(error) => return failed(error),
    };
    let status = match git_bytes(
        root,
        &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
    ) {
        Ok(value) => value,
        Err(error) => return failed(error),
    };
    let parsed = match parse_porcelain_z(&status) {
        Ok(parsed) => parsed,
        Err(error) => return failed(error),
    };
    let head_after = match git_text(root, &["rev-parse", "HEAD"]) {
        Ok(value) => value.trim().to_string(),
        Err(error) => return failed(error),
    };
    if head_before != head_after {
        return failed("git HEAD changed while source state was captured".into());
    }

    SourceState {
        kind: "git".into(),
        available: true,
        clean: Some(parsed.tracked_dirty == 0 && parsed.untracked == 0),
        head: Some(head_after),
        tracked_dirty: parsed.tracked_dirty,
        untracked: parsed.untracked,
        changed_paths: parsed.changed_paths,
        path_encoding_lossy: parsed.path_encoding_lossy,
        porcelain_sha256: Some(crate::hash::sha256_bytes(&status)),
        error: None,
    }
}

#[derive(Debug, PartialEq, Eq)]
struct ParsedStatus {
    tracked_dirty: usize,
    untracked: usize,
    changed_paths: Vec<String>,
    path_encoding_lossy: bool,
}

fn push_path(parsed: &mut ParsedStatus, path: &[u8]) {
    if std::str::from_utf8(path).is_err() {
        parsed.path_encoding_lossy = true;
    }
    parsed
        .changed_paths
        .push(String::from_utf8_lossy(path).into_owned());
}

fn parse_porcelain_z(status: &[u8]) -> Result<ParsedStatus, String> {
    let records = status.split(|byte| *byte == 0).collect::<Vec<_>>();
    let mut parsed = ParsedStatus {
        tracked_dirty: 0,
        untracked: 0,
        changed_paths: Vec::new(),
        path_encoding_lossy: false,
    };
    let mut index = 0;
    while index < records.len() {
        let record = records[index];
        if record.is_empty() {
            index += 1;
            continue;
        }
        if record.len() < 4 || record[2] != b' ' {
            return Err("malformed git porcelain -z record".into());
        }
        let status_code = &record[..2];
        if status_code == b"??" {
            parsed.untracked += 1;
        } else {
            parsed.tracked_dirty += 1;
        }
        push_path(&mut parsed, &record[3..]);
        if status_code.iter().any(|code| matches!(code, b'R' | b'C')) {
            index += 1;
            let Some(source_path) = records.get(index).filter(|path| !path.is_empty()) else {
                return Err("rename/copy git porcelain record is missing its second path".into());
            };
            push_path(&mut parsed, source_path);
        }
        index += 1;
    }
    Ok(parsed)
}

fn git_bytes(root: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .map_err(|error| format!("cannot execute git: {error}"))?;
    if output.status.success() {
        return Ok(output.stdout);
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    Err(if stderr.is_empty() {
        format!("git {} exited {}", args.join(" "), output.status)
    } else {
        stderr
    })
}

fn git_text(root: &Path, args: &[&str]) -> Result<String, String> {
    git_bytes(root, args).map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
}

fn unavailable(kind: &str) -> SourceState {
    SourceState {
        kind: kind.into(),
        available: false,
        clean: None,
        head: None,
        path_encoding_lossy: false,
        tracked_dirty: 0,
        untracked: 0,
        changed_paths: Vec::new(),
        porcelain_sha256: None,
        error: None,
    }
}

fn failed(error: String) -> SourceState {
    SourceState {
        kind: "git_error".into(),
        available: false,
        clean: None,
        head: None,
        path_encoding_lossy: false,
        tracked_dirty: 0,
        untracked: 0,
        changed_paths: Vec::new(),
        porcelain_sha256: None,
        error: Some(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn porcelain_z_preserves_spaces_controls_and_rename_paths() {
        let parsed = parse_porcelain_z(
            b"?? name with spaces.txt\0 M line\nfeed.txt\0R  renamed.txt\0old name.txt\0",
        )
        .unwrap();
        assert_eq!(parsed.untracked, 1);
        assert_eq!(parsed.tracked_dirty, 2);
        assert_eq!(
            parsed.changed_paths,
            [
                "name with spaces.txt",
                "line\nfeed.txt",
                "renamed.txt",
                "old name.txt"
            ]
        );
        assert!(!parsed.path_encoding_lossy);
    }

    #[test]
    fn porcelain_z_marks_invalid_utf8_lossy_without_losing_the_record() {
        let parsed = parse_porcelain_z(b"?? bad\xffname\0").unwrap();
        assert_eq!(parsed.untracked, 1);
        assert_eq!(parsed.changed_paths.len(), 1);
        assert!(parsed.changed_paths[0].contains('\u{fffd}'));
        assert!(parsed.path_encoding_lossy);
    }

    #[test]
    fn porcelain_z_rejects_truncated_rename_records() {
        assert!(
            parse_porcelain_z(b"R  renamed.txt\0")
                .unwrap_err()
                .contains("second path")
        );
    }
}
