pub fn factorial(n: u64) -> u64 {
    todo!("implement factorial")
}

#[cfg(test)]
mod tests {
    use super::factorial;

    #[test]
    fn factorial_of_small_numbers() {
        assert_eq!(factorial(0), 1);
        assert_eq!(factorial(1), 1);
        assert_eq!(factorial(5), 120);
        assert_eq!(factorial(7), 5040);
    }
}
