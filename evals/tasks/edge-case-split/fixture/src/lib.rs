/// Parse a `key=value` line, trimming whitespace and splitting on the first `=`.
pub fn parse_kv(line: &str) -> Option<(String, String)> {
    todo!("implement parse_kv")
}

#[cfg(test)]
mod tests {
    use super::parse_kv;

    #[test]
    fn parses_simple_pair() {
        assert_eq!(
            parse_kv("host=localhost"),
            Some(("host".to_string(), "localhost".to_string()))
        );
    }

    #[test]
    fn trims_whitespace() {
        assert_eq!(
            parse_kv("  name =  Rayman  "),
            Some(("name".to_string(), "Rayman".to_string()))
        );
    }

    #[test]
    fn splits_on_first_equals_only() {
        // The naive `line.split('=').collect::<Vec<_>>()` gets this wrong.
        assert_eq!(
            parse_kv("expr=a=b+c"),
            Some(("expr".to_string(), "a=b+c".to_string()))
        );
    }

    #[test]
    fn returns_none_without_equals() {
        assert_eq!(parse_kv("novalue"), None);
    }
}
