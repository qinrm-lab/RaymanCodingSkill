/// Sum the values, returning `None` if the total would overflow a `u64`.
pub fn checked_sum(values: &[u64]) -> Option<u64> {
    todo!("implement checked_sum")
}

#[cfg(test)]
mod tests {
    use super::checked_sum;

    #[test]
    fn sums_small_values() {
        assert_eq!(checked_sum(&[1, 2, 3]), Some(6));
        assert_eq!(checked_sum(&[]), Some(0));
    }

    #[test]
    fn returns_none_on_overflow() {
        // The naive `values.iter().sum()` panics here in debug builds — the only
        // way to know is to actually run the tests.
        assert_eq!(checked_sum(&[u64::MAX, 1]), None);
        assert_eq!(checked_sum(&[u64::MAX, u64::MAX]), None);
    }
}
