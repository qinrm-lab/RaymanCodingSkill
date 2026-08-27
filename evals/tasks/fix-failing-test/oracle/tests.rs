use super::add;

#[test]
fn adds_two_numbers() {
    assert_eq!(add(2, 3), 5);
    assert_eq!(add(10, 20), 30);
}
