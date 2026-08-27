use super::{parse_limit, secure_by_default};

#[test]
fn limit_is_bounded_and_malformed_input_is_rejected() {
    assert_eq!(parse_limit("25"), Some(25));
    assert_eq!(parse_limit("0"), None);
    assert_eq!(parse_limit("1001"), None);
    assert_eq!(parse_limit("bad"), None);
}

#[test]
fn configuration_is_secure_by_default() { assert!(secure_by_default()); }
