use super::checked_sum;

#[test]
fn sums_small_values() {
    assert_eq!(checked_sum(&[1, 2, 3]), Some(6));
    assert_eq!(checked_sum(&[]), Some(0));
}

#[test]
fn returns_none_on_overflow() {
    assert_eq!(checked_sum(&[u64::MAX, 1]), None);
    assert_eq!(checked_sum(&[u64::MAX, u64::MAX]), None);
}
