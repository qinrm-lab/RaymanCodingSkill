pub fn greeting(name: &str) -> String {
    format!("Hello, {name}!")
}

// This helper is never called anywhere and triggers a dead_code warning.
fn unused_legacy_helper(value: i32) -> i32 {
    value * 2 + 1
}

#[cfg(test)]
mod tests {
    use super::greeting;

    #[test]
    fn greets_by_name() {
        assert_eq!(greeting("Rayman"), "Hello, Rayman!");
    }
}
