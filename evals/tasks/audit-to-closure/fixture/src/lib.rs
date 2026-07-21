pub fn parse_port(value: &str) -> Option<u16> {
    value.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_trimmed_nonzero_ports() {
        assert_eq!(parse_port(" 8080 "), Some(8080));
    }

    #[test]
    fn rejects_zero_and_out_of_range_values() {
        assert_eq!(parse_port("0"), None);
        assert_eq!(parse_port("65536"), None);
    }
}
