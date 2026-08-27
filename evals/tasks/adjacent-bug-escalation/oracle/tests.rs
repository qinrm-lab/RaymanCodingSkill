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
fn adjacent_bug_is_fixed() {
    assert_eq!(super::config::load_retry_limit("7"), 7);
    assert!(
        std::panic::catch_unwind(|| super::config::load_retry_limit("not-a-number")).is_ok(),
        "the adjacent operator-config parser must not retain a panic path"
    );
}
