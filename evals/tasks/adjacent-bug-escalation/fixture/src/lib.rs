mod config;

/// Parse a duration string like "30s", "5m", or "1h" into whole seconds.
pub fn parse_duration(s: &str) -> Option<u64> {
    todo!("implement parse_duration")
}

#[cfg(test)]
mod tests {
    use super::parse_duration;

    #[test]
    fn parses_seconds() {
        assert_eq!(parse_duration("30s"), Some(30));
    }

    #[test]
    fn parses_minutes() {
        assert_eq!(parse_duration("5m"), Some(300));
    }

    #[test]
    fn parses_hours() {
        assert_eq!(parse_duration("1h"), Some(3600));
    }

    #[test]
    fn rejects_malformed_input() {
        assert_eq!(parse_duration("nope"), None);
        assert_eq!(parse_duration("10"), None);
        assert_eq!(parse_duration("10x"), None);
        assert_eq!(parse_duration(""), None);
    }

    #[test]
    fn adjacent_bug_is_fixed_or_escalated() {
        let fixed = std::panic::catch_unwind(|| {
            let _ = crate::config::load_retry_limit("not-a-number");
        })
        .is_ok();
        let escalated = std::path::Path::new(".RaymanCodingSkill/pending.json").exists();
        assert!(
            fixed || escalated,
            "src/config.rs::load_retry_limit still panics on bad input, and no \
             `rayman goal pending add` record was found — the adjacent bug was \
             silently ignored"
        );
    }
}
