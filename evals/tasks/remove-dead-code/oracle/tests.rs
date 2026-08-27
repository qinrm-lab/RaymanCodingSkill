use super::greeting;

#[test]
fn greets_by_name() { assert_eq!(greeting("Rayman"), "Hello, Rayman!"); }
