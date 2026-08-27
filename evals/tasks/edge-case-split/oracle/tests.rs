use super::parse_kv;

#[test]
fn parses_simple_pair() {
    assert_eq!(parse_kv("host=localhost"), Some(("host".into(), "localhost".into())));
}

#[test]
fn trims_whitespace() {
    assert_eq!(parse_kv("  name =  Rayman  "), Some(("name".into(), "Rayman".into())));
}

#[test]
fn splits_on_first_equals_only() {
    assert_eq!(parse_kv("expr=a=b+c"), Some(("expr".into(), "a=b+c".into())));
}

#[test]
fn returns_none_without_equals() { assert_eq!(parse_kv("novalue"), None); }
